//! Shared configured-transcoding service (ADR-0147).
//!
//! One application-layer service owns all streaming FFmpeg execution for the
//! OpenSubsonic and Jellyfin adapters (native v1 exposes no format/bitrate
//! parameters and is untouched). Templates come from the seeded
//! `transcoding.command.{aac,mp3,opus}` settings (ADR-0108's designated
//! mechanism, corrected from the malformed legacy seeds): each holds a JSON
//! object `{ "format":..., "bitrate":..., "command":... }` whose `command`
//! string is split into an argv array under shell-words rules — no shell is
//! ever invoked. Placeholders: `%s` → `pipe:0` (the validated, no-follow-opened
//! asset file is passed as the child's stdin, preserving the dirfd
//! race-resistance guarantee end to end; see the ADR-0147 decision 2
//! amendment), `%t` → start offset seconds (always `0`; `timeOffset` stays
//! rejected), `%bk` → target bitrate in kbps.
//!
//! Concurrency is bounded independently of direct play by
//! `transcoding.maxConcurrent` (decision 5); acquisition is non-blocking —
//! exhaustion rejects immediately. Cancellation: `kill_on_drop(true)` kills the
//! child on client disconnect or shutdown, and tokio's orphan reaping
//! guarantees the process is reaped; a normally completing stream awaits
//! `wait()` explicitly and logs a sanitized stderr tail on non-zero exit. No
//! wall-clock timeout is imposed on an established stream.

use std::io;
use std::process::Stdio;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use futures_util::stream;
use melodee_persistence::SettingsResolver;

use crate::resource_bounds::ResourceGauge;
use sqlx::PgPool;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

use crate::native_stream::{CanonicalAssetRoot, PlayableAsset};

/// Bytes read from FFmpeg stdout per body chunk (matches the direct-play
/// chunk granularity so proxy/memory behavior is uniform).
const STDOUT_CHUNK_SIZE: usize = 64 * 1024;
/// Bounded stderr capture for diagnostics. The drain task keeps reading past
/// the cap (discarding) so a chatty FFmpeg never deadlocks on a full pipe.
const STDERR_CAPTURE_CAP: usize = 64 * 1024;
/// Only the tail of stderr is logged; ffmpeg prefixes lines with progress
/// noise and the actionable error sits at the end.
const STDERR_LOG_TAIL: usize = 512;

/// Output formats with a configured template surface (ADR-0147 decision 2).
/// The settings key selects the format; the template JSON's own `format`
/// field is informational and not cross-checked (the command is
/// authoritative, matching the legacy configuration shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Mp3,
    Aac,
    Opus,
}

impl OutputFormat {
    /// Parses an adapter-supplied format tag (`mp3`/`aac`/`opus`,
    /// case-insensitive). Anything else is an unsupported format.
    #[must_use]
    pub fn parse_tag(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "mp3" => Some(Self::Mp3),
            "aac" => Some(Self::Aac),
            "opus" => Some(Self::Opus),
            _ => None,
        }
    }

    /// Wire `Content-Type` for transcoded responses (ADR-0147 decision 3/4).
    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Aac => "audio/aac",
            // The seeded opus template emits `-f opus` (Ogg/Opus).
            Self::Opus => "audio/ogg",
        }
    }

    /// File extension used for transcoded `download.view` attachment names.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::Opus => "opus",
        }
    }

    fn template_key(self) -> &'static str {
        match self {
            Self::Mp3 => "transcoding.command.mp3",
            Self::Aac => "transcoding.command.aac",
            Self::Opus => "transcoding.command.opus",
        }
    }
}

/// Why a transcode request could not be served. Adapters map these onto their
/// own wire error shapes (ADR-0102): OpenSubsonic uses the error-0 envelope at
/// HTTP 200 (with the captured 404+70 class retained for missing assets);
/// Jellyfin uses 400/503 problem responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscodeError {
    /// Missing, unparseable, or non-substitutable template (fail closed).
    TemplateUnavailable,
    /// The settings store could not be read at all.
    SettingsUnavailable,
    /// `transcoding.maxConcurrent` capacity exhausted (non-blocking acquire).
    CapacityExhausted,
    /// FFmpeg failed to spawn (missing binary, resource exhaustion).
    SpawnFailed,
    /// The catalog asset failed no-follow open/size validation.
    AssetUnavailable,
}

impl std::fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::TemplateUnavailable => "transcoding template unavailable",
            Self::SettingsUnavailable => "transcoding settings unavailable",
            Self::CapacityExhausted => "transcoding capacity exhausted",
            Self::SpawnFailed => "transcoding tool failed to start",
            Self::AssetUnavailable => "media asset unavailable",
        };
        f.write_str(message)
    }
}

impl std::error::Error for TranscodeError {}

/// A parsed, validated FFmpeg command template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscodeTemplate {
    /// Template-default target bitrate in kbps, used when the request does
    /// not override it (ADR-0147 decisions 3/4).
    pub bitrate_kbps: u32,
    argv: Vec<String>,
}

#[derive(serde::Deserialize)]
struct TemplateJson {
    bitrate: u32,
    command: String,
}

impl TranscodeTemplate {
    /// Parses a template JSON object. Fails closed on malformed JSON, a
    /// missing/zero `bitrate`, or a `command` that does not split into a
    /// non-empty argv (unbalanced quotes).
    pub fn parse(raw: &str) -> Result<Self, TranscodeError> {
        let json: TemplateJson =
            serde_json::from_str(raw).map_err(|_| TranscodeError::TemplateUnavailable)?;
        if json.bitrate == 0 {
            return Err(TranscodeError::TemplateUnavailable);
        }
        let argv = shlex::split(&json.command)
            .filter(|argv| !argv.is_empty() && !argv[0].is_empty())
            .ok_or(TranscodeError::TemplateUnavailable)?;
        Ok(Self {
            bitrate_kbps: json.bitrate,
            argv,
        })
    }

    /// Substitutes `%s`/`%t`/`%bk` placeholders. A template whose argv never
    /// references `%s` cannot consume the source stream and is rejected
    /// (ADR-0147: "non-substitutable template fails closed").
    fn render_argv(&self, target_bitrate_kbps: u32) -> Result<Vec<String>, TranscodeError> {
        // ffmpeg bitrate arguments take the `k` suffix (`-b:a 128k`); a bare
        // number is *bits* per second, which libopus rejects outright.
        let bitrate = format!("{target_bitrate_kbps}k");
        let mut consumes_input = false;
        let argv: Vec<String> = self
            .argv
            .iter()
            .map(|token| {
                if token.contains("%s") {
                    consumes_input = true;
                }
                token
                    .replace("%bk", &bitrate)
                    .replace("%t", "0")
                    .replace("%s", "pipe:0")
            })
            .collect();
        if !consumes_input {
            return Err(TranscodeError::TemplateUnavailable);
        }
        Ok(argv)
    }
}

/// Bounded, cancellation-safe FFmpeg transcoding over catalog assets.
#[derive(Debug, Clone)]
pub struct TranscodingService {
    root: CanonicalAssetRoot,
    pool: PgPool,
    permits: Arc<Semaphore>,
    gauge: Option<Arc<crate::resource_bounds::ResourceGauge>>,
}

impl TranscodingService {
    /// `max_concurrent == 0` disables transcoding entirely (ADR-0147 decision
    /// 5): routes then fail transcode requests closed while direct play is
    /// unaffected.
    #[must_use]
    pub fn new(root: CanonicalAssetRoot, pool: PgPool, max_concurrent: usize) -> Option<Self> {
        (max_concurrent > 0).then(|| Self {
            root,
            pool,
            permits: Arc::new(Semaphore::new(max_concurrent)),
            gauge: None,
        })
    }

    /// Attaches the process resource-bounds gauge for the transcoding class
    /// non-blocking admission failures are counted as rejections and
    /// in-flight utilization appears on the admin health surface.
    #[must_use]
    pub fn with_gauge(mut self, gauge: Arc<crate::resource_bounds::ResourceGauge>) -> Self {
        self.gauge = Some(gauge);
        self
    }

    /// Resolves and parses the configured template for `format`. Resolution
    /// is per request so admin edits take effect without restart; every
    /// failure mode fails closed with a sanitized log.
    pub async fn template(
        &self,
        format: OutputFormat,
    ) -> Result<TranscodeTemplate, TranscodeError> {
        let resolver = SettingsResolver::new(&self.pool);
        let resolved = resolver
            .resolve(format.template_key())
            .await
            .map_err(|error| {
                tracing::warn!(%error, ?format, "transcoding template resolution failed");
                TranscodeError::SettingsUnavailable
            })?;
        let raw = resolved
            .value
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                tracing::warn!(?format, "transcoding template setting has no value");
                TranscodeError::TemplateUnavailable
            })?;
        TranscodeTemplate::parse(raw).inspect_err(|error| {
            tracing::warn!(%error, ?format, "transcoding template is not parseable");
        })
    }

    /// Starts a bounded transcoding stream. The permit, child handle, and
    /// stderr drain all live inside the returned body's stream state, so a
    /// client disconnect drops them together: the permit is released and
    /// `kill_on_drop` kills the child (tokio reaps orphaned children through
    /// its driver queue, so no zombie accumulates).
    pub async fn stream(
        &self,
        asset: &PlayableAsset,
        format: OutputFormat,
        template: &TranscodeTemplate,
        target_bitrate_kbps: u32,
    ) -> Result<Body, TranscodeError> {
        let argv = template.render_argv(target_bitrate_kbps)?;
        let permit = match self.permits.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(error) => {
                if let Some(gauge) = &self.gauge {
                    gauge.reject();
                }
                let _ = error;
                return Err(TranscodeError::CapacityExhausted);
            }
        };
        let gauge_permit = self.gauge.as_ref().map(ResourceGauge::admit_immediate);
        // Validated no-follow open (catalog size-checked): the file itself —
        // not a re-joined path — becomes the child's stdin.
        let file = self
            .root
            .open_asset(asset)
            .map_err(|_| TranscodeError::AssetUnavailable)?;
        let mut command = Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .stdin(Stdio::from(file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            tracing::warn!(%error, ?format, "ffmpeg spawn failed");
            TranscodeError::SpawnFailed
        })?;
        let stdout = child.stdout.take().ok_or(TranscodeError::SpawnFailed)?;
        let stderr = child.stderr.take().ok_or(TranscodeError::SpawnFailed)?;
        let stderr_task = tokio::spawn(drain_bounded(stderr, STDERR_CAPTURE_CAP));
        tracing::debug!(?format, target_bitrate_kbps, "transcode stream started");
        Ok(Body::from_stream(stream::try_unfold(
            ChildStreamState {
                stdout,
                child,
                stderr_task,
                _permit: permit,
                _gauge: gauge_permit,
                format,
            },
            |mut state| async move {
                let mut buffer = vec![0_u8; STDOUT_CHUNK_SIZE];
                match state.stdout.read(&mut buffer).await {
                    Ok(0) => {
                        state.finish().await;
                        Ok(None)
                    }
                    Ok(read) => {
                        buffer.truncate(read);
                        Ok(Some((Bytes::from(buffer), state)))
                    }
                    Err(error) => {
                        tracing::warn!(%error, format = ?state.format, "transcode stdout read failed");
                        state.finish().await;
                        Err(io::Error::other("transcode stream read failure"))
                    }
                }
            },
        )))
    }
}

struct ChildStreamState {
    stdout: ChildStdout,
    child: Child,
    stderr_task: JoinHandle<String>,
    _permit: OwnedSemaphorePermit,
    _gauge: Option<crate::resource_bounds::GaugePermit>,
    format: OutputFormat,
}

impl ChildStreamState {
    /// Normal-completion path: stdout reached EOF, so await exit status and
    /// log a sanitized stderr tail on failure. Never logs argv or configured
    /// paths; ffmpeg identifies piped input as `pipe:0`.
    async fn finish(&mut self) {
        let status = self.child.wait().await;
        let stderr = (&mut self.stderr_task).await.unwrap_or_default();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                tracing::warn!(
                    %status,
                    format = ?self.format,
                    stderr_tail = %stderr_tail(&stderr),
                    "ffmpeg transcode exited non-zero"
                );
            }
            Err(error) => {
                tracing::warn!(%error, format = ?self.format, "ffmpeg wait failed");
            }
        }
    }
}

/// Drains stderr into a bounded buffer; keeps draining (discarding) past the
/// cap so the child never blocks on a full pipe.
async fn drain_bounded(mut stderr: ChildStderr, cap: usize) -> String {
    let mut captured = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = cap.saturating_sub(captured.len());
                if room > 0 {
                    captured.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
        }
    }
    String::from_utf8_lossy(&captured).into_owned()
}

fn stderr_tail(stderr: &str) -> &str {
    let start = stderr.len().saturating_sub(STDERR_LOG_TAIL);
    // Avoid splitting a UTF-8 sequence when the cap landed mid-codepoint.
    let boundary = stderr.ceil_char_boundary(start);
    &stderr[boundary..]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn output_format_tags_parse_case_insensitively() {
        assert_eq!(OutputFormat::parse_tag("mp3"), Some(OutputFormat::Mp3));
        assert_eq!(OutputFormat::parse_tag("AAC"), Some(OutputFormat::Aac));
        assert_eq!(OutputFormat::parse_tag(" opus "), Some(OutputFormat::Opus));
        assert_eq!(OutputFormat::parse_tag("flac"), None);
        assert_eq!(OutputFormat::parse_tag("raw"), None);
        assert_eq!(OutputFormat::parse_tag(""), None);
    }

    #[test]
    fn corrected_seed_templates_parse_and_render() {
        // Mirrors the corrected KNOWN_SETTINGS seeds (ADR-0147 decision 2).
        for (raw, expected_bitrate) in [
            (
                "{\"format\":\"Aac\",\"bitrate\":256,\"command\":\"ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -c:a aac -f adts -\"}",
                256,
            ),
            (
                "{\"format\":\"Mp3\",\"bitrate\":192,\"command\":\"ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -f mp3 -\"}",
                192,
            ),
            (
                "{\"format\":\"Opus\",\"bitrate\":128,\"command\":\"ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -c:a libopus -f opus -\"}",
                128,
            ),
        ] {
            let template = TranscodeTemplate::parse(raw).expect("seed template parses");
            assert_eq!(template.bitrate_kbps, expected_bitrate);
            let argv = template.render_argv(96).expect("seed template renders");
            assert_eq!(argv[0], "ffmpeg");
            assert!(argv.iter().any(|token| token == "pipe:0"));
            assert!(argv.iter().any(|token| token == "96k"));
            assert!(argv.iter().any(|token| token == "0"));
            assert!(!argv.iter().any(|token| token.contains('%')));
        }
    }

    #[test]
    fn malformed_templates_fail_closed() {
        // The legacy malformed seed shape (unbalanced quotes / single-quoted
        // pseudo-JSON) must never reach spawn.
        let legacy = "{ 'format': 'Mp3', 'bitrate: 192, 'command': 'ffmpeg -i %s -f mp3 -' }";
        assert_eq!(
            TranscodeTemplate::parse(legacy),
            Err(TranscodeError::TemplateUnavailable)
        );
        assert_eq!(
            TranscodeTemplate::parse("not json"),
            Err(TranscodeError::TemplateUnavailable)
        );
        assert_eq!(
            TranscodeTemplate::parse("{\"bitrate\":0,\"command\":\"ffmpeg -i %s -\"}"),
            Err(TranscodeError::TemplateUnavailable)
        );
        assert_eq!(
            TranscodeTemplate::parse("{\"bitrate\":192,\"command\":\"\"}"),
            Err(TranscodeError::TemplateUnavailable)
        );
        assert_eq!(
            TranscodeTemplate::parse("{\"bitrate\":192,\"command\":\"ffmpeg -i 'unterminated\"}"),
            Err(TranscodeError::TemplateUnavailable)
        );
    }

    #[test]
    fn template_without_input_placeholder_is_rejected() {
        let template = TranscodeTemplate::parse(
            "{\"bitrate\":192,\"command\":\"ffmpeg -f lavfi -i sine -f mp3 -\"}",
        )
        .expect("parses");
        assert_eq!(
            template.render_argv(192),
            Err(TranscodeError::TemplateUnavailable)
        );
    }

    #[test]
    fn quoted_command_tokens_survive_splitting() {
        let template = TranscodeTemplate::parse(
            "{\"bitrate\":128,\"command\":\"ffmpeg -i %s -metadata title=\\\"a b\\\" -f mp3 -\"}",
        )
        .expect("parses");
        let argv = template.render_argv(128).expect("renders");
        assert!(argv.iter().any(|token| token == "title=a b"));
    }

    #[test]
    fn stderr_tail_truncates_to_suffix() {
        let long = "x".repeat(4096);
        assert_eq!(stderr_tail(&long).len(), STDERR_LOG_TAIL);
        assert_eq!(stderr_tail("short"), "short");
        // Mid-codepoint caps snap forward to a char boundary.
        let unicode = format!("{}é", "y".repeat(STDERR_LOG_TAIL));
        let tail = stderr_tail(&unicode);
        assert!(tail.ends_with('é'));
        assert!(tail.len() <= STDERR_LOG_TAIL + 2);
    }
}
