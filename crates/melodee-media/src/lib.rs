//! Bounded, read-only media observation for Release inspection.
//!
//! This crate owns the boundary to Lofty and `ffprobe`. It returns Melodee
//! manifest types so callers never depend on either third-party representation.

#![forbid(unsafe_code)]

use std::{
    cmp::Reverse,
    collections::BTreeMap,
    fs::{self, DirEntry},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration as StdDuration, Instant},
};

use blake3::Hasher;
use lofty::{
    config::WriteOptions,
    file::{AudioFile, TaggedFileExt},
    prelude::Accessor,
    read_from, read_from_path,
    tag::{ItemKey, Tag},
};
use melodee_domain::{
    AudioTechnicalObservation, Finding, FindingSeverity, ManifestReleaseObservation,
    ObservedArtwork, ObservedFile, ObservedFileKind, ObservedMetadata, ObservedTrack,
    ReleaseManifest,
};
use serde::Deserialize;
use thiserror::Error;
use time::{OffsetDateTime, UtcOffset};
use tracing::{info, warn};
use uuid::Uuid;

const DEFAULT_FFPROBE_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const DEFAULT_MAX_TOOL_OUTPUT: usize = 1_048_576;
const DEFAULT_MAX_ARTWORK_BYTES: usize = 16 * 1024 * 1024;
const HASH_BUFFER_SIZE: usize = 128 * 1024;

/// media-tool invocation telemetry. One debug event at start and one at
/// drop (covering every early-return path) with the tool name, the file
/// **name only** — absolute source roots stay out of operator logs — and the
/// elapsed duration and outcome. `tokio::task::spawn_blocking` propagates the
/// caller's tracing span, so these events inherit the enclosing work-item
/// span's `work_item_id`/`correlation_id` fields.
pub(crate) struct ToolInvocation {
    tool: &'static str,
    file_name: String,
    started: Instant,
    outcome: Option<&'static str>,
}

impl ToolInvocation {
    pub(crate) fn start(tool: &'static str, path: &Path) -> Self {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unknown>".to_owned());
        tracing::debug!(tool, file = %file_name, "media tool invocation started");
        Self {
            tool,
            file_name,
            started: Instant::now(),
            outcome: None,
        }
    }

    pub(crate) fn set_outcome(&mut self, outcome: &'static str) {
        self.outcome = Some(outcome);
    }
}

impl Drop for ToolInvocation {
    fn drop(&mut self) {
        tracing::debug!(
            tool = self.tool,
            file = %self.file_name,
            duration_ms = self.started.elapsed().as_millis() as u64,
            outcome = self.outcome.unwrap_or("interrupted"),
            "media tool invocation finished"
        );
    }
}

mod normalize;
pub mod operation_metrics;
mod secure_fs;

pub use normalize::{
    NormalizationError, NormalizationOptions, NormalizationReport, normalize_release,
};

/// Audio extensions the inspection classifier treats as media (the song
/// files the legacy `directoryProcessingStart` hook context counts). Shared
/// by the scripting-hook context builder so the hook sees exactly
/// the inspection classifier's vocabulary.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "ac3", "aif", "aiff", "ape", "flac", "m4a", "m4b", "m4p", "mp2", "mp3", "mp4", "mpc",
    "ogg", "opus", "spx", "wav", "wma", "wv",
];

/// Whether `name` ends in a known audio extension (case-insensitive),
/// matching the inspection classifier's audio branch.
#[must_use]
pub fn is_audio_file_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension.as_str()))
}

/// Lightweight tag probe for the scripting-hook context (ADR-0135):
/// duration from container properties and track number from the primary
/// tag. Mirrors the legacy `DirectoryContextProvider`, which read real tags
/// through song plugins and skipped files it could not process — hence
/// `Option`, never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaTagProbe {
    pub duration_ms: Option<u64>,
    pub track_number: Option<u32>,
}

/// Tag fields that the safe metadata transformation layer is allowed to
/// change. `None` means the field is left untouched; clearing a field is
/// represented by `clear_*` so an absent JSON property cannot accidentally
/// erase source metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagRewrite {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub recording_date: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub clear_artist: bool,
    pub clear_album_artist: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSnapshot {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub recording_date: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
}

#[derive(Debug, Error)]
pub enum TagRewriteError {
    #[error("media tag file could not be read")]
    Read(#[source] lofty::error::FileParseError),
    #[error("media tag file could not be written")]
    Write(#[source] lofty::error::FileEncodingError),
    #[error("media container does not support a writable primary tag")]
    Unsupported,
}

/// Reads the normalized tag view used by the safe transformation engine.
pub fn read_tag_snapshot(path: &Path) -> Result<TagSnapshot, TagRewriteError> {
    let tagged = read_from_path(path).map_err(TagRewriteError::Read)?;
    Ok(snapshot_from_tagged(&tagged))
}

/// Applies only the named tag fields to an already-created same-filesystem
/// temporary copy. The caller owns atomic replacement and rollback; this
/// function never touches the original path.
pub fn apply_tag_rewrite(
    path: &Path,
    rewrite: &TagRewrite,
) -> Result<TagSnapshot, TagRewriteError> {
    let mut tagged = read_from_path(path).map_err(TagRewriteError::Read)?;
    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        if !tagged.tag_support(tag_type).is_writable() {
            return Err(TagRewriteError::Unsupported);
        }
        tagged.insert_tag(Tag::new(tag_type));
    }
    let Some(tag) = tagged.primary_tag_mut() else {
        return Err(TagRewriteError::Unsupported);
    };
    if let Some(value) = rewrite.title.as_deref() {
        tag.set_title(value.to_owned());
    }
    if let Some(value) = rewrite.artist.as_deref() {
        tag.set_artist(value.to_owned());
    }
    if rewrite.clear_artist {
        tag.remove_artist();
    }
    if let Some(value) = rewrite.album_artist.as_deref() {
        tag.insert_text(ItemKey::AlbumArtist, value.to_owned());
    }
    if rewrite.clear_album_artist {
        tag.remove_key(ItemKey::AlbumArtist);
    }
    if let Some(value) = rewrite.album.as_deref() {
        tag.set_album(value.to_owned());
    }
    if let Some(value) = rewrite.recording_date.as_deref() {
        tag.insert_text(ItemKey::RecordingDate, value.to_owned());
    }
    if let Some(value) = rewrite.track_number {
        tag.set_track(value);
    }
    if let Some(value) = rewrite.disc_number {
        tag.set_disk(value);
    }
    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(TagRewriteError::Write)?;
    Ok(snapshot_from_tagged(&tagged))
}

fn snapshot_from_tagged(tagged: &lofty::file::TaggedFile) -> TagSnapshot {
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    TagSnapshot {
        title: tag.and_then(|tag| tag.title().map(|value| value.into_owned())),
        artist: tag.and_then(|tag| tag.artist().map(|value| value.into_owned())),
        album_artist: tag
            .and_then(|tag| tag.get_string(ItemKey::AlbumArtist).map(ToOwned::to_owned)),
        album: tag.and_then(|tag| tag.album().map(|value| value.into_owned())),
        recording_date: tag.and_then(|tag| {
            tag.get_string(ItemKey::RecordingDate)
                .or_else(|| tag.get_string(ItemKey::Year))
                .map(ToOwned::to_owned)
        }),
        track_number: tag.and_then(Accessor::track),
        disc_number: tag.and_then(Accessor::disk),
    }
}

// Every tool is admitted before a supervisor thread or runtime is created.
// The slot remains held until the child is reaped and all owned pipes close.
const MAX_SUPERVISED_TOOLS: usize = 8;
static TOOL_ADMISSION: std::sync::OnceLock<(std::sync::Mutex<usize>, std::sync::Condvar)> =
    std::sync::OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub(crate) enum SupervisedToolError {
    #[error("canceled")]
    Canceled,
    #[error("timed out")]
    Timeout,
    #[error("tool process error: {0}")]
    Io(#[from] io::Error),
    #[error("tool supervisor thread failed")]
    Supervisor,
}

#[derive(Clone, Copy)]
pub(crate) enum ToolStdout {
    Capture(usize),
    PcmFingerprint,
    None,
}

pub(crate) struct SupervisedToolOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub fingerprint: Option<String>,
}

struct ToolSlot;
impl Drop for ToolSlot {
    fn drop(&mut self) {
        let (lock, ready) = TOOL_ADMISSION.get_or_init(Default::default);
        if let Ok(mut count) = lock.lock() {
            *count = count.saturating_sub(1);
            ready.notify_one();
        }
    }
}

fn tool_canceled(cancellation: Option<&AtomicBool>, shutdown: Option<&AtomicBool>) -> bool {
    cancellation.is_some_and(|flag| flag.load(Ordering::SeqCst))
        || shutdown.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

fn acquire_tool_slot(
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
) -> Result<ToolSlot, SupervisedToolError> {
    let (lock, ready) = TOOL_ADMISSION.get_or_init(Default::default);
    let mut count = lock.lock().map_err(|_| SupervisedToolError::Supervisor)?;
    loop {
        if tool_canceled(cancellation, shutdown) {
            return Err(SupervisedToolError::Canceled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SupervisedToolError::Timeout);
        }
        if *count < MAX_SUPERVISED_TOOLS {
            *count += 1;
            return Ok(ToolSlot);
        }
        let (next, _) = ready
            .wait_timeout(count, remaining.min(StdDuration::from_millis(25)))
            .map_err(|_| SupervisedToolError::Supervisor)?;
        count = next;
    }
}

async fn tool_cancel_or_timeout(
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
) -> SupervisedToolError {
    loop {
        if tool_canceled(cancellation, shutdown) {
            return SupervisedToolError::Canceled;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return SupervisedToolError::Timeout;
        }
        tokio::time::sleep(remaining.min(StdDuration::from_millis(10))).await;
    }
}

async fn read_tool_pipe<R: tokio::io::AsyncRead + Unpin>(
    reader: Option<R>,
    mode: ToolStdout,
) -> io::Result<(Vec<u8>, Option<String>)> {
    use tokio::io::AsyncReadExt;
    let Some(mut reader) = reader else {
        return Ok((Vec::new(), None));
    };
    let mut output = Vec::new();
    let mut hasher = matches!(mode, ToolStdout::PcmFingerprint).then(|| {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"melodee-decoded-audio-pcm-v1\0");
        hasher
    });
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        if let Some(hasher) = &mut hasher {
            hasher.update(&buffer[..count]);
        }
        if let ToolStdout::Capture(limit) = mode {
            let remaining = limit.saturating_sub(output.len());
            output.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    }
    Ok((
        output,
        hasher.map(|hash| hash.finalize().to_hex().to_string()),
    ))
}

#[cfg(windows)]
type ToolChild = Box<dyn process_wrap::tokio::ChildWrapper>;
#[cfg(not(windows))]
type ToolChild = tokio::process::Child;

fn spawn_with_budget<T>(
    mut spawn: impl FnMut() -> io::Result<T>,
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
) -> Result<T, SupervisedToolError> {
    normalize::retry_executable_busy(|| {
        if tool_canceled(cancellation, shutdown) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "tool canceled before spawn",
            ));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tool deadline expired before spawn",
            ));
        }
        spawn()
    })
    .map_err(|error| {
        if tool_canceled(cancellation, shutdown) {
            SupervisedToolError::Canceled
        } else if Instant::now() >= deadline {
            SupervisedToolError::Timeout
        } else {
            SupervisedToolError::Io(error)
        }
    })
}

#[cfg(windows)]
fn spawn_supervised(
    command: tokio::process::Command,
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
) -> Result<ToolChild, SupervisedToolError> {
    use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
    let mut wrapped = CommandWrap::from(command);
    wrapped.wrap(KillOnDrop).wrap(JobObject);
    spawn_with_budget(|| wrapped.spawn(), deadline, cancellation, shutdown)
}

#[cfg(not(windows))]
fn spawn_supervised(
    mut command: tokio::process::Command,
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
) -> Result<ToolChild, SupervisedToolError> {
    spawn_with_budget(|| command.spawn(), deadline, cancellation, shutdown)
}

#[cfg(windows)]
fn take_tool_pipes(
    child: &mut ToolChild,
) -> (
    Option<tokio::process::ChildStdout>,
    Option<tokio::process::ChildStderr>,
) {
    (child.stdout().take(), child.stderr().take())
}
#[cfg(not(windows))]
fn take_tool_pipes(
    child: &mut ToolChild,
) -> (
    Option<tokio::process::ChildStdout>,
    Option<tokio::process::ChildStderr>,
) {
    (child.stdout.take(), child.stderr.take())
}

#[cfg(unix)]
fn kill_tool_group(child: &ToolChild) {
    // The direct child remains unreaped here, pinning its process-group ID.
    if let Some(pid) = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}
#[cfg(not(unix))]
fn kill_tool_group(_child: &ToolChild) {}

async fn kill_and_reap_tool(child: &mut ToolChild) {
    kill_tool_group(child);
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(all(
    unix,
    not(any(
        target_os = "openbsd",
        target_os = "redox",
        target_os = "horizon",
        target_os = "cygwin"
    ))
))]
async fn wait_tool_exit(child: &mut ToolChild) -> io::Result<std::process::ExitStatus> {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    let raw = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| io::Error::other("tool child ID is unavailable"))?;
    loop {
        match waitid(
            WaitId::Pid(raw),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(Some(_)) => {
                // Kill any remaining ordinary descendants while the unreaped
                // group leader pins the identifier, then collect its status.
                kill_tool_group(child);
                return child.wait().await;
            }
            Ok(None) => tokio::time::sleep(StdDuration::from_millis(10)).await,
            Err(error) => return Err(io::Error::from_raw_os_error(error.raw_os_error())),
        }
    }
}

#[cfg(windows)]
async fn wait_tool_exit(child: &mut ToolChild) -> io::Result<std::process::ExitStatus> {
    loop {
        if let Some(status) = child.inner_mut().try_wait()? {
            // The job handle is still owned, so terminating it cannot target a
            // recycled process identifier. Wait for its completion after kill.
            let _ = child.start_kill();
            child.wait().await?;
            return Ok(status);
        }
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
}

// OpenBSD's kernel rejects registering NOTE_EXIT after a process begins
// exiting. Our child has not been reaped, so ESRCH means the original PID is
// still pinned and its process group can be terminated safely.
#[cfg(any(target_os = "openbsd", all(test, target_os = "macos")))]
async fn wait_tool_exit_kqueue(child: &mut ToolChild) -> io::Result<std::process::ExitStatus> {
    use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
    use nix::sys::time::TimeSpec;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("tool child ID is unavailable"))?;
    let queue = Kqueue::new().map_err(io::Error::from)?;
    rustix::io::fcntl_setfd(&queue, rustix::io::FdFlags::CLOEXEC).map_err(io::Error::from)?;
    let event = KEvent::new(
        pid as usize,
        EventFilter::EVFILT_PROC,
        EvFlags::EV_ADD | EvFlags::EV_ONESHOT,
        FilterFlag::NOTE_EXIT,
        0,
        0,
    );
    let events = [event];
    let zero = *TimeSpec::new(0, 0).as_ref();
    let mut registering = true;
    loop {
        let changes = if registering { &events[..] } else { &[] };
        let mut result = [event];
        match queue.kevent(changes, &mut result, Some(zero)) {
            Err(nix::errno::Errno::ESRCH) => {
                kill_tool_group(child);
                return child.wait().await;
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => return Err(io::Error::from(error)),
            Ok(count) if count > 0 => {
                if result[0].flags().contains(EvFlags::EV_ERROR) {
                    let code = i32::try_from(result[0].data()).unwrap_or(i32::MAX);
                    if code != nix::errno::Errno::ESRCH as i32 {
                        return Err(io::Error::from_raw_os_error(code));
                    }
                }
                kill_tool_group(child);
                return child.wait().await;
            }
            Ok(_) => {
                registering = false;
            }
        }
        // kqueue polling never owns a detached blocking worker; the scoped
        // supervisor can still observe cancellation and its deadline.
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
}

#[cfg(target_os = "openbsd")]
async fn wait_tool_exit(child: &mut ToolChild) -> io::Result<std::process::ExitStatus> {
    wait_tool_exit_kqueue(child).await
}

#[cfg(not(any(
    windows,
    all(
        unix,
        not(any(
            target_os = "openbsd",
            target_os = "redox",
            target_os = "horizon",
            target_os = "cygwin"
        ))
    ),
    target_os = "openbsd"
)))]
async fn wait_tool_exit(child: &mut ToolChild) -> io::Result<std::process::ExitStatus> {
    child.wait().await
}

/// Run and drain one process with a bounded resource slot and no detached readers.
pub(crate) fn run_supervised_tool(
    command: Command,
    timeout: StdDuration,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
    stdout_mode: ToolStdout,
    stderr_limit: Option<usize>,
) -> Result<SupervisedToolOutput, SupervisedToolError> {
    let operation = if command.get_args().any(|arg| arg == "-version") {
        operation_metrics::Operation::ToolVersion
    } else {
        match Path::new(command.get_program())
            .file_stem()
            .and_then(|name| name.to_str())
        {
            Some("ffprobe") => operation_metrics::Operation::Ffprobe,
            Some("ffmpeg") => operation_metrics::Operation::Ffmpeg,
            Some("fpcalc") => operation_metrics::Operation::Fpcalc,
            _ => operation_metrics::Operation::OtherTool,
        }
    };
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(SupervisedToolError::Timeout)?;
    let _slot = acquire_tool_slot(deadline, cancellation, shutdown)?;
    thread::scope(|scope| {
        let supervisor = thread::Builder::new().name("melodee-media-tool".to_owned()).spawn_scoped(scope, move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            runtime.block_on(async move {
                let mut command = tokio::process::Command::from(command);
                command.stdin(Stdio::null()).stdout(match stdout_mode { ToolStdout::None => Stdio::null(), _ => Stdio::piped() })
                    .stderr(if stderr_limit.is_some() { Stdio::piped() } else { Stdio::null() }).kill_on_drop(true);
                #[cfg(unix)]
                command.process_group(0);
                let mut child = spawn_supervised(command, deadline, cancellation, shutdown)?;
                let operation = operation_metrics::OperationGuard::start(operation);
                let (stdout, stderr) = take_tool_pipes(&mut child);
                let reads = async {
                    tokio::try_join!(read_tool_pipe(stdout, stdout_mode),
                        read_tool_pipe(stderr, ToolStdout::Capture(stderr_limit.unwrap_or(0))))
                };
                let output = tokio::select! {
                    result = reads => match result { Ok(value) => value, Err(error) => { kill_and_reap_tool(&mut child).await; return Err(SupervisedToolError::Io(error)); } },
                    error = tool_cancel_or_timeout(deadline, cancellation, shutdown) => { kill_and_reap_tool(&mut child).await; return Err(error); }
                };
                let status = tokio::select! {
                    result = wait_tool_exit(&mut child) => match result { Ok(value) => value, Err(error) => { kill_and_reap_tool(&mut child).await; return Err(SupervisedToolError::Io(error)); } },
                    error = tool_cancel_or_timeout(deadline, cancellation, shutdown) => { kill_and_reap_tool(&mut child).await; return Err(error); }
                };
                if status.success() { operation.finish(); }
                Ok(SupervisedToolOutput { status, stdout: output.0.0, fingerprint: output.0.1, stderr: output.1.0 })
            })
        }).map_err(SupervisedToolError::Io)?;
        supervisor
            .join()
            .map_err(|_| SupervisedToolError::Supervisor)?
    })
}

/// Computes a streaming fingerprint over a canonical decoded audio stream.
/// The output is never buffered in memory. This is the integrity predicate for
/// tag rewrites; a whole-file hash is expected to change when tags change.
pub fn decoded_audio_fingerprint(
    path: &Path,
    ffmpeg_path: &Path,
    timeout: StdDuration,
) -> Result<String, String> {
    let path_text = path
        .to_str()
        .ok_or_else(|| "media path is not UTF-8".to_owned())?;
    let mut command = Command::new(ffmpeg_path);
    command.args([
        "-v",
        "error",
        "-i",
        path_text,
        "-map",
        "0:a:0",
        "-vn",
        "-f",
        "s16le",
        "-ar",
        "48000",
        "-ac",
        "2",
        "-acodec",
        "pcm_s16le",
        "-",
    ]);
    let output = run_supervised_tool(
        command,
        timeout,
        None,
        None,
        ToolStdout::PcmFingerprint,
        None,
    )
    .map_err(|error| format!("audio verification {error}"))?;
    if !output.status.success() {
        return Err("audio verifier rejected the media".to_owned());
    }
    output
        .fingerprint
        .ok_or_else(|| "audio verifier did not expose output".to_owned())
}

/// Probe one media file for the hook context fields. Returns `None` when
/// the file cannot be read or parsed; callers tolerate per-file failures
/// exactly like the legacy provider did (log-and-skip).
#[must_use]
pub fn probe_media_file(path: &Path) -> Option<MediaTagProbe> {
    let mut file = secure_fs::open_read(path).ok()?;
    let tagged = read_from(&mut file).ok()?;
    let duration_ms = u64::try_from(tagged.properties().duration().as_millis()).ok();
    let track_number = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .and_then(|tag| tag.track());
    Some(MediaTagProbe {
        duration_ms,
        track_number,
    })
}

/// Input/resource limits for one inspection. Each worker receives one copy,
/// so the values also bound aggregate memory when worker concurrency is bounded.
#[derive(Debug, Clone)]
pub struct InspectionOptions {
    pub ffprobe_path: PathBuf,
    pub ffprobe_timeout: StdDuration,
    pub max_tool_output_bytes: usize,
    pub max_artwork_bytes: usize,
}

impl Default for InspectionOptions {
    fn default() -> Self {
        Self {
            ffprobe_path: PathBuf::from("ffprobe"),
            ffprobe_timeout: DEFAULT_FFPROBE_TIMEOUT,
            max_tool_output_bytes: DEFAULT_MAX_TOOL_OUTPUT,
            max_artwork_bytes: DEFAULT_MAX_ARTWORK_BYTES,
        }
    }
}

/// Result of inspecting one coherent source tree.
#[derive(Debug)]
pub struct InspectionReport {
    pub manifest: ReleaseManifest,
    pub manifest_hash: String,
    pub structurally_valid: bool,
    pub bytes_hashed: u64,
    pub ffprobe_invocations: u32,
    pub file_count: usize,
}

#[derive(Debug, Error)]
pub enum InspectionError {
    #[error("source directory is missing: {0}")]
    SourceMissing(PathBuf),
    #[error("source directory cannot be inspected: {path}: {message}")]
    SourceUnreadable { path: PathBuf, message: String },
    #[error("manifest serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("ffprobe process failed: {0}")]
    Probe(String),
}

/// Inspect all files below `release_root` without writing to the source.
/// `expected_signature` is the settling signature; callers compare it
/// before and after this operation as well, because a stable source can change
/// during a long inspection.
pub fn inspect_release(
    work_item_id: Uuid,
    source_library_id: Option<Uuid>,
    source_path: &str,
    release_root: &Path,
    source_snapshot_id: Option<i64>,
    source_signature: Option<&str>,
    options: &InspectionOptions,
) -> Result<InspectionReport, InspectionError> {
    if !release_root.is_dir() {
        return Err(InspectionError::SourceMissing(release_root.to_path_buf()));
    }

    let mut paths = Vec::new();
    collect_files(release_root, release_root, &mut paths)?;
    paths.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let audio_paths: Vec<String> = paths
        .iter()
        .filter(|(relative, _)| classify(relative) == ObservedFileKind::Audio)
        .map(|(relative, _)| relative.clone())
        .collect();
    let mut files = Vec::with_capacity(paths.len());
    let mut tracks = Vec::new();
    let mut artwork = Vec::new();
    let mut sidecars = Vec::new();
    let mut findings = Vec::new();
    let mut release_titles = Vec::new();
    let mut release_album_artists = Vec::new();
    let mut release_track_artists = Vec::new();
    let mut bytes_hashed = 0_u64;
    let mut ffprobe_invocations = 0_u32;
    let ffprobe_version = tool_version(&options.ffprobe_path, options.max_tool_output_bytes);

    for (relative_path, path) in paths {
        let mut observed = inspect_file(
            &relative_path,
            &path,
            release_root,
            options,
            &mut bytes_hashed,
            &mut ffprobe_invocations,
        )?;

        // The C# lyric plugin looks for a basename-matched `<audio>.txt` and
        // does not treat arbitrary README/source text as lyrics. Promote only
        // those matching files after the complete directory inventory is
        // known; unmatched `.txt` files remain explicit unknown residue.
        if observed.kind == ObservedFileKind::Unknown
            && observed
                .relative_path
                .to_ascii_lowercase()
                .ends_with(".txt")
            && audio_paths
                .iter()
                .any(|audio| same_stem_and_parent(audio, &observed.relative_path))
        {
            observed.kind = ObservedFileKind::Lyrics;
            observed.sidecar_type = Some("lyrics".to_owned());
            match hash_file(&path) {
                Ok((hash, bytes)) => {
                    observed.content_hash = Some(hash);
                    bytes_hashed = bytes_hashed.saturating_add(bytes);
                }
                Err(error) => {
                    observed.bytes_read_errors = true;
                    observed.findings.push(Finding {
                        code: "file.hash_failed".to_owned(),
                        message: error.to_string(),
                        severity: FindingSeverity::Error,
                        file: Some(observed.relative_path.clone()),
                        evidence: serde_json::json!({}),
                    });
                }
            }
        }

        if let Some(metadata) = &observed.metadata {
            push_unique(&mut release_titles, metadata.album.clone());
            push_unique(&mut release_album_artists, metadata.album_artist.clone());
            push_unique(&mut release_track_artists, metadata.artist.clone());
        }

        if observed.kind == ObservedFileKind::Audio {
            if let Some(metadata) = observed.metadata.clone() {
                tracks.push(ObservedTrack {
                    file_path: observed.relative_path.clone(),
                    track_number: metadata.track_number,
                    track_title: metadata.track_title.clone(),
                    media_number: metadata.disc_number,
                    sort_order: metadata.track_number,
                    byte_hash: observed.content_hash.clone(),
                    duration_ms: observed.audio.as_ref().and_then(|audio| audio.duration_ms),
                    observed_metadata: Some(metadata),
                });
            } else {
                tracks.push(ObservedTrack {
                    file_path: observed.relative_path.clone(),
                    track_number: None,
                    track_title: None,
                    media_number: None,
                    sort_order: None,
                    byte_hash: observed.content_hash.clone(),
                    duration_ms: observed.audio.as_ref().and_then(|audio| audio.duration_ms),
                    observed_metadata: None,
                });
            }
        }

        if observed.kind == ObservedFileKind::Artwork {
            artwork.push(artwork_from_external(&observed));
        }
        artwork.extend(observed.embedded_artwork.clone());
        if is_sidecar(observed.kind.clone()) {
            sidecars.push(observed.clone());
        }
        findings.extend(observed.findings.clone());
        files.push(observed);
    }

    tracks.sort_unstable_by(|left, right| left.file_path.cmp(&right.file_path));
    artwork.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
    sidecars.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
    release_titles.sort_unstable();
    release_album_artists.sort_unstable();
    release_track_artists.sort_unstable();
    let release_artists =
        select_release_artist_observations(&release_album_artists, &release_track_artists);
    let release_artist_source = if release_album_artists.is_empty() {
        "track_artist_fallback"
    } else {
        "album_artist"
    };
    // The album-level date is an audio observation like the album title: it is
    // read from the track tags here so the importer receives it through the
    // sealed manifest. Sidecars only fill it when the audio carries nothing
    // (normalize.rs), keeping tagged audio authoritative over provenance text.
    let release_date = select_release_date(&tracks);

    let audio_count = files
        .iter()
        .filter(|file| file.kind == ObservedFileKind::Audio)
        .count();
    if audio_count == 0 {
        findings.push(Finding {
            code: "release.no_audio".to_owned(),
            message: "release contains no classified audio files".to_owned(),
            severity: FindingSeverity::Error,
            file: None,
            evidence: serde_json::json!({}),
        });
    }

    let mut track_numbers = tracks
        .iter()
        .filter_map(|track| track.track_number)
        .collect::<Vec<_>>();
    track_numbers.sort_unstable();
    let duplicate_track_numbers = track_numbers
        .windows(2)
        .any(|window| window[0] == window[1]);
    if duplicate_track_numbers {
        findings.push(Finding {
            code: "release.duplicate_track_numbers".to_owned(),
            message: "multiple audio files report the same track number".to_owned(),
            severity: FindingSeverity::Warning,
            file: None,
            evidence: serde_json::json!({ "track_numbers": track_numbers }),
        });
    }

    let manifest = ReleaseManifest::new(
        work_item_id,
        source_library_id,
        source_path.to_owned(),
        source_snapshot_id,
        source_signature.map(ToOwned::to_owned),
        ffprobe_version,
        options
            .ffprobe_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        Some(0),
        Some(
            options
                .ffprobe_timeout
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        ),
        true,
        ManifestReleaseObservation {
            title: release_titles.first().cloned(),
            artists: release_artists,
            date: release_date,
            source_labels: serde_json::json!({
                "observed_titles": release_titles,
                "observed_album_artists": release_album_artists,
                "observed_track_artists": release_track_artists,
                "release_artist_source": release_artist_source,
            }),
        },
        files,
        tracks,
        artwork,
        sidecars,
        findings,
    );
    let manifest_hash = hash_manifest(&manifest)?;
    let structurally_valid = !manifest
        .findings
        .iter()
        .any(|finding| finding.severity == FindingSeverity::Error);

    info!(
        work_item_id = %work_item_id,
        file_count = manifest.files.len(),
        audio_count,
        structurally_valid,
        "release inspection completed"
    );

    Ok(InspectionReport {
        file_count: manifest.files.len(),
        manifest,
        manifest_hash,
        structurally_valid,
        bytes_hashed,
        ffprobe_invocations,
    })
}

fn select_release_artist_observations(
    album_artists: &[String],
    track_artists: &[String],
) -> Vec<String> {
    if album_artists.is_empty() {
        track_artists.to_vec()
    } else {
        album_artists.to_vec()
    }
}

/// Hashes a manifest's stable semantic representation with BLAKE3.
///
/// The workspace serializes `serde_json::Value` with `preserve_order`
/// (insertion order) because captured wire contracts depend on it, but the
/// manifest is persisted through PostgreSQL `jsonb`, which reorders object
/// keys. Without canonicalization the same semantic manifest hashes
/// differently before and after a database round trip (observed as
/// `validation.input_semantic_manifest_hash_mismatch` for any manifest whose
/// free-form subdocuments — raw tags, parsed sidecars — have keys whose
/// insertion order differs from jsonb order). Keys are therefore sorted
/// recursively here so the hash depends only on semantic content, never on
/// storage or serialization ordering.
pub fn hash_manifest(manifest: &ReleaseManifest) -> Result<String, InspectionError> {
    let value = manifest.semantic_value()?;
    let canonical = canonicalize_value(&value);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

/// Returns a copy of `value` with every object key sorted recursively, so
/// hashing is independent of map insertion/storage order.
fn canonicalize_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            for key in keys {
                // Keys came from `map`; indexing back into it cannot miss.
                if let Some(entry) = map.get(key) {
                    sorted.insert(key.clone(), canonicalize_value(entry));
                }
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_value).collect())
        }
        scalar => scalar.clone(),
    }
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), InspectionError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| InspectionError::SourceUnreadable {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| InspectionError::SourceUnreadable {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?;
    entries.sort_unstable_by_key(DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| InspectionError::SourceUnreadable {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if metadata.is_dir() {
            collect_files(root, &path, output)?;
        } else if metadata.is_file() {
            let relative =
                path.strip_prefix(root)
                    .map_err(|_| InspectionError::SourceUnreadable {
                        path: path.clone(),
                        message: "path escaped release root".to_owned(),
                    })?;
            output.push((path_to_posix(relative), path));
        } else if metadata.file_type().is_symlink() {
            warn!(relative_path = %path.display(), "ignoring symlink in release inspection");
        }
    }
    Ok(())
}

fn same_stem_and_parent(left: &str, right: &str) -> bool {
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    left_path.parent() == right_path.parent()
        && left_path.file_stem().and_then(|value| value.to_str())
            == right_path.file_stem().and_then(|value| value.to_str())
}

fn inspect_file(
    relative_path: &str,
    path: &Path,
    release_root: &Path,
    options: &InspectionOptions,
    bytes_hashed: &mut u64,
    ffprobe_invocations: &mut u32,
) -> Result<ObservedFile, InspectionError> {
    // Take the initial metadata snapshot from the same no-follow open used by
    // the content readers. A path-only metadata check could observe a regular
    // file and then let a replacement redirect the subsequent readers.
    let file = secure_fs::open_read(path).map_err(|error| InspectionError::SourceUnreadable {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| InspectionError::SourceUnreadable {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    let kind = classify(relative_path);
    let modified_at = metadata.modified().ok().map(|value| {
        OffsetDateTime::from(value)
            .to_offset(UtcOffset::UTC)
            .to_string()
    });
    let mut file = ObservedFile {
        relative_path: relative_path.to_owned(),
        kind: kind.clone(),
        size_bytes: metadata.len(),
        modified_at,
        content_hash: None,
        mime_type: mime_for_path(relative_path),
        kind_code: None,
        metadata: None,
        audio: None,
        findings: Vec::new(),
        embedded_artwork: Vec::new(),
        sidecar_type: sidecar_name(&kind),
        bytes_read_errors: false,
    };

    if kind == ObservedFileKind::Audio {
        // A zero-byte file can never carry media, but ffprobe still answers
        // for some raw demuxers (an empty `.flac` probes as a 0-channel
        // stream). Flag the emptiness explicitly so the failure is
        // actionable at inspection instead of only surfacing downstream.
        if file.size_bytes == 0 {
            file.findings.push(Finding {
                code: "media.empty_file".to_owned(),
                message: "audio file is zero bytes; no media content can be observed".to_owned(),
                severity: FindingSeverity::Error,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({ "size_bytes": 0 }),
            });
        }
        match hash_file(path) {
            Ok((hash, bytes)) => {
                file.content_hash = Some(hash);
                *bytes_hashed = bytes_hashed.saturating_add(bytes);
            }
            Err(error) => {
                file.bytes_read_errors = true;
                file.findings.push(Finding {
                    code: "file.hash_failed".to_owned(),
                    message: error.to_string(),
                    severity: FindingSeverity::Error,
                    file: Some(relative_path.to_owned()),
                    evidence: serde_json::json!({}),
                });
            }
        }

        match read_native_metadata(path, relative_path, options.max_artwork_bytes) {
            Ok((observed, embedded_artwork, native_findings)) => {
                file.metadata = Some(observed);
                file.embedded_artwork = embedded_artwork.clone();
                file.findings.extend(native_findings);
                // Embedded art is represented in the file findings and the
                // release artwork list by a stable pseudo-relative path.
                for (index, embedded) in embedded_artwork.into_iter().enumerate() {
                    file.findings.push(Finding {
                        code: "artwork.embedded_observed".to_owned(),
                        message: format!("embedded artwork observed at index {index}"),
                        severity: FindingSeverity::Info,
                        file: Some(relative_path.to_owned()),
                        evidence: serde_json::json!({
                            "bytes": embedded.size_bytes,
                            "mime_type": embedded.mime_type,
                        }),
                    });
                }
            }
            Err(error) => file.findings.push(Finding {
                code: "metadata.read_failed".to_owned(),
                message: truncate_message(&error),
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({ "reader": "lofty" }),
            }),
        }

        *ffprobe_invocations = ffprobe_invocations.saturating_add(1);
        match run_ffprobe(path, options) {
            Ok(probe) => {
                // media is detected from probed content, not trusted
                // from the filename extension alone. A probed container MIME
                // overrides the extension-derived guess and the disagreement
                // is recorded as an actionable finding.
                if let Some(probed_mime) = probe.mime_type {
                    if let Some(declared) = &file.mime_type
                        && *declared != probed_mime
                    {
                        file.findings.push(Finding {
                            code: "media.extension_mismatch".to_owned(),
                            message: format!(
                                "filename extension suggests {declared} but probed content is {probed_mime}"
                            ),
                            severity: FindingSeverity::Warning,
                            file: Some(relative_path.to_owned()),
                            evidence: serde_json::json!({
                                "extension_mime_type": declared,
                                "probed_mime_type": probed_mime,
                                "container": probe.audio.container,
                                "codec": probe.audio.codec,
                            }),
                        });
                    }
                    file.mime_type = Some(probed_mime);
                }
                file.audio = Some(probe.audio);
            }
            Err(error) => {
                file.findings.push(Finding {
                    code: if error.contains("timed out") {
                        "media.ffprobe_timeout".to_owned()
                    } else {
                        "media.ffprobe_failed".to_owned()
                    },
                    message: truncate_message(&error),
                    severity: FindingSeverity::Error,
                    file: Some(relative_path.to_owned()),
                    evidence: serde_json::json!({ "tool": "ffprobe" }),
                });
            }
        }

        if file.metadata.is_none() {
            file.findings.push(Finding {
                code: "metadata.missing".to_owned(),
                message: "audio file has no readable native metadata".to_owned(),
                severity: FindingSeverity::Info,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({}),
            });
        }
        if file.audio.is_none() {
            file.kind_code = Some("unsupported_or_corrupt_audio".to_owned());
        }
    } else if kind == ObservedFileKind::Artwork {
        let inspect_header = file.size_bytes <= options.max_artwork_bytes as u64;
        let mut header_result = None;
        let hash_result = if inspect_header {
            hash_artwork_and_header(path, options.max_artwork_bytes).map(|(hash, header)| {
                header_result = Some(header);
                hash
            })
        } else {
            hash_file(path).map(|(hash, _)| hash)
        };
        match hash_result {
            Ok(hash) => file.content_hash = Some(hash),
            Err(error) => file.findings.push(Finding {
                code: "artwork.hash_failed".to_owned(),
                message: error.to_string(),
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({}),
            }),
        }
        if file.size_bytes > options.max_artwork_bytes as u64 {
            file.findings.push(Finding {
                code: "artwork.oversized".to_owned(),
                message: "external artwork exceeds the configured observation bound".to_owned(),
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({
                    "size_bytes": file.size_bytes,
                    "max_bytes": options.max_artwork_bytes,
                }),
            });
        } else if let Err(error) =
            header_result.unwrap_or_else(|| inspect_image_header(path, options.max_artwork_bytes))
        {
            file.findings.push(Finding {
                code: "artwork.invalid".to_owned(),
                message: error,
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({}),
            });
        }
    } else if is_sidecar(kind) {
        match hash_file(path) {
            Ok((hash, bytes)) => {
                file.content_hash = Some(hash);
                *bytes_hashed = bytes_hashed.saturating_add(bytes);
            }
            Err(error) => {
                file.bytes_read_errors = true;
                file.findings.push(Finding {
                    code: "file.hash_failed".to_owned(),
                    message: error.to_string(),
                    severity: FindingSeverity::Error,
                    file: Some(relative_path.to_owned()),
                    evidence: serde_json::json!({}),
                });
            }
        }
    }

    let _ = release_root;
    Ok(file)
}

fn classify(relative_path: &str) -> ObservedFileKind {
    let path = Path::new(relative_path);
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if filename == ".melodee-ready" {
        return ObservedFileKind::MelodeeControl;
    }
    if filename.eq_ignore_ascii_case(".blackbeard.provenance.json") {
        return ObservedFileKind::Sidecar;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    if extension
        .as_deref()
        .is_some_and(|value| AUDIO_EXTENSIONS.contains(&value))
    {
        return ObservedFileKind::Audio;
    }
    match extension.as_deref() {
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp") => ObservedFileKind::Artwork,
        Some("cue") => ObservedFileKind::CueSheet,
        Some("m3u" | "m3u8") => ObservedFileKind::Playlist,
        Some("nfo") => ObservedFileKind::Nfo,
        Some("sfv" | "md5" | "sha1" | "sha256") => ObservedFileKind::Checksum,
        Some("lrc") => ObservedFileKind::Lyrics,
        _ => ObservedFileKind::Unknown,
    }
}

fn is_sidecar(kind: ObservedFileKind) -> bool {
    matches!(
        kind,
        ObservedFileKind::CueSheet
            | ObservedFileKind::Playlist
            | ObservedFileKind::Nfo
            | ObservedFileKind::Checksum
            | ObservedFileKind::Lyrics
            | ObservedFileKind::Sidecar
    )
}

fn sidecar_name(kind: &ObservedFileKind) -> Option<String> {
    if is_sidecar(kind.clone()) {
        Some(format!("{kind:?}").to_ascii_lowercase())
    } else {
        None
    }
}

fn mime_for_path(path: &str) -> Option<String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)?;
    let mime = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "aac" => "audio/aac",
        "ac3" => "audio/ac3",
        "wav" => "audio/wav",
        "m4a" | "mp4" => "audio/mp4",
        "wma" => "audio/x-ms-wma",
        _ => return None,
    };
    Some(mime.to_owned())
}

/// MIME type for a probed ffprobe container (`format_name`), restricted to
/// the vocabulary `mime_for_path` already establishes. Content-based
/// detection must not invent new MIME values: containers without an
/// established equivalent (ape, wv,...) yield `None` and leave any
/// extension-derived value in place.
fn mime_for_container(format_name: Option<&str>) -> Option<String> {
    for name in format_name?.split(',') {
        let mime = match name {
            "mp3" => "audio/mpeg",
            "flac" => "audio/flac",
            "ogg" => "audio/ogg",
            "aac" => "audio/aac",
            "ac3" => "audio/ac3",
            "wav" => "audio/wav",
            "asf" => "audio/x-ms-wma",
            "mov" | "mp4" | "m4a" => "audio/mp4",
            _ => continue,
        };
        return Some(mime.to_owned());
    }
    None
}

pub(crate) fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut operation =
        operation_metrics::OperationGuard::start(operation_metrics::Operation::MediaFileHash);
    // Hash through the descriptor-relative no-follow opener. This keeps the
    // inspection/normalization settling hash from following a source symlink
    // introduced after the caller's metadata check.
    let mut file = secure_fs::open_read(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        operation.add_read(u64::try_from(read).unwrap_or(u64::MAX));
        hasher.update(&buffer[..read]);
        bytes = bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    operation.finish();
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

/// Retained MusicBrainz identifier families (TRACK-001, PRD-FP-001..005, ADR-0117/0118).
///
/// Each maps one stable observation key in `ObservedMetadata.raw["musicbrainz_ids"]`
/// to the [`ItemKey`]s that lofty normalizes the format-native tag keys for that
/// family to (ID3 TXXX/TID3, Vorbis comment, MP4 iTun/`----` atoms, APE, etc.).
const MUSICBRAINZ_ID_FAMILIES: [(&str, ItemKey); 6] = [
    ("recording", ItemKey::MusicBrainzRecordingId),
    ("release", ItemKey::MusicBrainzReleaseId),
    ("release_group", ItemKey::MusicBrainzReleaseGroupId),
    ("artist", ItemKey::MusicBrainzArtistId),
    ("release_artist", ItemKey::MusicBrainzReleaseArtistId),
    ("work", ItemKey::MusicBrainzWorkId),
];

/// Extract the six retained MusicBrainz identifier families from a native tag.
///
/// Values are kept as raw observation strings — including malformed ones — because
/// they are evidence for later identity matching, never identity keys themselves
/// (PRD-FP-001..005, ADR-0117/0118; the legacy oracle stored them the same way, as
/// an opaque `MusicBrainzId` string parsed with `SafeParser.ToGuid`). The result
/// is a JSON object containing only the families present in the tag, so files
/// without MusicBrainz IDs produce a `{}` object.
fn musicbrainz_id_observations(tag: &lofty::tag::Tag) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (name, key) in MUSICBRAINZ_ID_FAMILIES {
        if let Some(value) = tag.get_string(key) {
            out.insert(name.to_owned(), serde_json::json!(value));
        }
    }
    serde_json::Value::Object(out)
}

fn read_native_metadata(
    path: &Path,
    relative_path: &str,
    max_artwork_bytes: usize,
) -> Result<(ObservedMetadata, Vec<ObservedArtwork>, Vec<Finding>), String> {
    let mut file = secure_fs::open_read(path).map_err(|error| error.to_string())?;
    let tagged = read_from(&mut file).map_err(|error| error.to_string())?;
    // Prefer the file type's primary tag (MP3 → ID3v2, FLAC → Vorbis Comments,
    // MP4 → MP4 ILST); fall back to the first available tag for file types whose
    // metadata lives in a non-primary tag container.
    let mut tag = tagged.first_tag().cloned();

    // ID3v2.4 stores the MusicBrainz family in TXXX frames keyed by description
    // (e.g. `TXXX:MusicBrainz Album Id`) or a MusicBrainz-owned UFID, which lofty
    // normalizes only when converting to `Id3v2Tag`. The format-agnostic `Tag`
    // view keeps those frames without a canonical key, so for an ID3v2 primary
    // tag we round-trip through `Id3v2Tag` to recover them (TRACK-001). The
    // conversion only touches ID3v2 files, so other formats are unaffected.
    if let Some(current) = tag.take() {
        tag = if current.tag_type() == lofty::tag::TagType::Id3v2 {
            Some(lofty::id3::v2::Id3v2Tag::from(current).into())
        } else {
            // Not an ID3v2 tag — put it back unchanged.
            Some(current)
        };
    }

    let Some(tag) = tag else {
        return Err("no native metadata tag".to_owned());
    };

    let text = |value: Option<std::borrow::Cow<'_, str>>| value.map(|value| value.into_owned());
    let raw = musicbrainz_id_observations(&tag);
    let metadata = ObservedMetadata {
        album: text(tag.album()),
        album_artist: tag.get_string(ItemKey::AlbumArtist).map(ToOwned::to_owned),
        track_title: text(tag.title()),
        artist: text(tag.artist()),
        track_number: tag.track(),
        track_total: tag.track_total(),
        disc_number: tag.disk(),
        disc_total: tag.disk_total(),
        year: None,
        date: tag.date().map(|value| value.to_string()),
        genre: text(tag.genre()),
        raw: serde_json::json!({
            "album": tag.album().as_deref(),
            "album_artist": tag.get_string(ItemKey::AlbumArtist),
            "title": tag.title().as_deref(),
            "artist": tag.artist().as_deref(),
            "track": tag.track(),
            "track_total": tag.track_total(),
            "disc": tag.disk(),
            "disc_total": tag.disk_total(),
            "date": tag.date().map(|value| value.to_string()),
            "genre": tag.genre().as_deref(),
            "musicbrainz_release_type": tag.get_string(ItemKey::MusicBrainzReleaseType),
            "flag_compilation": tag.get_string(ItemKey::FlagCompilation),
            // TRACK-001: retained MusicBrainz identifiers live in `raw` as raw
            // observation strings. They are evidence for identity matching
            // (PRD-FP-001..005, ADR-0117/0118), never identity keys themselves;
            // malformed values are kept verbatim as observations.
            "musicbrainz_ids": raw,
        }),
    };
    let mut artwork = Vec::new();
    let mut findings = Vec::new();
    for (index, picture) in tag.pictures().iter().enumerate() {
        let size = picture.data().len();
        if size > max_artwork_bytes {
            findings.push(Finding {
                code: "artwork.embedded_oversized".to_owned(),
                message: "embedded artwork exceeds the configured observation bound".to_owned(),
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({
                    "index": index,
                    "size_bytes": size,
                    "max_bytes": max_artwork_bytes,
                }),
            });
            continue;
        }
        let (width, height) = image_dimensions(picture.data()).unwrap_or((0, 0));
        artwork.push(ObservedArtwork {
            relative_path: format!("{relative_path}#embedded-{index}"),
            source: "embedded".to_owned(),
            width: Some(width),
            height: Some(height),
            size_bytes: u64::try_from(size).unwrap_or(u64::MAX),
            content_hash: Some(blake3::hash(picture.data()).to_hex().to_string()),
            mime_type: picture.mime_type().map(ToString::to_string),
            is_embedded: true,
            picture_type: Some(format!("{:?}", picture.pic_type())),
        });
    }
    Ok((metadata, artwork, findings))
}

fn artwork_from_external(file: &ObservedFile) -> ObservedArtwork {
    ObservedArtwork {
        relative_path: file.relative_path.clone(),
        source: "external".to_owned(),
        width: None,
        height: None,
        size_bytes: file.size_bytes,
        content_hash: file.content_hash.clone(),
        mime_type: file.mime_type.clone(),
        is_embedded: false,
        picture_type: None,
    }
}

#[derive(Debug)]
pub(crate) struct ProbeResult {
    pub(crate) audio: AudioTechnicalObservation,
    pub(crate) mime_type: Option<String>,
    pub(crate) audio_stream_count: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProbeDocument {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProbeStream {
    index: Option<u32>,
    codec_type: Option<String>,
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    bits_per_sample: Option<u32>,
    bit_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

pub(crate) fn run_ffprobe(path: &Path, options: &InspectionOptions) -> Result<ProbeResult, String> {
    let cancellation = AtomicBool::new(false);
    run_ffprobe_cancellable(path, options, &cancellation, None)
}

/// ffprobe argument vector for probing one media path. The `--` separator
/// guarantees the path is parsed as an operand even if a future caller ever
/// relaxes the absolute-path invariant (defense in depth; all current paths
/// are absolute).
fn ffprobe_args(path: &Path) -> Vec<std::ffi::OsString> {
    [
        std::ffi::OsString::from("-v"),
        std::ffi::OsString::from("error"),
        std::ffi::OsString::from("-print_format"),
        std::ffi::OsString::from("json"),
        std::ffi::OsString::from("-show_format"),
        std::ffi::OsString::from("-show_streams"),
        std::ffi::OsString::from("--"),
        path.as_os_str().to_owned(),
    ]
    .to_vec()
}

pub(crate) fn run_ffprobe_cancellable(
    path: &Path,
    options: &InspectionOptions,
    cancellation: &AtomicBool,
    shutdown: Option<&AtomicBool>,
) -> Result<ProbeResult, String> {
    let mut invocation = ToolInvocation::start("ffprobe", path);
    let mut command = Command::new(&options.ffprobe_path);
    command.args(ffprobe_args(path));
    let output = run_supervised_tool(
        command,
        options.ffprobe_timeout,
        Some(cancellation),
        shutdown,
        ToolStdout::Capture(options.max_tool_output_bytes),
        Some(options.max_tool_output_bytes),
    )
    .map_err(|error| format!("ffprobe {error}"))?;
    let status = output.status;
    let stdout = output.stdout;
    let stderr = output.stderr;
    if !status.success() {
        invocation.set_outcome("exit_failure");
        return Err(format!(
            "ffprobe exited with {}: {}",
            status,
            truncate_bytes(&stderr)
        ));
    }
    invocation.set_outcome("ok");
    let document = serde_json::from_slice::<ProbeDocument>(&stdout)
        .map_err(|error| format!("invalid ffprobe JSON: {error}"))?;
    let stream = document
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"))
        .ok_or_else(|| "ffprobe reported no audio stream".to_owned())?;
    let duration = stream
        .duration
        .as_deref()
        .or(document.format.duration.as_deref())
        .and_then(parse_milliseconds);
    let bitrate = stream
        .bit_rate
        .as_deref()
        .or(document.format.bit_rate.as_deref())
        .and_then(|value| value.parse::<u64>().ok());
    let audio_stream_count = document
        .streams
        .iter()
        .filter(|candidate| candidate.codec_type.as_deref() == Some("audio"))
        .count();
    let mime_type = mime_for_container(document.format.format_name.as_deref());
    Ok(ProbeResult {
        audio: AudioTechnicalObservation {
            container: document.format.format_name,
            codec: stream.codec_name.clone(),
            sample_rate_hz: stream.sample_rate.as_deref().and_then(|v| v.parse().ok()),
            channels: stream.channels,
            channel_layout: stream.channel_layout.clone(),
            bit_depth: stream.bits_per_sample,
            bitrate_bps: bitrate,
            stream_count: u32::try_from(document.streams.len()).ok(),
            duration_ms: duration,
            stream_index: stream.index,
        },
        mime_type,
        audio_stream_count: u32::try_from(audio_stream_count).unwrap_or(u32::MAX),
    })
}

/// Version probes are serialized by bounded executable identity. Results are
/// not cached: a path-based subprocess can observe an A→B→A replacement that
/// before/after metadata checks cannot detect.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ToolVersionKey {
    path: std::path::PathBuf,
    invocation: std::path::PathBuf,
    size: u64,
    modified: Option<StdDuration>,
    #[cfg(unix)]
    unix_identity: (u64, u64, i64, i64),
    max_output: usize,
}

const MAX_CACHED_TOOL_VERSIONS: usize = 16;

#[derive(Default)]
struct ToolVersionState {
    pending: Vec<ToolVersionKey>,
}

#[derive(Default)]
struct ToolVersionCache {
    state: std::sync::Mutex<ToolVersionState>,
    ready: std::sync::Condvar,
}

static TOOL_VERSION_CACHE: std::sync::OnceLock<ToolVersionCache> = std::sync::OnceLock::new();

/// Always release an identity's in-flight slot, including on unwind.
struct ToolVersionFlight {
    cache: &'static ToolVersionCache,
    key: ToolVersionKey,
}

impl Drop for ToolVersionFlight {
    fn drop(&mut self) {
        let mut state = self
            .cache
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(index) = state.pending.iter().position(|key| key == &self.key) {
            state.pending.swap_remove(index);
        }
        drop(state);
        self.cache.ready.notify_all();
    }
}

pub(crate) fn tool_version(path: &Path, max_output: usize) -> Option<String> {
    tool_version_with_cancellation(path, max_output, None, None)
}

pub(crate) fn tool_version_with_cancellation(
    path: &Path,
    max_output: usize,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
) -> Option<String> {
    let search_path = std::env::var_os("PATH");
    tool_version_with_search_path(
        path,
        max_output,
        cancellation,
        shutdown,
        search_path.as_deref(),
    )
}

fn tool_version_with_search_path(
    path: &Path,
    max_output: usize,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
    search_path: Option<&std::ffi::OsStr>,
) -> Option<String> {
    tool_version_with_probe_hook(path, max_output, cancellation, shutdown, search_path, || {})
}

fn tool_version_with_probe_hook(
    path: &Path,
    max_output: usize,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
    search_path: Option<&std::ffi::OsStr>,
    before_probe: impl FnOnce(),
) -> Option<String> {
    if tool_canceled(cancellation, shutdown) {
        return None;
    }
    // Windows' executable search also includes system/application directories;
    // Android does not expose effective-ID faccessat through rustix. In both
    // cases use the platform search for bare names without caching a guessed
    // identity for a potentially different file.
    #[cfg(any(windows, target_os = "android"))]
    if is_bare_tool_name(path) {
        return probe_tool_version(path, max_output, cancellation, shutdown);
    }
    let cache = TOOL_VERSION_CACHE.get_or_init(ToolVersionCache::default);
    // Bound waiting as well as the child process runtime. Waiters poll the
    // cancellation flags while another thread probes the same identity.
    let deadline = Instant::now().checked_add(StdDuration::from_secs(2))?;
    let (invocation, key) = loop {
        if tool_canceled(cancellation, shutdown) {
            return None;
        }
        let invocation = resolve_tool_path(path, search_path)?;
        let key = tool_version_key(&invocation, max_output)?;
        let mut state = cache
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        if !state.pending.contains(&key) && state.pending.len() < MAX_CACHED_TOOL_VERSIONS {
            state.pending.push(key.clone());
            break (invocation, key);
        }
        let (_state, _) = cache
            .ready
            .wait_timeout(state, remaining.min(StdDuration::from_millis(25)))
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    };
    let flight = ToolVersionFlight {
        cache,
        key: key.clone(),
    };
    before_probe();
    let value = probe_tool_version(&invocation, max_output, cancellation, shutdown);
    let current = resolve_tool_path(path, search_path)
        .and_then(|resolved| tool_version_key(&resolved, max_output));
    if tool_canceled(cancellation, shutdown) || current.as_ref() != Some(&key) {
        return None;
    }
    drop(flight);
    value
}

fn is_bare_tool_name(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| path.as_os_str() == name)
}

fn resolve_tool_path(path: &Path, search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let invocation = if is_bare_tool_name(path) {
        std::env::split_paths(search_path?)
            .map(|directory| directory.join(path))
            .find(|candidate| is_executable_file(candidate))?
    } else {
        path.to_path_buf()
    };
    if invocation.is_absolute() {
        Some(invocation)
    } else {
        Some(std::env::current_dir().ok()?.join(invocation))
    }
}

fn is_executable_file(path: &Path) -> bool {
    if !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    #[cfg(all(unix, not(target_os = "android")))]
    {
        rustix::fs::accessat(
            rustix::fs::CWD,
            path,
            rustix::fs::Access::EXEC_OK,
            rustix::fs::AtFlags::EACCESS,
        )
        .is_ok()
    }
    #[cfg(any(not(unix), target_os = "android"))]
    {
        true
    }
}

fn tool_version_key(invocation: &Path, max_output: usize) -> Option<ToolVersionKey> {
    let metadata = std::fs::metadata(invocation).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    let unix_identity = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    Some(ToolVersionKey {
        path: invocation.canonicalize().ok()?,
        invocation: invocation.to_path_buf(),
        size: metadata.len(),
        modified: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok()),
        #[cfg(unix)]
        unix_identity,
        max_output,
    })
}

fn probe_tool_version(
    path: &Path,
    max_output: usize,
    cancellation: Option<&AtomicBool>,
    shutdown: Option<&AtomicBool>,
) -> Option<String> {
    let mut command = Command::new(path);
    command.arg("-version");
    let output = run_supervised_tool(
        command,
        StdDuration::from_secs(2),
        cancellation,
        shutdown,
        ToolStdout::Capture(max_output),
        Some(max_output),
    )
    .ok()?;
    let stdout = output.stdout;
    let status = output.status;
    if !status.success() {
        return None;
    }
    let version_output = truncate_bytes(&stdout);
    let first_line = version_output.lines().next()?.trim();
    if first_line.is_empty() {
        None
    } else {
        Some(first_line.to_owned())
    }
}

pub(crate) fn read_limited<R: Read>(mut reader: R, limit: usize) -> io::Result<Vec<u8>> {
    let mut operation =
        operation_metrics::OperationGuard::start(operation_metrics::Operation::MediaBoundedRead);
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            operation.finish();
            return Ok(output);
        }
        operation.add_read(u64::try_from(read).unwrap_or(u64::MAX));
        if output.len() < limit {
            let remaining = limit - output.len();
            output.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
}

pub(crate) fn parse_milliseconds(value: &str) -> Option<u64> {
    let seconds = value.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let millis = (seconds * 1000.0).round();
    if millis > u64::MAX as f64 {
        return None;
    }
    Some(millis as u64)
}

fn inspect_image_header(path: &Path, max_bytes: usize) -> Result<(), String> {
    let file = secure_fs::open_read(path).map_err(|error| error.to_string())?;
    let bytes = read_limited(file, max_bytes).map_err(|error| error.to_string())?;
    image_dimensions(&bytes)
        .map(|_| ())
        .ok_or_else(|| "unsupported or malformed image header".to_owned())
}

/// Hash all bytes and inspect a bounded prefix from the same opened stream.
/// The full hash is required even if the source grows past the header limit.
fn hash_artwork_and_header(
    path: &Path,
    max_bytes: usize,
) -> io::Result<(String, Result<(), String>)> {
    let mut operation =
        operation_metrics::OperationGuard::start(operation_metrics::Operation::MediaFileHash);
    let mut file = secure_fs::open_read(path)?;
    let mut hasher = Hasher::new();
    let mut header = Vec::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        operation.add_read(u64::try_from(count).unwrap_or(u64::MAX));
        hasher.update(&buffer[..count]);
        let take = count.min(max_bytes.saturating_sub(header.len()));
        header.extend_from_slice(&buffer[..take]);
    }
    let result = image_dimensions(&header)
        .map(|_| ())
        .ok_or_else(|| "unsupported or malformed image header".to_owned());
    operation.finish();
    Ok((hasher.finalize().to_hex().to_string(), result))
}

pub(crate) fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((width, height));
    }
    if bytes.starts_with(b"GIF8") && bytes.len() >= 10 {
        let width = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
        let height = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
        return Some((width, height));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        return jpeg_dimensions(bytes);
    }
    None
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut index = 2_usize;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        index += 2;
        if marker == 0xd8 || marker == 0xd9 {
            continue;
        }
        let length = usize::from(u16::from_be_bytes(
            bytes.get(index..index + 2)?.try_into().ok()?,
        ));
        if length < 2 || index + length > bytes.len() {
            return None;
        }
        if (0xc0..=0xc3).contains(&marker)
            || (0xc5..=0xc7).contains(&marker)
            || (0xc9..=0xcb).contains(&marker)
            || (0xcd..=0xcf).contains(&marker)
        {
            let height = u32::from(u16::from_be_bytes(
                bytes.get(index + 3..index + 5)?.try_into().ok()?,
            ));
            let width = u32::from(u16::from_be_bytes(
                bytes.get(index + 5..index + 7)?.try_into().ok()?,
            ));
            return Some((width, height));
        }
        index += length;
    }
    None
}

/// Selects the album-level date from the observed track tags.
///
/// The value carried by the most audio tracks wins, because one mistagged
/// track must not override the rest of the release. `BTreeMap` iteration is
/// lexicographic and ties are broken towards the smallest observed value, so
/// the choice is a pure function of the files' contents rather than of
/// directory enumeration order; identical corpora therefore always seal the
/// same manifest. Values are passed through verbatim: validating or
/// normalizing them is the importer's concern (`parse_observed_date`).
fn select_release_date(tracks: &[ObservedTrack]) -> Option<String> {
    let mut observed: BTreeMap<&str, usize> = BTreeMap::new();
    for track in tracks {
        let Some(date) = track
            .observed_metadata
            .as_ref()
            .and_then(|metadata| metadata.date.as_deref())
            .map(str::trim)
            .filter(|date| !date.is_empty())
        else {
            continue;
        };
        *observed.entry(date).or_default() += 1;
    }
    observed
        .into_iter()
        .max_by_key(|(date, count)| (*count, Reverse(*date)))
        .map(|(date, _)| date.to_owned())
}

fn push_unique(values: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty())
        && !values.iter().any(|existing| existing == &value)
    {
        values.push(value);
    }
}

pub(crate) fn path_to_posix(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn truncate_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).into_owned()
}

fn truncate_message(message: &str) -> String {
    if message.len() > 512 {
        format!("{}…", &message[..512])
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use lofty::{
        config::WriteOptions,
        tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType},
    };

    // Runs only when invoked as an isolated test subprocess by the tests below.
    // It uses the same executable on Unix and Windows, avoiding shell-specific
    // child-tree or pipe behavior in the fixture.
    #[test]
    // This deliberately models a parent that exits without waiting for a descendant.
    #[allow(clippy::zombie_processes)]
    fn supervised_child_fixture() {
        use std::io::Write;
        let Ok(mode) = std::env::var("MELODEE_MEDIA_TEST_CHILD_MODE") else {
            return;
        };
        match mode.as_str() {
            "parent" => {
                let mut child = Command::new(std::env::current_exe().unwrap());
                child
                    .args(["--exact", "tests::supervised_child_fixture", "--nocapture"])
                    .env("MELODEE_MEDIA_TEST_CHILD_MODE", "descendant")
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
                let _child = child.spawn().unwrap();
                let ready = PathBuf::from(std::env::var_os("MELODEE_MEDIA_TEST_READY").unwrap());
                let deadline = Instant::now() + StdDuration::from_secs(2);
                while !ready.exists() && Instant::now() < deadline {
                    thread::sleep(StdDuration::from_millis(5));
                }
                assert!(ready.exists());
            }
            "parent_closed" => {
                let mut child = Command::new(std::env::current_exe().unwrap());
                child
                    .args(["--exact", "tests::supervised_child_fixture", "--nocapture"])
                    .env("MELODEE_MEDIA_TEST_CHILD_MODE", "descendant")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                let _child = child.spawn().unwrap();
                let ready = PathBuf::from(std::env::var_os("MELODEE_MEDIA_TEST_READY").unwrap());
                let deadline = Instant::now() + StdDuration::from_secs(2);
                while !ready.exists() && Instant::now() < deadline {
                    thread::sleep(StdDuration::from_millis(5));
                }
                assert!(ready.exists());
            }
            "descendant" => {
                fs::write(
                    std::env::var_os("MELODEE_MEDIA_TEST_READY").unwrap(),
                    b"ready",
                )
                .unwrap();
                thread::sleep(StdDuration::from_millis(1500));
                fs::write(
                    std::env::var_os("MELODEE_MEDIA_TEST_MARKER").unwrap(),
                    b"escaped",
                )
                .unwrap();
            }
            "flood" => {
                let payload = [b'x'; 8192];
                for _ in 0..32 {
                    std::io::stdout().write_all(&payload).unwrap();
                    std::io::stderr().write_all(&payload).unwrap();
                }
            }
            "sleep" => thread::sleep(StdDuration::from_secs(10)),
            "delayed_exit" => thread::sleep(StdDuration::from_millis(100)),
            _ => panic!("unknown fixture mode"),
        }
    }

    fn supervised_fixture_command(mode: &str, ready: &Path, marker: &Path) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::supervised_child_fixture", "--nocapture"])
            .env("MELODEE_MEDIA_TEST_CHILD_MODE", mode)
            .env("MELODEE_MEDIA_TEST_READY", ready)
            .env("MELODEE_MEDIA_TEST_MARKER", marker);
        command
    }

    #[test]
    fn supervised_tool_drains_both_pipes_and_bounds_captured_output() {
        let temp = temp_dir("supervised-flood");
        let output = run_supervised_tool(
            supervised_fixture_command("flood", &temp.join("ready"), &temp.join("marker")),
            StdDuration::from_secs(5),
            None,
            None,
            ToolStdout::Capture(37),
            Some(53),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 37);
        assert_eq!(output.stderr.len(), 53);
        assert_eq!(*TOOL_ADMISSION.get().unwrap().0.lock().unwrap(), 0);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn inherited_pipes_timeout_terminates_tree_and_releases_admission() {
        let temp = temp_dir("supervised-inherited-pipe");
        let ready = temp.join("ready");
        let marker = temp.join("marker");
        let start = Instant::now();
        let result = run_supervised_tool(
            supervised_fixture_command("parent", &ready, &marker),
            StdDuration::from_millis(700),
            None,
            None,
            ToolStdout::Capture(128),
            Some(128),
        );
        assert!(matches!(result, Err(SupervisedToolError::Timeout)));
        assert!(
            ready.exists(),
            "descendant was active and inherited the pipes"
        );
        assert!(start.elapsed() < StdDuration::from_secs(3));
        assert_eq!(*TOOL_ADMISSION.get().unwrap().0.lock().unwrap(), 0);
        thread::sleep(StdDuration::from_millis(1600));
        assert!(
            !marker.exists(),
            "descendant must not keep working after timeout"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn cancellation_terminates_tree_and_releases_admission() {
        let temp = temp_dir("supervised-cancel");
        let ready = temp.join("ready");
        let marker = temp.join("marker");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let signal_path = ready.clone();
        let canceler = thread::spawn(move || {
            let deadline = Instant::now() + StdDuration::from_secs(3);
            while !signal_path.exists() && Instant::now() < deadline {
                thread::sleep(StdDuration::from_millis(5));
            }
            flag.store(true, Ordering::SeqCst);
        });
        let result = run_supervised_tool(
            supervised_fixture_command("parent", &ready, &marker),
            StdDuration::from_secs(5),
            Some(&cancel),
            None,
            ToolStdout::Capture(128),
            Some(128),
        );
        canceler.join().unwrap();
        assert!(matches!(result, Err(SupervisedToolError::Canceled)));
        assert!(ready.exists());
        assert_eq!(*TOOL_ADMISSION.get().unwrap().0.lock().unwrap(), 0);
        thread::sleep(StdDuration::from_millis(1600));
        assert!(
            !marker.exists(),
            "descendant must not keep working after cancellation"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn normal_exit_terminates_descendant_that_closed_its_pipes() {
        let temp = temp_dir("supervised-closed-pipes");
        let ready = temp.join("ready");
        let marker = temp.join("marker");
        let output = run_supervised_tool(
            supervised_fixture_command("parent_closed", &ready, &marker),
            StdDuration::from_secs(4),
            None,
            None,
            ToolStdout::Capture(128),
            Some(128),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(ready.exists());
        thread::sleep(StdDuration::from_millis(1600));
        assert!(
            !marker.exists(),
            "a successful parent exit must not leave descendants running"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn supervisor_admits_before_spawning_and_times_out_when_full() {
        let deadline = Instant::now() + StdDuration::from_secs(5);
        let slots: Vec<_> = (0..MAX_SUPERVISED_TOOLS)
            .map(|_| acquire_tool_slot(deadline, None, None).unwrap())
            .collect();
        let temp = temp_dir("supervised-admission");
        let result = run_supervised_tool(
            supervised_fixture_command("parent", &temp.join("ready"), &temp.join("marker")),
            StdDuration::from_millis(75),
            None,
            None,
            ToolStdout::None,
            None,
        );
        assert!(matches!(result, Err(SupervisedToolError::Timeout)));
        assert!(
            !temp.join("ready").exists(),
            "no child is spawned before admission"
        );
        drop(slots);
        assert_eq!(*TOOL_ADMISSION.get().unwrap().0.lock().unwrap(), 0);
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(any(target_os = "openbsd", target_os = "macos"))]
    #[test]
    fn kqueue_observer_receives_exit_after_registration() {
        let temp = temp_dir("supervised-kqueue-delayed");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut command = tokio::process::Command::from(supervised_fixture_command(
                "delayed_exit",
                &temp.join("ready"),
                &temp.join("marker"),
            ));
            command
                .process_group(0)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child = command.spawn().unwrap();
            let status =
                tokio::time::timeout(StdDuration::from_secs(3), wait_tool_exit_kqueue(&mut child))
                    .await
                    .unwrap()
                    .unwrap();
            assert!(status.success());
        });
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(any(target_os = "openbsd", target_os = "macos"))]
    #[test]
    fn kqueue_observer_handles_a_child_that_already_exited() {
        let temp = temp_dir("supervised-kqueue-exited");
        let ready = temp.join("ready");
        let marker = temp.join("marker");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut command = tokio::process::Command::from(supervised_fixture_command(
                "parent_closed",
                &ready,
                &marker,
            ));
            command
                .process_group(0)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child = command.spawn().unwrap();
            tokio::time::sleep(StdDuration::from_millis(250)).await;
            let status = wait_tool_exit_kqueue(&mut child).await.unwrap();
            assert!(status.success());
        });
        assert!(ready.exists());
        thread::sleep(StdDuration::from_millis(1600));
        assert!(
            !marker.exists(),
            "the already-exited leader must still fence its descendants"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn executable_busy_retry_checks_cancellation_and_deadline_before_each_attempt() {
        let cancel = AtomicBool::new(false);
        let mut attempts = 0;
        let result = spawn_with_budget::<()>(
            || {
                attempts += 1;
                cancel.store(true, Ordering::SeqCst);
                Err(io::Error::from_raw_os_error(26))
            },
            Instant::now() + StdDuration::from_secs(1),
            Some(&cancel),
            None,
        );
        assert!(matches!(result, Err(SupervisedToolError::Canceled)));
        assert_eq!(attempts, 1);
        let mut attempts = 0;
        let result = spawn_with_budget::<()>(
            || {
                attempts += 1;
                Err(io::Error::from_raw_os_error(26))
            },
            Instant::now() + StdDuration::from_millis(1),
            None,
            None,
        );
        assert!(matches!(result, Err(SupervisedToolError::Timeout)));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn deadline_also_covers_process_wait_with_no_pipes() {
        let temp = temp_dir("supervised-wait");
        let start = Instant::now();
        let result = run_supervised_tool(
            supervised_fixture_command("sleep", &temp.join("ready"), &temp.join("marker")),
            StdDuration::from_millis(150),
            None,
            None,
            ToolStdout::None,
            None,
        );
        assert!(matches!(result, Err(SupervisedToolError::Timeout)));
        assert!(start.elapsed() < StdDuration::from_secs(2));
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn ffprobe_argv_separates_options_from_the_media_path() {
        let args = ffprobe_args(Path::new("relative/-leading-dash.flac"));
        let separator = args.iter().position(|arg| arg == "--").expect("separator");
        assert_eq!(
            separator,
            args.len() - 2,
            "the path must be the sole operand after `--`: {args:?}"
        );
        assert_eq!(
            args.last().expect("path operand"),
            "relative/-leading-dash.flac"
        );
    }
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("melodee-media-{name}-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).expect("temp directory");
        path
    }

    #[cfg(unix)]
    #[test]
    fn tool_version_resolves_path_order_and_invalidates_replaced_executables() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("tool-version-path");
        let denied = root.join("denied");
        let first = root.join("first");
        let second = root.join("second");
        for directory in [&denied, &first, &second] {
            fs::create_dir(directory).unwrap();
        }
        fs::write(denied.join("tool"), b"not executable").unwrap();
        let marker = root.join("calls");
        let write_tool = |path: &Path, value: &str| {
            fs::write(
                path,
                format!(
                    "#!/bin/sh\nprintf x >> '{}'\nprintf '{value}'\n",
                    marker.display()
                ),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        };
        write_tool(&first.join("tool"), "alpha");
        write_tool(&second.join("tool"), "bravo");
        let paths = std::env::join_paths([&denied, &first, &second]).unwrap();
        let lookup = |path: &std::ffi::OsStr| {
            tool_version_with_search_path(Path::new("tool"), 1024, None, None, Some(path))
        };
        assert_eq!(lookup(&paths).as_deref(), Some("alpha"));
        assert_eq!(lookup(&paths).as_deref(), Some("alpha"));
        assert_eq!(
            fs::read(&marker).unwrap(),
            b"xx",
            "unbound executable results must not be reused"
        );
        let reordered = std::env::join_paths([&second, &first]).unwrap();
        assert_eq!(lookup(&reordered).as_deref(), Some("bravo"));

        let original = fs::metadata(first.join("tool")).unwrap();
        let replacement = first.join("replacement");
        write_tool(&replacement, "gamma");
        assert_eq!(fs::metadata(&replacement).unwrap().len(), original.len());
        fs::File::open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(original.modified().unwrap()))
            .unwrap();
        fs::rename(&replacement, first.join("tool")).unwrap();
        assert_eq!(
            fs::metadata(first.join("tool"))
                .unwrap()
                .modified()
                .unwrap(),
            original.modified().unwrap()
        );
        assert_eq!(lookup(&paths).as_deref(), Some("gamma"));
        assert_eq!(fs::read(&marker).unwrap(), b"xxxx");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_tool_version_probe_is_not_cached() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("tool-version-retry");
        let ready = root.join("ready");
        let tool = root.join("tool");
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\nif [ -f '{}' ]; then printf ready; else exit 1; fi\n",
                ready.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(tool_version(&tool, 1024).is_none());
        fs::write(&ready, b"ready").unwrap();
        assert_eq!(tool_version(&tool, 1024).as_deref(), Some("ready"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn executable_replaced_during_version_probe_is_not_reported_or_cached() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("tool-version-race");
        let tool = root.join("tool");
        let ready = root.join("ready");
        let release = root.join("release");
        fs::write(&tool, format!(
            "#!/bin/sh\nprintf ready > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf old\n",
            ready.display(), release.display()
        )).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let task_tool = tool.clone();
        let task = thread::spawn(move || tool_version(&task_tool, 1024));
        let deadline = Instant::now() + StdDuration::from_secs(1);
        while !ready.exists() && Instant::now() < deadline {
            thread::sleep(StdDuration::from_millis(5));
        }
        assert!(ready.exists(), "version probe did not start");
        let replacement = root.join("replacement");
        fs::write(&replacement, "#!/bin/sh\nprintf new\n").unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(&replacement, &tool).unwrap();
        fs::write(&release, b"go").unwrap();
        assert!(task.join().unwrap().is_none());
        assert_eq!(tool_version(&tool, 1024).as_deref(), Some("new"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn transient_aba_executable_swap_cannot_poison_later_version() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = temp_dir("tool-version-aba");
        let tool = root.join("tool");
        let a = root.join("a");
        let b = root.join("b");
        let ready = root.join("ready");
        let release = root.join("release");
        fs::write(&a, "#!/bin/sh\nprintf alpha\n").unwrap();
        fs::write(&b, format!(
            "#!/bin/sh\nprintf ready > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf bravo\n",
            ready.display(), release.display()
        )).unwrap();
        for path in [&a, &b] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        symlink(&a, &tool).unwrap();
        let probe_tool = tool.clone();
        let switch_tool = tool.clone();
        let b_for_hook = b.clone();
        let probe = thread::spawn(move || {
            tool_version_with_probe_hook(&probe_tool, 1024, None, None, None, || {
                fs::remove_file(&switch_tool).unwrap();
                symlink(&b_for_hook, &switch_tool).unwrap();
            })
        });
        let deadline = Instant::now() + StdDuration::from_secs(1);
        while !ready.exists() && Instant::now() < deadline {
            thread::sleep(StdDuration::from_millis(5));
        }
        assert!(ready.exists(), "transient B executable did not start");
        fs::remove_file(&tool).unwrap();
        symlink(&a, &tool).unwrap();
        fs::write(&release, b"go").unwrap();
        assert_eq!(probe.join().unwrap().as_deref(), Some("bravo"));
        // The unbound B observation is never retained as A's future result.
        assert_eq!(tool_version(&tool, 1024).as_deref(), Some("alpha"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_unbound_version_requests_are_serialized_but_not_reused() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Barrier};

        let root = temp_dir("tool-version-singleflight");
        let tool = root.join("tool");
        let calls = root.join("calls");
        let release = root.join("release");
        fs::write(&tool, format!(
            "#!/bin/sh\nprintf x >> '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf version\n",
            calls.display(), release.display()
        )).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let barrier = Arc::new(Barrier::new(9));
        let tasks = (0..8)
            .map(|_| {
                let tool = tool.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    tool_version(&tool, 1024)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let deadline = Instant::now() + StdDuration::from_secs(1);
        while !calls.exists() && Instant::now() < deadline {
            thread::sleep(StdDuration::from_millis(5));
        }
        assert!(calls.exists(), "version probe did not start");
        thread::sleep(StdDuration::from_millis(100));
        assert_eq!(
            fs::read(&calls).unwrap(),
            b"x",
            "only one probe may be in flight for an identity"
        );
        fs::write(&release, b"go").unwrap();
        for task in tasks {
            assert_eq!(task.join().unwrap().as_deref(), Some("version"));
        }
        assert_eq!(fs::read(&calls).unwrap(), b"xxxxxxxx");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn canceled_version_waiter_does_not_cancel_or_duplicate_leader() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;

        let root = temp_dir("tool-version-waiter-cancel");
        let tool = root.join("tool");
        let calls = root.join("calls");
        let release = root.join("release");
        fs::write(&tool, format!(
            "#!/bin/sh\nprintf x >> '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf version\n",
            calls.display(), release.display()
        )).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let leader_tool = tool.clone();
        let leader = thread::spawn(move || tool_version(&leader_tool, 1024));
        let deadline = Instant::now() + StdDuration::from_secs(1);
        while !calls.exists() && Instant::now() < deadline {
            thread::sleep(StdDuration::from_millis(5));
        }
        assert!(calls.exists(), "version leader did not start");
        let cancel = Arc::new(AtomicBool::new(false));
        let waiter_cancel = cancel.clone();
        let waiter_tool = tool.clone();
        let waiter = thread::spawn(move || {
            tool_version_with_cancellation(&waiter_tool, 1024, Some(&waiter_cancel), None)
        });
        thread::sleep(StdDuration::from_millis(75));
        let started = Instant::now();
        cancel.store(true, Ordering::SeqCst);
        assert!(waiter.join().unwrap().is_none());
        assert!(started.elapsed() < StdDuration::from_millis(500));
        fs::write(&release, b"go").unwrap();
        assert_eq!(leader.join().unwrap().as_deref(), Some("version"));
        assert_eq!(fs::read(&calls).unwrap(), b"x");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn canceled_version_leader_releases_waiter_to_retry() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;

        let root = temp_dir("tool-version-leader-cancel");
        let tool = root.join("tool");
        let calls = root.join("calls");
        let release = root.join("release");
        fs::write(&tool, format!(
            "#!/bin/sh\nprintf x >> '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf version\n",
            calls.display(), release.display()
        )).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let leader_cancel = cancel.clone();
        let leader_tool = tool.clone();
        let leader = thread::spawn(move || {
            tool_version_with_cancellation(&leader_tool, 1024, Some(&leader_cancel), None)
        });
        let deadline = Instant::now() + StdDuration::from_secs(1);
        while !calls.exists() && Instant::now() < deadline {
            thread::sleep(StdDuration::from_millis(5));
        }
        assert!(calls.exists(), "version leader did not start");
        let waiter_tool = tool.clone();
        let waiter = thread::spawn(move || tool_version(&waiter_tool, 1024));
        thread::sleep(StdDuration::from_millis(75));
        cancel.store(true, Ordering::SeqCst);
        assert!(leader.join().unwrap().is_none());
        let deadline = Instant::now() + StdDuration::from_secs(1);
        while fs::read(&calls).unwrap().len() < 2 && Instant::now() < deadline {
            thread::sleep(StdDuration::from_millis(5));
        }
        assert_eq!(fs::read(&calls).unwrap(), b"xx");
        fs::write(&release, b"go").unwrap();
        assert_eq!(waiter.join().unwrap().as_deref(), Some("version"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_artist_observation_prefers_album_artist_over_track_billing() {
        let album_artists = vec!["6arelyhuman".to_owned()];
        let track_artists = vec!["6arelyhuman".to_owned(), "6arelyhuman/Baby Jane".to_owned()];
        assert_eq!(
            select_release_artist_observations(&album_artists, &track_artists),
            album_artists
        );
        assert_eq!(
            select_release_artist_observations(&[], &track_artists),
            track_artists
        );
    }

    #[test]
    fn classification_preserves_control_and_sidecars() {
        assert_eq!(classify(".melodee-ready"), ObservedFileKind::MelodeeControl);
        assert_eq!(classify("disc/cover.JPG"), ObservedFileKind::Artwork);
        assert_eq!(classify("disc/list.m3u"), ObservedFileKind::Playlist);
        assert_eq!(classify("disc/info.nfo"), ObservedFileKind::Nfo);
        assert_eq!(classify("disc/track.flac"), ObservedFileKind::Audio);
        assert_eq!(classify("disc/track.txt"), ObservedFileKind::Unknown);
        assert_eq!(
            classify("disc/.blackbeard.provenance.json"),
            ObservedFileKind::Sidecar
        );
    }

    #[test]
    fn safe_tag_rewrite_changes_container_hash_but_preserves_decoded_audio() {
        let root = temp_dir("tag-rewrite-integrity");
        let original = root.join("track.mp3");
        let rewritten = root.join("track-copy.mp3");
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.4:sample_rate=8000",
                "-metadata",
                "title=Before",
                "-metadata",
                "artist=Original Artist",
                "-metadata",
                "album=Observed Release",
                "-metadata",
                "track=1/1",
                "-c:a",
                "libmp3lame",
            ])
            .arg(&original)
            .output()
            .expect("ffmpeg");
        assert!(output.status.success(), "ffmpeg failed to create fixture");
        fs::copy(&original, &rewritten).expect("copy source");
        let before_hash = hash_file(&rewritten).expect("hash before").0;
        let before_audio =
            decoded_audio_fingerprint(&rewritten, Path::new("ffmpeg"), StdDuration::from_secs(30))
                .expect("decoded audio before");
        let before_snapshot = read_tag_snapshot(&rewritten).expect("snapshot before");
        assert_eq!(before_snapshot.title.as_deref(), Some("Before"));
        let after = apply_tag_rewrite(
            &rewritten,
            &TagRewrite {
                title: Some("After".to_owned()),
                clear_artist: true,
                track_number: Some(2),
                ..TagRewrite::default()
            },
        )
        .expect("rewrite tags");
        let after_hash = hash_file(&rewritten).expect("hash after").0;
        let after_audio =
            decoded_audio_fingerprint(&rewritten, Path::new("ffmpeg"), StdDuration::from_secs(30))
                .expect("decoded audio after");
        assert_ne!(before_hash, after_hash, "tag changes must alter file hash");
        assert_eq!(before_audio, after_audio, "decoded audio must be stable");
        assert_eq!(after.title.as_deref(), Some("After"));
        assert_eq!(after.artist, None);
        assert_eq!(after.track_number, Some(2));
    }

    #[test]
    fn probed_container_maps_to_the_established_mime_vocabulary() {
        let cases = [
            ("mp3", Some("audio/mpeg")),
            ("flac", Some("audio/flac")),
            ("ogg", Some("audio/ogg")),
            ("aac", Some("audio/aac")),
            ("ac3", Some("audio/ac3")),
            ("wav", Some("audio/wav")),
            ("asf", Some("audio/x-ms-wma")),
            ("mov,mp4,m4a,3gp,3g2,mj2", Some("audio/mp4")),
            // Containers without an established equivalent stay unmapped so
            // content detection never invents new MIME values.
            ("ape", None),
            ("wv", None),
        ];
        for (format_name, expected) in cases {
            assert_eq!(
                mime_for_container(Some(format_name)).as_deref(),
                expected,
                "container {format_name}"
            );
        }
        assert_eq!(mime_for_container(None), None);
    }

    #[test]
    fn byte_hash_distinguishes_same_size_content() {
        let root = temp_dir("hash");
        let left = root.join("left.bin");
        let right = root.join("right.bin");
        fs::write(&left, b"abcd").expect("left");
        fs::write(&right, b"abce").expect("right");
        assert_ne!(hash_file(&left).unwrap().0, hash_file(&right).unwrap().0);
    }

    #[test]
    fn artwork_hash_covers_the_full_stream_with_a_bounded_header() {
        let root = temp_dir("artwork-hash");
        let path = root.join("cover.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03".to_vec();
        bytes.extend(std::iter::repeat_n(7, HASH_BUFFER_SIZE + 1));
        fs::write(&path, &bytes).expect("write artwork");
        let (hash, header) = hash_artwork_and_header(&path, 24).expect("read artwork");
        assert_eq!(hash, blake3::hash(&bytes).to_hex().to_string());
        assert!(header.is_ok());
        let (_, short_header) = hash_artwork_and_header(&path, 23).expect("read artwork");
        assert!(short_header.is_err());
        fs::remove_dir_all(root).expect("remove test directory");
    }

    /// large-file rule: media hashing streams through a bounded buffer.
    /// A sparse 256 MiB file (2048x the 128 KiB chunk) hashes identically to a
    /// streamed reference, proving multi-chunk operation without materializing
    /// the file — the same code path bounds arbitrarily large media.
    #[test]
    fn hashing_streams_large_files_in_bounded_chunks() {
        const FILE_LEN: u64 = 256 * 1024 * 1024;
        assert!(
            FILE_LEN >= 2048 * HASH_BUFFER_SIZE as u64,
            "fixture must span many hash chunks"
        );
        let root = temp_dir("streaming-hash");
        let path = root.join("large.bin");
        let file = fs::File::create(&path).expect("create sparse file");
        file.set_len(FILE_LEN).expect("extend sparse file");
        drop(file);

        let (hash, bytes) = hash_file(&path).expect("hash sparse file");
        assert_eq!(bytes, FILE_LEN);

        let zero_chunk = vec![0_u8; HASH_BUFFER_SIZE];
        let chunks = usize::try_from(FILE_LEN).expect("fixture length") / HASH_BUFFER_SIZE;
        let mut reference = Hasher::new();
        for _ in 0..chunks {
            reference.update(&zero_chunk);
        }
        assert_eq!(hash, reference.finalize().to_hex().to_string());
    }

    /// PRD-FP-001/ADR-0145: the album date is an audio observation. It must be
    /// lifted from the track tags, prefer the value most tracks agree on, and
    /// ignore untagged or blank tracks entirely.
    #[test]
    fn release_date_is_the_majority_track_tag_and_is_stable() {
        fn track(path: &str, date: Option<&str>) -> ObservedTrack {
            ObservedTrack {
                file_path: path.to_owned(),
                track_number: None,
                track_title: None,
                media_number: None,
                sort_order: None,
                byte_hash: None,
                duration_ms: None,
                observed_metadata: date.map(|date| ObservedMetadata {
                    album: None,
                    album_artist: None,
                    track_title: None,
                    artist: None,
                    track_number: None,
                    track_total: None,
                    disc_number: None,
                    disc_total: None,
                    year: None,
                    date: Some(date.to_owned()),
                    genre: None,
                    raw: serde_json::json!({}),
                }),
            }
        }

        // The majority wins over a stray mistagged track.
        let majority = vec![
            track("01.mp3", Some("2019-04-12")),
            track("02.mp3", Some("2001-01-01")),
            track("03.mp3", Some("2019-04-12")),
        ];
        assert_eq!(
            select_release_date(&majority).as_deref(),
            Some("2019-04-12")
        );

        // Ties and enumeration order must not change the sealed value.
        let tied = vec![
            track("01.mp3", Some("2020-09-01")),
            track("02.mp3", Some("2019-04-12")),
        ];
        let mut reversed = tied.clone();
        reversed.reverse();
        assert_eq!(select_release_date(&tied).as_deref(), Some("2019-04-12"));
        assert_eq!(select_release_date(&tied), select_release_date(&reversed));

        // Blank and absent dates are not observations.
        let none = vec![track("01.mp3", Some("   ")), track("02.mp3", None)];
        assert_eq!(select_release_date(&none), None);
        assert_eq!(select_release_date(&[]), None);
    }

    #[test]
    fn manifest_hash_excludes_volatile_provenance() {
        let first = ReleaseManifest::new(
            Uuid::now_v7(),
            None,
            "alpha".to_owned(),
            Some(1),
            Some("snapshot-a".to_owned()),
            Some("ffprobe".to_owned()),
            Some("/usr/bin/ffprobe".to_owned()),
            Some(0),
            Some(30_000),
            true,
            ManifestReleaseObservation {
                title: Some("Title".to_owned()),
                artists: vec!["Artist".to_owned()],
                date: None,
                source_labels: serde_json::json!({}),
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let second = ReleaseManifest::new(
            Uuid::now_v7(),
            None,
            "other".to_owned(),
            Some(2),
            Some("snapshot-b".to_owned()),
            Some("other".to_owned()),
            Some("other".to_owned()),
            Some(0),
            Some(1),
            true,
            first.release_observation.clone(),
            first.files.clone(),
            first.tracks.clone(),
            first.artwork.clone(),
            first.sidecars.clone(),
            first.findings.clone(),
        );
        assert_eq!(
            hash_manifest(&first).unwrap(),
            hash_manifest(&second).unwrap()
        );
    }

    /// The semantic hash must survive a PostgreSQL `jsonb` round trip, which
    /// reorders object keys while the workspace serializes `serde_json::Value`
    /// in insertion order (`preserve_order`). Regression test for
    /// `validation.input_semantic_manifest_hash_mismatch` on manifests whose
    /// free-form subdocuments stored keys in non-jsonb order.
    #[test]
    fn manifest_hash_is_independent_of_value_key_order() {
        let mut manifest = ReleaseManifest::new(
            Uuid::now_v7(),
            None,
            "alpha".to_owned(),
            Some(1),
            Some("snapshot-a".to_owned()),
            Some("ffprobe".to_owned()),
            Some("/usr/bin/ffprobe".to_owned()),
            Some(0),
            Some(30_000),
            true,
            ManifestReleaseObservation {
                title: Some("Title".to_owned()),
                artists: vec!["Artist".to_owned()],
                date: None,
                // Multi-key document in insertion order that differs from
                // jsonb's (length, then bytewise) ordering.
                source_labels: serde_json::json!({"zz": 1, "a": 2, "mm": 3}),
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let original = hash_manifest(&manifest).unwrap();

        // Simulate jsonb reordering: rebuild every object with reversed key
        // order and deserialize the manifest back from it.
        fn reverse_keys(value: &serde_json::Value) -> serde_json::Value {
            match value {
                serde_json::Value::Object(map) => {
                    let mut reversed = serde_json::Map::new();
                    for (key, entry) in map.iter().rev() {
                        reversed.insert(key.clone(), reverse_keys(entry));
                    }
                    serde_json::Value::Object(reversed)
                }
                serde_json::Value::Array(items) => {
                    serde_json::Value::Array(items.iter().map(reverse_keys).collect())
                }
                scalar => scalar.clone(),
            }
        }
        let reordered = reverse_keys(&serde_json::to_value(&manifest).unwrap());
        manifest = serde_json::from_value(reordered).unwrap();
        assert_eq!(original, hash_manifest(&manifest).unwrap());
    }

    #[test]
    fn valid_tagless_wav_is_inspected_as_audio() {
        let root = temp_dir("tagless-wav");
        let wav = root.join("track.wav");
        let sample_rate = 8_000_u32;
        let sample_count = 800_u32;
        let data_size = sample_count * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0_u8, data_size as usize));
        fs::write(&wav, bytes).expect("write wav");

        let report = inspect_release(
            Uuid::now_v7(),
            None,
            "tagless",
            &root,
            Some(1),
            Some("snapshot"),
            &InspectionOptions::default(),
        )
        .expect("inspect wav");
        assert!(report.structurally_valid);
        assert_eq!(1, report.manifest.tracks.len());
        assert!(report.manifest.tracks[0].observed_metadata.is_none());
        assert!(report.manifest.tracks[0].duration_ms.is_some());
        assert!(report.manifest.files[0].content_hash.is_some());
    }

    #[test]
    fn corrupt_audio_is_an_explicit_structural_finding_and_sidecars_are_inventory() {
        let root = temp_dir("corrupt-sidecars");
        fs::write(root.join("broken.mp3"), b"not an mp3").expect("broken audio");
        for sidecar in ["disc.cue", "disc.nfo", "disc.m3u", "disc.sfv", "disc.lrc"] {
            fs::write(root.join(sidecar), b"sidecar").expect("sidecar");
        }
        fs::write(root.join("broken.txt"), b"plain lyrics").expect("lyrics sidecar");
        fs::write(root.join("README.txt"), b"unknown").expect("unknown");

        let report = inspect_release(
            Uuid::now_v7(),
            None,
            "corrupt",
            &root,
            None,
            None,
            &InspectionOptions::default(),
        )
        .expect("inspection report");
        assert!(!report.structurally_valid);
        assert!(
            report
                .manifest
                .findings
                .iter()
                .any(|finding| finding.code == "media.ffprobe_failed")
        );
        assert_eq!(6, report.manifest.sidecars.len());
        assert!(report.manifest.files.iter().any(
            |file| file.relative_path == "broken.txt" && file.kind == ObservedFileKind::Lyrics
        ));
        assert!(
            report
                .manifest
                .files
                .iter()
                .any(|file| file.kind == ObservedFileKind::Unknown)
        );
    }

    #[test]
    fn ffprobe_format_matrix_covers_the_required_format_vocabulary() {
        let root = temp_dir("format-matrix");
        let cases = [
            ("mp3", "libmp3lame", "mp3", "audio/mpeg"),
            ("flac", "flac", "flac", "audio/flac"),
            ("ogg", "libvorbis", "vorbis", "audio/ogg"),
            ("m4a", "aac", "aac", "audio/mp4"),
            ("opus", "libopus", "opus", "audio/ogg"),
            ("aac", "aac", "aac", "audio/aac"),
            ("ac3", "ac3", "ac3", "audio/ac3"),
            ("wma", "wmav2", "wmav2", "audio/x-ms-wma"),
            ("wav", "pcm_s16le", "pcm_s16le", "audio/wav"),
        ];
        for (extension, encoder, expected_codec, expected_mime) in cases {
            let case_root = root.join(extension);
            fs::create_dir_all(&case_root).expect("case root");
            let mut command = std::process::Command::new("ffmpeg");
            command.args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2:sample_rate=8000",
                "-c:a",
                encoder,
            ]);
            if extension == "aac" {
                command.args(["-f", "adts"]);
            } else if extension == "ac3" {
                command.args(["-f", "ac3"]);
            }
            if extension == "mp3" {
                command.args([
                    "-metadata",
                    "artist=Unicode 東京",
                    "-metadata",
                    "album=Observed Release",
                    "-metadata",
                    "title=First Light",
                    "-metadata",
                    "track=1/1",
                ]);
            }
            let output = command
                .arg(case_root.join(format!("track.{extension}")))
                .output()
                .expect("ffmpeg");
            assert!(output.status.success(), "ffmpeg failed for {extension}");
            let report = inspect_release(
                Uuid::now_v7(),
                None,
                extension,
                &case_root,
                None,
                None,
                &InspectionOptions::default(),
            )
            .expect("inspect generated media");
            assert!(report.structurally_valid, "{extension} should be valid");
            assert_eq!(
                Some(expected_mime.to_owned()),
                report.manifest.files[0].mime_type
            );
            let audio = report.manifest.files[0]
                .audio
                .as_ref()
                .unwrap_or_else(|| panic!("probed audio observation for {extension}"));
            assert_eq!(
                audio.codec.as_deref(),
                Some(expected_codec),
                "probed codec for {extension}"
            );
            let duration_ms = audio
                .duration_ms
                .unwrap_or_else(|| panic!("duration observed for {extension}"));
            assert!(
                (50..=2_000).contains(&duration_ms),
                "duration {duration_ms} ms implausible for the 0.2 s {extension} fixture"
            );
            assert_eq!(audio.stream_count, Some(1), "stream count for {extension}");
            if extension == "mp3" {
                let metadata = report.manifest.files[0]
                    .metadata
                    .as_ref()
                    .expect("native metadata");
                assert_eq!(Some("Unicode 東京".to_owned()), metadata.artist);
            }
        }
    }

    #[test]
    fn native_release_type_and_compilation_signals_are_captured_verbatim() {
        let root = temp_dir("classification-signals");
        let cases = [
            ("mp3", "libmp3lame", TagType::Id3v2, "compilation", "1"),
            (
                "flac",
                "flac",
                TagType::VorbisComments,
                "album; compilation",
                "0",
            ),
        ];

        for (extension, codec, tag_type, expected_release_type, expected_compilation) in cases {
            let path = root.join(format!("classification-signals.{extension}"));
            let output = std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.2:sample_rate=8000",
                    "-c:a",
                    codec,
                ])
                .arg(&path)
                .output()
                .expect("ffmpeg");
            assert!(output.status.success(), "ffmpeg failed for {extension}");

            let mut tag = Tag::new(tag_type);
            assert!(tag.insert_text(
                ItemKey::MusicBrainzReleaseType,
                expected_release_type.to_owned()
            ));
            assert!(tag.insert_text(ItemKey::FlagCompilation, expected_compilation.to_owned()));
            tag.save_to_path(&path, WriteOptions::default())
                .expect("write native tag");

            let (metadata, _, _) =
                read_native_metadata(&path, path.to_str().expect("UTF-8 path"), 0)
                    .expect("read native metadata");
            assert_eq!(
                metadata.raw["musicbrainz_release_type"],
                serde_json::json!(expected_release_type),
                "release type from {extension}"
            );
            assert_eq!(
                metadata.raw["flag_compilation"],
                serde_json::json!(expected_compilation),
                "compilation flag from {extension}"
            );
        }
    }

    /// TRACK-001: the six retained MusicBrainz identifier families are read back
    /// as raw observation strings in `ObservedMetadata.raw["musicbrainz_ids"]` for
    /// each format-native tag family (ID3v2 TXXX, Vorbis comment, MP4 atoms).
    #[test]
    fn musicbrainz_identifiers_are_retained_as_raw_observations_per_format() {
        const REC: &str = "c51b7f5e-3c31-4c9e-a1e1-9b8a7d6c5b4a";
        const REL: &str = "0e586670-1b32-4c1d-9a1f-6d8e2b3a4c5d";
        const RLG: &str = "1f2e3d4c-5b6a-4e7f-8a9b-0c1d2e3f4a5b";
        const ART: &str = "2a3b4c5d-6e7f-4a8b-9c0d-1e2f3a4b5c6d";
        const REL_ART: &str = "3b4c5d6e-7f8a-4b9c-0d1e-2f3a4b5c6d7e";
        const WORK: &str = "4c5d6e7f-8a9b-4c0d-1e2f-3a4b5c6d7e8f";

        // (observation key, canonical ItemKey, ID3v2.4 TXXX description). The
        // ID3v2.4 family has no fixed-frame ID, so its tag key is a TXXX
        // description — matching the legacy C# `[TXX, TXXX]` MusicBrainzId
        // mapping. Vorbis comments and MP4 `----` atoms use the same canonical
        // keys with fixed, case-insensitive names.
        const FAMILIES: [(&str, ItemKey, &str); 6] = [
            (
                "recording",
                ItemKey::MusicBrainzRecordingId,
                "MUSICBRAINZ_TRACKID",
            ),
            (
                "release",
                ItemKey::MusicBrainzReleaseId,
                "MUSICBRAINZ_ALBUMID",
            ),
            (
                "release_group",
                ItemKey::MusicBrainzReleaseGroupId,
                "MUSICBRAINZ_RELEASEGROUPID",
            ),
            (
                "artist",
                ItemKey::MusicBrainzArtistId,
                "MUSICBRAINZ_ARTISTID",
            ),
            (
                "release_artist",
                ItemKey::MusicBrainzReleaseArtistId,
                "MUSICBRAINZ_ALBUMARTISTID",
            ),
            ("work", ItemKey::MusicBrainzWorkId, "MUSICBRAINZ_WORKID"),
        ];
        const VALUES: [&str; 6] = [REC, REL, RLG, ART, REL_ART, WORK];

        let root = temp_dir("musicbrainz-ids");
        let (mp3, flac, m4a) = (
            root.join("mb-ids.mp3"),
            root.join("mb-ids.flac"),
            root.join("mb-ids.m4a"),
        );
        for (path, codec) in [(&mp3, "libmp3lame"), (&flac, "flac"), (&m4a, "aac")] {
            let output = std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.2:sample_rate=8000",
                    "-c:a",
                    codec,
                ])
                .arg(path)
                .output()
                .expect("ffmpeg");
            assert!(
                output.status.success(),
                "ffmpeg failed for {}",
                path.display()
            );
        }

        let mut id3 = Tag::new(TagType::Id3v2);
        let mut vorbis = Tag::new(TagType::VorbisComments);
        let mut mp4 = Tag::new(TagType::Mp4Ilst);
        for (index, (_, key, description)) in FAMILIES.iter().enumerate() {
            let value = VALUES[index].to_owned();
            // ID3v2: insert an item keyed for TXXX with the Picard description
            // so the writer emits `TXXX:MusicBrainz Track Id`-style frames that
            // lofty normalizes back to the canonical key on read.
            let mut item = TagItem::new(*key, ItemValue::Text(value.clone()));
            item.set_description(description.to_string());
            id3.insert_unchecked(item);
            assert!(
                vorbis.insert_text(*key, value.clone()),
                "insert {key:?} into vorbis"
            );
            assert!(mp4.insert_text(*key, value), "insert {key:?} into mp4");
        }
        id3.save_to_path(&mp3, WriteOptions::default())
            .expect("write id3 tag");
        vorbis
            .save_to_path(&flac, WriteOptions::default())
            .expect("write vorbis tag");
        mp4.save_to_path(&m4a, WriteOptions::default())
            .expect("write mp4 tag");

        // ID3v2.4 has no fixed frame for the release and release-group IDs:
        // lofty 0.25 maps only the other four families to TXXX/UFID frames, so
        // those two are not round-trip-able through lofty's writer and are
        // asserted only for the formats that support them (FLAC, MP4).
        let all_indexes = [0_usize, 1, 2, 3, 4, 5];
        let mp3_indexes = [0_usize, 3, 4, 5];
        let cases: [(&Path, &str, &[usize]); 3] = [
            (mp3.as_path(), "mp3", &mp3_indexes),
            (flac.as_path(), "flac", &all_indexes),
            (m4a.as_path(), "m4a", &all_indexes),
        ];
        for (path, format, indexes) in cases {
            let (metadata, _, _) =
                read_native_metadata(path, path.to_str().expect("UTF-8 path"), 0)
                    .expect("read native metadata");
            let ids = &metadata.raw["musicbrainz_ids"];
            for index in indexes {
                let index = *index;
                assert_eq!(
                    ids[FAMILIES[index].0],
                    serde_json::json!(VALUES[index]),
                    "{} id from {format}",
                    FAMILIES[index].0
                );
            }
        }
    }

    /// TRACK-001: a tagged file without MusicBrainz tags produces an empty
    /// observation object (no phantom `null` entries), and malformed values are
    /// retained verbatim as observations — never rejected or normalized.
    #[test]
    fn musicbrainz_id_observations_are_absent_when_untagged_and_verbatim_when_malformed() {
        let root = temp_dir("musicbrainz-ids-empty");

        // A tagged file without MusicBrainz IDs retains an empty observation
        // object — no phantom `null` entries for absent families.
        let tagged = root.join("tagged.flac");
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2:sample_rate=8000",
                "-c:a",
                "flac",
            ])
            .arg(&tagged)
            .output()
            .expect("ffmpeg");
        assert!(output.status.success(), "ffmpeg failed for flac fixture");
        let mut tag = Tag::new(TagType::VorbisComments);
        assert!(tag.insert_text(ItemKey::TrackTitle, "No MusicBrainz ids".to_owned()));
        tag.save_to_path(&tagged, WriteOptions::default())
            .expect("write tagged flac");
        let (metadata, _, _) =
            read_native_metadata(&tagged, "tagged.flac", 0).expect("read tagged flac");
        assert_eq!(
            metadata.raw["musicbrainz_ids"],
            serde_json::json!({}),
            "tagged file without MusicBrainz IDs retains an empty observation object"
        );

        // An untagged file carries no native tag at all: the absence is surfaced
        // as an error, not as a phantom observation object.
        let untagged = root.join("untagged.flac");
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2:sample_rate=8000",
                "-c:a",
                "flac",
            ])
            .arg(&untagged)
            .output()
            .expect("ffmpeg");
        assert!(
            output.status.success(),
            "ffmpeg failed for untagged flac fixture"
        );
        // ffmpeg always writes a Vorbis Comment block (with a vendor string),
        // so the file is not truly untagged. We verify that reading succeeds
        // but yields no user-visible metadata fields.
        let (metadata, _, _) =
            read_native_metadata(&untagged, "untagged.flac", 0).expect("untagged file reads");
        assert!(
            metadata.album.is_none()
                && metadata.album_artist.is_none()
                && metadata.track_title.is_none()
                && metadata.artist.is_none(),
            "untagged file has no user-visible metadata fields"
        );

        // A tagged FLAC with a malformed recording ID retains the value verbatim.
        let malformed = root.join("malformed.flac");
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2:sample_rate=8000",
                "-c:a",
                "flac",
            ])
            .arg(&malformed)
            .output()
            .expect("ffmpeg");
        assert!(
            output.status.success(),
            "ffmpeg failed for malformed flac fixture"
        );
        let mut tag = Tag::new(TagType::VorbisComments);
        assert!(tag.insert_text(
            ItemKey::MusicBrainzRecordingId,
            "not-a-guid at all".to_owned()
        ));
        tag.save_to_path(&malformed, WriteOptions::default())
            .expect("write malformed tag");
        let (metadata, _, _) =
            read_native_metadata(&malformed, "malformed.flac", 0).expect("read malformed flac");
        assert_eq!(
            metadata.raw["musicbrainz_ids"]["recording"],
            serde_json::json!("not-a-guid at all"),
            "malformed identifiers are retained verbatim as observations"
        );
    }

    /// TRACK-001 ingestion-level proof (Wave 1): a release directory whose
    /// audio files carry embedded MusicBrainz IDs produces a manifest in which
    /// `ObservedMetadata.raw["musicbrainz_ids"]` retains every present family
    /// as a raw observation string, for every format inspected here. The
    /// semantic manifest hash is stable across a jsonb-style key-ordering
    /// round trip, so the hash the publication gate compares
    /// (`input_manifest_hash`) depends only on observed facts.
    #[test]
    fn musicbrainz_ids_survive_inspection_into_the_durable_manifest() {
        const REC: &str = "aa1b1c1d-0000-7000-8000-000000000001";
        const REL: &str = "aa1b1c1d-0000-7000-8000-000000000002";
        const RLG: &str = "aa1b1c1d-0000-7000-8000-000000000003";
        const ART: &str = "aa1b1c1d-0000-7000-8000-000000000004";
        const REL_ART: &str = "aa1b1c1d-0000-7000-8000-000000000005";
        const WORK: &str = "aa1b1c1d-0000-7000-8000-000000000006";
        const FAMILIES: [(&str, ItemKey, &str); 6] = [
            (
                "recording",
                ItemKey::MusicBrainzRecordingId,
                "MUSICBRAINZ_TRACKID",
            ),
            (
                "release",
                ItemKey::MusicBrainzReleaseId,
                "MUSICBRAINZ_ALBUMID",
            ),
            (
                "release_group",
                ItemKey::MusicBrainzReleaseGroupId,
                "MUSICBRAINZ_RELEASEGROUPID",
            ),
            (
                "artist",
                ItemKey::MusicBrainzArtistId,
                "MUSICBRAINZ_ARTISTID",
            ),
            (
                "release_artist",
                ItemKey::MusicBrainzReleaseArtistId,
                "MUSICBRAINZ_ALBUMARTISTID",
            ),
            ("work", ItemKey::MusicBrainzWorkId, "MUSICBRAINZ_WORKID"),
        ];
        const VALUES: [&str; 6] = [REC, REL, RLG, ART, REL_ART, WORK];
        // ID3v2.4 cannot round-trip the release/release-group families through
        // lofty's writer, so MP3 asserts the four TXXX/UFID-mapped families;
        // FLAC and MP4 assert all six.
        const MP3_INDEXES: [usize; 4] = [0, 3, 4, 5];
        const ALL_INDEXES: [usize; 6] = [0, 1, 2, 3, 4, 5];

        let root = temp_dir("mb-ids-ingestion");
        let (mp3, flac, m4a) = (
            root.join("track.mp3"),
            root.join("album.flac"),
            root.join("disc.m4a"),
        );
        for (path, codec) in [(&mp3, "libmp3lame"), (&flac, "flac"), (&m4a, "aac")] {
            let output = std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.2:sample_rate=8000",
                    "-c:a",
                    codec,
                ])
                .arg(path)
                .output()
                .expect("ffmpeg");
            assert!(
                output.status.success(),
                "ffmpeg failed for {}",
                path.display()
            );
        }

        let mut id3 = Tag::new(TagType::Id3v2);
        let mut vorbis = Tag::new(TagType::VorbisComments);
        let mut mp4 = Tag::new(TagType::Mp4Ilst);
        for (index, (_, key, description)) in FAMILIES.iter().enumerate() {
            let value = VALUES[index].to_owned();
            let mut item = TagItem::new(*key, ItemValue::Text(value.clone()));
            item.set_description(description.to_string());
            id3.insert_unchecked(item);
            assert!(vorbis.insert_text(*key, value.clone()));
            assert!(mp4.insert_text(*key, value));
        }
        id3.save_to_path(&mp3, WriteOptions::default())
            .expect("write id3 tag");
        vorbis
            .save_to_path(&flac, WriteOptions::default())
            .expect("write vorbis tag");
        mp4.save_to_path(&m4a, WriteOptions::default())
            .expect("write mp4 tag");

        let report = inspect_release(
            Uuid::now_v7(),
            None,
            "mb-ids",
            &root,
            None,
            None,
            &InspectionOptions::default(),
        )
        .expect("inspect mb-tagged release");
        assert!(report.structurally_valid);
        assert_eq!(3, report.manifest.files.len());

        for (path, indexes) in [
            (&mp3, MP3_INDEXES.as_slice()),
            (&flac, ALL_INDEXES.as_slice()),
            (&m4a, ALL_INDEXES.as_slice()),
        ] {
            let file = report
                .manifest
                .files
                .iter()
                .find(|file| file.relative_path == path.file_name().unwrap().to_string_lossy())
                .unwrap_or_else(|| panic!("inspected file for {}", path.display()));
            let metadata = file
                .metadata
                .as_ref()
                .unwrap_or_else(|| panic!("native metadata for {}", path.display()));
            let ids = &metadata.raw["musicbrainz_ids"];
            for index in indexes {
                let index = *index;
                assert_eq!(
                    ids[FAMILIES[index].0],
                    serde_json::json!(VALUES[index]),
                    "{} retained in the durable manifest for {}",
                    FAMILIES[index].0,
                    path.display()
                );
            }
        }

        // The hash the ingestion/publication gate compares must be a function
        // of observed facts only: a jsonb-style key reordering of the whole
        // manifest (including the retained ID object) leaves it unchanged.
        let original = hash_manifest(&report.manifest).unwrap();
        fn reverse_keys(value: &serde_json::Value) -> serde_json::Value {
            match value {
                serde_json::Value::Object(map) => {
                    let mut reversed = serde_json::Map::new();
                    for (key, entry) in map.iter().rev() {
                        reversed.insert(key.clone(), reverse_keys(entry));
                    }
                    serde_json::Value::Object(reversed)
                }
                serde_json::Value::Array(items) => {
                    serde_json::Value::Array(items.iter().map(reverse_keys).collect())
                }
                scalar => scalar.clone(),
            }
        }
        let reordered = reverse_keys(&serde_json::to_value(&report.manifest).unwrap());
        let decoded: ReleaseManifest = serde_json::from_value(reordered).unwrap();
        assert_eq!(original, hash_manifest(&decoded).unwrap());
        assert_eq!(report.manifest_hash, original);
    }

    #[cfg(unix)]
    #[test]
    fn ffprobe_timeout_becomes_a_structured_finding() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("probe-timeout");
        let wav = root.join("track.wav");
        let sample_rate = 8_000_u32;
        let data_size = 160_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0_u8, data_size as usize));
        fs::write(&wav, bytes).expect("wav");
        let wrapper = root.join("slow-ffprobe.sh");
        // Keep the version probe fast, then block the media probe using only
        // shell builtins. Calling an external `sleep` can itself fail to fork
        // under a highly parallel workspace test run and turn the expected
        // timeout into a process-failure finding.
        fs::write(
            &wrapper,
            b"#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then\n  echo 'ffprobe test wrapper'\n  exit 0\nfi\nwhile :; do :; done\n",
        )
        .expect("wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("permissions");
        let options = InspectionOptions {
            ffprobe_path: wrapper,
            ffprobe_timeout: StdDuration::from_millis(20),
            ..InspectionOptions::default()
        };
        let report = inspect_release(Uuid::now_v7(), None, "timeout", &root, None, None, &options)
            .expect("timeout report");
        assert!(!report.structurally_valid);
        assert!(
            report
                .manifest
                .findings
                .iter()
                .any(|finding| finding.code == "media.ffprobe_timeout")
        );
    }

    #[test]
    #[ignore = "evidence harness; run explicitly with --ignored --nocapture"]
    fn media_inspection_performance_evidence() {
        let root = temp_dir("performance");
        let source_wav = root.join("source.wav");
        let sample_rate = 8_000_u32;
        let data_size = 8_000_u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend(std::iter::repeat_n(0_u8, data_size as usize));
        fs::write(&source_wav, wav).expect("source wav");

        let cases = [
            ("small-wav", 1_usize),
            ("large-track-count", 20),
            ("artwork-heavy", 1),
        ];
        for (name, count) in cases {
            let case_root = root.join(name);
            fs::create_dir_all(&case_root).expect("case root");
            for index in 0..count {
                fs::copy(&source_wav, case_root.join(format!("track-{index:02}.wav")))
                    .expect("copy wav");
            }
            if name == "artwork-heavy" {
                let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
                png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
                png.extend_from_slice(&1_u32.to_be_bytes());
                png.extend_from_slice(&1_u32.to_be_bytes());
                png.extend_from_slice(&[8, 2, 0, 0, 0]);
                fs::write(case_root.join("cover.png"), png).expect("artwork");
            }
            let started = Instant::now();
            let report = inspect_release(
                Uuid::now_v7(),
                None,
                name,
                &case_root,
                None,
                None,
                &InspectionOptions::default(),
            )
            .expect("benchmark inspection");
            let manifest_size = serde_json::to_vec(&report.manifest)
                .expect("manifest json")
                .len();
            println!(
                "benchmark={{\"case\":\"{name}\",\"files\":{},\"bytes_hashed\":{},\"ffprobe_invocations\":{},\"elapsed_ms\":{},\"manifest_bytes\":{}}}",
                report.file_count,
                report.bytes_hashed,
                report.ffprobe_invocations,
                started.elapsed().as_millis(),
                manifest_size,
            );
        }
    }
}
