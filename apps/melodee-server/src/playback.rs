//! Local playback backend boundary (ADR-0156).
//!
//! The backend is deliberately an application-layer protocol adapter. MPD and MPV are
//! operator-owned processes; their failure is reported as an unavailable optional subsystem and
//! never blocks catalog, streaming, or ingestion work.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::{Arc, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpStream, UnixStream},
    sync::{Mutex, Notify},
    time::timeout,
};
use uuid::Uuid;

use melodee_persistence::SettingsResolver;

type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, PlaybackError>> + Send + 'a>>;

use std::future::Future;

/// Cooperative cancellation passed through backend operations.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        self.notify.notified().await;
    }
}

/// Public backend capability/status contract. The JSON names intentionally mirror the existing
/// native playback endpoint vocabulary.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackCapabilities {
    pub can_play: bool,
    pub can_pause: bool,
    pub can_stop: bool,
    pub can_seek: bool,
    pub can_skip: bool,
    pub can_set_volume: bool,
    pub can_report_position: bool,
    pub is_available: bool,
    pub backend_info: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStatus {
    pub backend: String,
    pub state: String,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub current_file: Option<String>,
    pub current_item_api_key: Option<Uuid>,
    pub volume: Option<u8>,
    pub is_available: bool,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("local playback is disabled")]
    Disabled,
    #[error("configured playback backend is unsupported")]
    Unsupported,
    #[error("playback backend is unavailable")]
    Unavailable,
    #[error("playback backend operation timed out")]
    Timeout,
    #[error("playback backend operation was cancelled")]
    Cancelled,
    #[error("playback backend returned a malformed response")]
    MalformedResponse,
    #[error("playback backend rejected the command")]
    Rejected,
    #[error("playback backend configuration is invalid")]
    Configuration,
    #[error("playback backend I/O failed")]
    Io(#[source] std::io::Error),
}

impl PlaybackError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Io(_) | Self::Unavailable | Self::Timeout)
    }
}

/// Protocol-independent backend seam. It is intentionally outside the domain crate because it
/// depends on asynchronous sockets, cancellation, and operator configuration.
pub trait PlaybackBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities<'a>(
        &'a self,
        cancel: CancellationToken,
    ) -> BackendFuture<'a, PlaybackCapabilities>;
    fn initialize<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()>;
    fn shutdown<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()>;
    fn play<'a>(
        &'a self,
        path: &'a Path,
        start_position_seconds: f64,
        cancel: CancellationToken,
    ) -> BackendFuture<'a, ()>;
    fn pause<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()>;
    fn resume<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()>;
    fn stop<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()>;
    fn seek<'a>(
        &'a self,
        position_seconds: f64,
        cancel: CancellationToken,
    ) -> BackendFuture<'a, ()>;
    fn set_volume<'a>(&'a self, volume: u8, cancel: CancellationToken) -> BackendFuture<'a, ()>;
    fn status<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, PlaybackStatus>;
}

#[derive(Clone, Debug)]
struct MpdConfig {
    host: String,
    port: u16,
    password: Option<String>,
    timeout: Duration,
    initial_volume: u8,
}

struct MpdBackend {
    config: MpdConfig,
    connection: Mutex<Option<TcpStream>>,
}

impl MpdBackend {
    fn new(config: MpdConfig) -> Self {
        Self {
            config,
            connection: Mutex::new(None),
        }
    }

    async fn connect(&self) -> Result<TcpStream, PlaybackError> {
        let address = format!("{}:{}", self.config.host, self.config.port);
        let mut stream = timeout(self.config.timeout, TcpStream::connect(address))
            .await
            .map_err(|_| PlaybackError::Timeout)?
            .map_err(PlaybackError::Io)?;
        let greeting = read_line(&mut stream, self.config.timeout).await?;
        if !greeting.starts_with("OK MPD") {
            return Err(PlaybackError::MalformedResponse);
        }
        if let Some(password) = self
            .config
            .password
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let command = format!("password {}\n", quote_mpd(password));
            stream
                .write_all(command.as_bytes())
                .await
                .map_err(PlaybackError::Io)?;
            let lines = read_mpd_reply(&mut stream, self.config.timeout).await?;
            if lines.iter().any(|line| line.starts_with("ACK")) {
                return Err(PlaybackError::Rejected);
            }
        }
        Ok(stream)
    }

    async fn command_once(&self, command: &str) -> Result<Vec<String>, PlaybackError> {
        let mut connection = self.connection.lock().await;
        if connection.is_none() {
            *connection = Some(self.connect().await?);
        }
        let Some(stream) = connection.as_mut() else {
            return Err(PlaybackError::Unavailable);
        };
        let result = async {
            stream
                .write_all(format!("{command}\n").as_bytes())
                .await
                .map_err(PlaybackError::Io)?;
            read_mpd_reply(stream, self.config.timeout).await
        }
        .await;
        if result.is_err() {
            *connection = None;
        }
        result
    }

    async fn command(
        &self,
        command: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<String>, PlaybackError> {
        let mut last_error = PlaybackError::Unavailable;
        for attempt in 0..=1 {
            let operation = self.command_once(command);
            let result = tokio::select! {
                _ = cancel.cancelled() => Err(PlaybackError::Cancelled),
                result = timeout(self.config.timeout, operation) => result.map_err(|_| PlaybackError::Timeout)?,
            };
            match result {
                Ok(lines) => return Ok(lines),
                Err(error) if attempt == 0 && error.is_retryable() => last_error = error,
                Err(error) => return Err(error),
            }
        }
        Err(last_error)
    }

    async fn simple(
        &self,
        command: String,
        cancel: &CancellationToken,
    ) -> Result<(), PlaybackError> {
        let lines = self.command(&command, cancel).await?;
        if lines.iter().any(|line| line.starts_with("ACK")) {
            Err(PlaybackError::Rejected)
        } else {
            Ok(())
        }
    }
}

impl PlaybackBackend for MpdBackend {
    fn name(&self) -> &'static str {
        "mpd"
    }

    fn capabilities<'a>(
        &'a self,
        cancel: CancellationToken,
    ) -> BackendFuture<'a, PlaybackCapabilities> {
        Box::pin(async move {
            let available = self.command("status", &cancel).await.is_ok();
            Ok(PlaybackCapabilities {
                can_play: available,
                can_pause: available,
                can_stop: available,
                can_seek: available,
                can_skip: available,
                can_set_volume: available,
                can_report_position: available,
                is_available: available,
                backend_info: Some("MPD".to_owned()),
            })
        })
    }

    fn initialize<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.simple(format!("volume {}", self.config.initial_volume), &cancel)
                .await
                .or_else(|error| {
                    if matches!(error, PlaybackError::Rejected) {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
        })
    }

    fn shutdown<'a>(&'a self, _cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            let mut connection = self.connection.lock().await;
            *connection = None;
            Ok(())
        })
    }

    fn play<'a>(
        &'a self,
        path: &'a Path,
        start: f64,
        cancel: CancellationToken,
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.simple("clear".to_owned(), &cancel).await?;
            self.simple(
                format!("add \"{}\"", quote_mpd(&path.to_string_lossy())),
                &cancel,
            )
            .await?;
            if start > 0.0 {
                self.simple(format!("seekcur {start:.3}"), &cancel).await?;
            }
            self.simple("play".to_owned(), &cancel).await
        })
    }

    fn pause<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move { self.simple("pause 1".to_owned(), &cancel).await })
    }

    fn resume<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move { self.simple("pause 0".to_owned(), &cancel).await })
    }

    fn stop<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move { self.simple("stop".to_owned(), &cancel).await })
    }

    fn seek<'a>(&'a self, position: f64, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move { self.simple(format!("seekcur {position:.3}"), &cancel).await })
    }

    fn set_volume<'a>(&'a self, volume: u8, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move { self.simple(format!("volume {volume}"), &cancel).await })
    }

    fn status<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, PlaybackStatus> {
        Box::pin(async move {
            let lines = self.command("status", &cancel).await?;
            let values = parse_key_values(&lines);
            let state = values
                .get("state")
                .cloned()
                .unwrap_or_else(|| "stop".to_owned());
            let (position, duration) = values
                .get("time")
                .and_then(|value| value.split_once(':'))
                .map(|(position, duration)| {
                    (position.parse().unwrap_or(0.0), duration.parse().ok())
                })
                .unwrap_or((0.0, None));
            let volume = values
                .get("volume")
                .and_then(|value| value.parse::<u8>().ok());
            Ok(PlaybackStatus {
                backend: self.name().to_owned(),
                state,
                position_seconds: position,
                duration_seconds: duration,
                current_file: None,
                current_item_api_key: None,
                volume,
                is_available: true,
                error: None,
            })
        })
    }
}

#[derive(Clone, Debug)]
struct MpvConfig {
    socket_path: PathBuf,
    timeout: Duration,
    initial_volume: u8,
}

struct MpvBackend {
    config: MpvConfig,
    connection: Mutex<Option<UnixStream>>,
    request_id: AtomicU64,
}

impl MpvBackend {
    fn new(config: MpvConfig) -> Self {
        Self {
            config,
            connection: Mutex::new(None),
            request_id: AtomicU64::new(1),
        }
    }

    async fn connect(&self) -> Result<UnixStream, PlaybackError> {
        timeout(
            self.config.timeout,
            UnixStream::connect(&self.config.socket_path),
        )
        .await
        .map_err(|_| PlaybackError::Timeout)?
        .map_err(PlaybackError::Io)
    }

    async fn command_once(&self, request: Value) -> Result<Value, PlaybackError> {
        let mut connection = self.connection.lock().await;
        if connection.is_none() {
            *connection = Some(self.connect().await?);
        }
        let Some(stream) = connection.as_mut() else {
            return Err(PlaybackError::Unavailable);
        };
        let result = async {
            stream
                .write_all(format!("{}\n", request).as_bytes())
                .await
                .map_err(PlaybackError::Io)?;
            let mut reader = BufReader::new(&mut *stream);
            let mut line = String::new();
            loop {
                line.clear();
                timeout(self.config.timeout, reader.read_line(&mut line))
                    .await
                    .map_err(|_| PlaybackError::Timeout)?
                    .map_err(PlaybackError::Io)?;
                if line.is_empty() {
                    return Err(PlaybackError::Unavailable);
                }
                let response: Value = serde_json::from_str(line.trim())
                    .map_err(|_| PlaybackError::MalformedResponse)?;
                if response.get("request_id") == request.get("request_id") {
                    if response
                        .get("error")
                        .and_then(Value::as_str)
                        .is_some_and(|error| error != "success")
                    {
                        return Err(PlaybackError::Rejected);
                    }
                    return Ok(response);
                }
            }
        }
        .await;
        if result.is_err() {
            *connection = None;
        }
        result
    }

    async fn command(
        &self,
        command: Vec<Value>,
        cancel: &CancellationToken,
    ) -> Result<Value, PlaybackError> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let request = serde_json::json!({"command": command, "request_id": id});
        tokio::select! {
            _ = cancel.cancelled() => Err(PlaybackError::Cancelled),
            result = timeout(self.config.timeout, self.command_once(request)) => result.map_err(|_| PlaybackError::Timeout)?,
        }
    }
}

impl PlaybackBackend for MpvBackend {
    fn name(&self) -> &'static str {
        "mpv"
    }

    fn capabilities<'a>(
        &'a self,
        cancel: CancellationToken,
    ) -> BackendFuture<'a, PlaybackCapabilities> {
        Box::pin(async move {
            let available = self
                .command(
                    vec![
                        Value::String("get_property".to_owned()),
                        Value::String("pause".to_owned()),
                    ],
                    &cancel,
                )
                .await
                .is_ok();
            Ok(PlaybackCapabilities {
                can_play: available,
                can_pause: available,
                can_stop: available,
                can_seek: available,
                can_skip: available,
                can_set_volume: available,
                can_report_position: available,
                is_available: available,
                backend_info: Some("MPV IPC".to_owned()),
            })
        })
    }

    fn initialize<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.command(
                vec![
                    Value::String("set".to_owned()),
                    Value::String("volume".to_owned()),
                    Value::from(self.config.initial_volume),
                ],
                &cancel,
            )
            .await
            .map(|_| ())
        })
    }

    fn shutdown<'a>(&'a self, _cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            let mut connection = self.connection.lock().await;
            *connection = None;
            Ok(())
        })
    }

    fn play<'a>(
        &'a self,
        path: &'a Path,
        start: f64,
        cancel: CancellationToken,
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.command(
                vec![
                    Value::String("loadfile".to_owned()),
                    Value::String(path.to_string_lossy().to_string()),
                    Value::String("replace".to_owned()),
                    serde_json::json!({"start": start.max(0.0)}),
                ],
                &cancel,
            )
            .await
            .map(|_| ())
        })
    }

    fn pause<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.command(
                vec![
                    Value::String("set".to_owned()),
                    Value::String("pause".to_owned()),
                    Value::Bool(true),
                ],
                &cancel,
            )
            .await
            .map(|_| ())
        })
    }

    fn resume<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.command(
                vec![
                    Value::String("set".to_owned()),
                    Value::String("pause".to_owned()),
                    Value::Bool(false),
                ],
                &cancel,
            )
            .await
            .map(|_| ())
        })
    }

    fn stop<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.command(vec![Value::String("stop".to_owned())], &cancel)
                .await
                .map(|_| ())
        })
    }

    fn seek<'a>(&'a self, position: f64, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.command(
                vec![
                    Value::String("seek".to_owned()),
                    Value::from(position.max(0.0)),
                    Value::String("absolute".to_owned()),
                ],
                &cancel,
            )
            .await
            .map(|_| ())
        })
    }

    fn set_volume<'a>(&'a self, volume: u8, cancel: CancellationToken) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.command(
                vec![
                    Value::String("set".to_owned()),
                    Value::String("volume".to_owned()),
                    Value::from(volume),
                ],
                &cancel,
            )
            .await
            .map(|_| ())
        })
    }

    fn status<'a>(&'a self, cancel: CancellationToken) -> BackendFuture<'a, PlaybackStatus> {
        Box::pin(async move {
            let mut properties = HashMap::new();
            for property in [
                "pause",
                "idle-active",
                "time-pos",
                "duration",
                "filename",
                "volume",
            ] {
                let response = self
                    .command(
                        vec![
                            Value::String("get_property".to_owned()),
                            Value::String(property.to_owned()),
                        ],
                        &cancel,
                    )
                    .await?;
                properties.insert(property, response.get("data").cloned());
            }
            let paused = properties
                .get("pause")
                .and_then(Option::as_ref)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let idle = properties
                .get("idle-active")
                .and_then(Option::as_ref)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let current_file = properties
                .get("filename")
                .and_then(Option::as_ref)
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            Ok(PlaybackStatus {
                backend: self.name().to_owned(),
                state: if idle || current_file.is_none() {
                    "stopped"
                } else if paused {
                    "paused"
                } else {
                    "playing"
                }
                .to_owned(),
                position_seconds: properties
                    .get("time-pos")
                    .and_then(Option::as_ref)
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                duration_seconds: properties
                    .get("duration")
                    .and_then(Option::as_ref)
                    .and_then(Value::as_f64),
                current_file,
                current_item_api_key: None,
                volume: properties
                    .get("volume")
                    .and_then(Option::as_ref)
                    .and_then(Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok()),
                is_available: true,
                error: None,
            })
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayRequest {
    pub song_api_key: Uuid,
    pub start_position_seconds: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeekRequest {
    pub position_seconds: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VolumeRequest {
    pub volume: u8,
}

#[derive(Clone, Debug)]
enum ConfiguredBackend {
    Disabled,
    Mpd(MpdConfig),
    Mpv(MpvConfig),
}

#[derive(Clone, Debug)]
pub struct PlaybackService {
    pool: sqlx::PgPool,
}

impl PlaybackService {
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    async fn configured(&self) -> Result<ConfiguredBackend, PlaybackError> {
        let settings = SettingsResolver::new(&self.pool);
        let enabled = setting_bool(&settings, "jukebox.enabled").await?;
        let kind = setting_string(&settings, "jukebox.backendType")
            .await?
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !enabled || kind.is_empty() {
            return Ok(ConfiguredBackend::Disabled);
        }
        match kind.as_str() {
            "mpd" => {
                let config = MpdConfig {
                    host: setting_string(&settings, "mpd.host")
                        .await?
                        .unwrap_or_else(|| "localhost".to_owned()),
                    port: setting_u16(&settings, "mpd.port").await?.unwrap_or(6600),
                    password: setting_string(&settings, "mpd.password").await?,
                    timeout: Duration::from_millis(
                        setting_u64(&settings, "mpd.timeoutMs")
                            .await?
                            .unwrap_or(10_000)
                            .clamp(1, 60_000),
                    ),
                    initial_volume: setting_u8_percent(&settings, "mpd.initialVolume")
                        .await?
                        .unwrap_or(80),
                };
                select_configured_backend(true, &kind, Some(config), None)
            }
            "mpv" => {
                let socket = setting_string(&settings, "mpv.socketPath")
                    .await?
                    .filter(|value| !value.is_empty())
                    .ok_or(PlaybackError::Configuration)?;
                let config = MpvConfig {
                    socket_path: PathBuf::from(socket),
                    timeout: Duration::from_millis(
                        setting_u64(&settings, "mpv.timeoutMs")
                            .await?
                            .unwrap_or(5_000)
                            .clamp(1, 60_000),
                    ),
                    initial_volume: setting_u8_percent(&settings, "mpv.initialVolume")
                        .await?
                        .unwrap_or(80),
                };
                select_configured_backend(true, &kind, None, Some(config))
            }
            _ => Err(PlaybackError::Unsupported),
        }
    }

    async fn backend(&self) -> Result<Arc<dyn PlaybackBackend>, PlaybackError> {
        let configured = self.configured().await?;
        let key = backend_key(&configured);
        let registry = BACKEND_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
        let mut registry = registry.lock().await;
        if let Some(existing) = registry.get(&key) {
            return Ok(Arc::clone(existing));
        }
        let backend: Arc<dyn PlaybackBackend> = match configured {
            ConfiguredBackend::Disabled => return Err(PlaybackError::Disabled),
            ConfiguredBackend::Mpd(config) => Arc::new(MpdBackend::new(config)),
            ConfiguredBackend::Mpv(config) => Arc::new(MpvBackend::new(config)),
        };
        registry.insert(key, Arc::clone(&backend));
        Ok(backend)
    }

    pub async fn capabilities(
        &self,
        cancel: CancellationToken,
    ) -> Result<PlaybackCapabilities, PlaybackError> {
        self.backend().await?.capabilities(cancel).await
    }

    pub async fn initialize(&self, cancel: CancellationToken) -> Result<(), PlaybackError> {
        self.backend().await?.initialize(cancel).await
    }

    pub async fn shutdown(&self, cancel: CancellationToken) -> Result<(), PlaybackError> {
        let configured = self.configured().await?;
        let key = backend_key(&configured);
        if let Some(backend) = BACKEND_REGISTRY
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .await
            .remove(&key)
        {
            backend.shutdown(cancel).await?;
        }
        Ok(())
    }

    pub async fn play(
        &self,
        request: &PlayRequest,
        cancel: CancellationToken,
    ) -> Result<(), PlaybackError> {
        let path: String = sqlx::query_scalar(
            "SELECT m.canonical_relative_path FROM tracks t JOIN media_assets m ON m.id=t.playable_media_asset_id WHERE t.public_api_id=$1",
        )
        .bind(request.song_api_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| PlaybackError::Unavailable)?
        .ok_or(PlaybackError::Unavailable)?;
        self.backend()
            .await?
            .play(
                Path::new(&path),
                request.start_position_seconds.unwrap_or(0.0),
                cancel,
            )
            .await
    }

    pub async fn pause(&self, cancel: CancellationToken) -> Result<(), PlaybackError> {
        self.backend().await?.pause(cancel).await
    }
    pub async fn resume(&self, cancel: CancellationToken) -> Result<(), PlaybackError> {
        self.backend().await?.resume(cancel).await
    }
    pub async fn stop(&self, cancel: CancellationToken) -> Result<(), PlaybackError> {
        self.backend().await?.stop(cancel).await
    }
    pub async fn seek(
        &self,
        request: &SeekRequest,
        cancel: CancellationToken,
    ) -> Result<(), PlaybackError> {
        self.backend()
            .await?
            .seek(request.position_seconds, cancel)
            .await
    }
    pub async fn set_volume(
        &self,
        request: &VolumeRequest,
        cancel: CancellationToken,
    ) -> Result<(), PlaybackError> {
        self.backend()
            .await?
            .set_volume(request.volume, cancel)
            .await
    }
    pub async fn status(&self, cancel: CancellationToken) -> Result<PlaybackStatus, PlaybackError> {
        self.backend().await?.status(cancel).await
    }
}

fn select_configured_backend(
    enabled: bool,
    kind: &str,
    mpd: Option<MpdConfig>,
    mpv: Option<MpvConfig>,
) -> Result<ConfiguredBackend, PlaybackError> {
    if !enabled || kind.is_empty() {
        return Ok(ConfiguredBackend::Disabled);
    }
    match kind {
        "mpd" => mpd
            .map(ConfiguredBackend::Mpd)
            .ok_or(PlaybackError::Configuration),
        "mpv" => mpv
            .map(ConfiguredBackend::Mpv)
            .ok_or(PlaybackError::Configuration),
        _ => Err(PlaybackError::Unsupported),
    }
}

static BACKEND_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<dyn PlaybackBackend>>>> =
    OnceLock::new();

fn backend_key(configured: &ConfiguredBackend) -> String {
    match configured {
        ConfiguredBackend::Disabled => "disabled".to_owned(),
        ConfiguredBackend::Mpd(config) => format!("mpd:{}:{}", config.host, config.port),
        ConfiguredBackend::Mpv(config) => format!("mpv:{}", config.socket_path.display()),
    }
}

async fn setting_value(
    settings: &SettingsResolver<'_>,
    key: &str,
) -> Result<Option<Value>, PlaybackError> {
    settings
        .resolve(key)
        .await
        .map(|setting| setting.value)
        .map_err(|_| PlaybackError::Configuration)
}

async fn setting_string(
    settings: &SettingsResolver<'_>,
    key: &str,
) -> Result<Option<String>, PlaybackError> {
    Ok(setting_value(settings, key)
        .await?
        .and_then(|value| value.as_str().map(ToOwned::to_owned)))
}

async fn setting_bool(settings: &SettingsResolver<'_>, key: &str) -> Result<bool, PlaybackError> {
    Ok(setting_value(settings, key)
        .await?
        .and_then(|value| value.as_bool())
        .unwrap_or(false))
}

async fn setting_u64(
    settings: &SettingsResolver<'_>,
    key: &str,
) -> Result<Option<u64>, PlaybackError> {
    Ok(setting_value(settings, key)
        .await?
        .and_then(|value| value.as_u64()))
}

async fn setting_u16(
    settings: &SettingsResolver<'_>,
    key: &str,
) -> Result<Option<u16>, PlaybackError> {
    Ok(setting_u64(settings, key)
        .await?
        .and_then(|value| u16::try_from(value).ok()))
}

async fn setting_u8_percent(
    settings: &SettingsResolver<'_>,
    key: &str,
) -> Result<Option<u8>, PlaybackError> {
    Ok(setting_value(settings, key)
        .await?
        .and_then(|value| value.as_f64())
        .map(|value| (value.clamp(0.0, 1.0) * 100.0).round() as u8))
}

async fn read_line(stream: &mut TcpStream, deadline: Duration) -> Result<String, PlaybackError> {
    let mut reader = BufReader::new(&mut *stream);
    let mut line = String::new();
    timeout(deadline, reader.read_line(&mut line))
        .await
        .map_err(|_| PlaybackError::Timeout)?
        .map_err(PlaybackError::Io)?;
    if line.is_empty() {
        return Err(PlaybackError::Unavailable);
    }
    Ok(line.trim_end().to_owned())
}

async fn read_mpd_reply(
    stream: &mut TcpStream,
    deadline: Duration,
) -> Result<Vec<String>, PlaybackError> {
    let mut reader = BufReader::new(&mut *stream);
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        timeout(deadline, reader.read_line(&mut line))
            .await
            .map_err(|_| PlaybackError::Timeout)?
            .map_err(PlaybackError::Io)?;
        if line.is_empty() {
            return Err(PlaybackError::Unavailable);
        }
        let line = line.trim_end().to_owned();
        let done = line == "OK" || line.starts_with("ACK");
        lines.push(line);
        if done {
            return Ok(lines);
        }
    }
}

fn parse_key_values(lines: &[String]) -> HashMap<String, String> {
    lines
        .iter()
        .filter_map(|line| {
            line.split_once(": ")
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn quote_mpd(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::net::SocketAddr;

    use tempfile::tempdir;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, UnixListener},
    };

    use super::*;

    #[test]
    fn mpd_paths_are_quoted_without_leaking_newlines() {
        assert_eq!(quote_mpd("a\\b\"c"), "a\\\\b\\\"c");
    }

    #[test]
    fn mpd_status_fields_are_parsed() {
        let lines = vec![
            "state: play".to_owned(),
            "time: 12:120".to_owned(),
            "volume: 80".to_owned(),
            "OK".to_owned(),
        ];
        let values = parse_key_values(&lines);
        assert_eq!(values.get("state"), Some(&"play".to_owned()));
        assert_eq!(values.get("volume"), Some(&"80".to_owned()));
    }

    #[test]
    fn configuration_selection_is_truthful_for_disabled_mpd_and_mpv() {
        let mpd = MpdConfig {
            host: "localhost".to_owned(),
            port: 6600,
            password: None,
            timeout: Duration::from_secs(1),
            initial_volume: 80,
        };
        let mpv = MpvConfig {
            socket_path: PathBuf::from("/run/mpv.sock"),
            timeout: Duration::from_secs(1),
            initial_volume: 80,
        };
        assert!(matches!(
            select_configured_backend(false, "mpd", Some(mpd.clone()), None),
            Ok(ConfiguredBackend::Disabled)
        ));
        assert!(matches!(
            select_configured_backend(true, "mpd", Some(mpd), None),
            Ok(ConfiguredBackend::Mpd(_))
        ));
        assert!(matches!(
            select_configured_backend(true, "mpv", None, Some(mpv)),
            Ok(ConfiguredBackend::Mpv(_))
        ));
        assert!(matches!(
            select_configured_backend(true, "mpv", None, None),
            Err(PlaybackError::Configuration)
        ));
    }

    #[tokio::test]
    async fn cancellation_is_observed_before_a_backend_operation() {
        let token = CancellationToken::new();
        token.cancel();
        let backend = MpdBackend::new(MpdConfig {
            host: "127.0.0.1".to_owned(),
            port: 1,
            password: None,
            timeout: Duration::from_secs(1),
            initial_volume: 80,
        });
        assert!(matches!(
            backend.stop(token).await,
            Err(PlaybackError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn fake_mpd_server_exercises_lifecycle_and_status() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind MPD fixture");
        let address = listener.local_addr().expect("MPD address");
        let server = tokio::spawn(async move {
            let (stream, _): (TcpStream, SocketAddr) = listener.accept().await.expect("accept MPD");
            let (read_half, mut write_half) = stream.into_split();
            write_half
                .write_all(b"OK MPD 0.23.5\n")
                .await
                .expect("greeting");
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.expect("read MPD command") == 0 {
                    break;
                }
                if line.trim_end() == "status" {
                    write_half
                        .write_all(b"state: play\ntime: 12:120\nvolume: 65\nOK\n")
                        .await
                        .expect("status response");
                } else {
                    write_half
                        .write_all(b"OK\n")
                        .await
                        .expect("command response");
                }
            }
        });
        let backend = MpdBackend::new(MpdConfig {
            host: address.ip().to_string(),
            port: address.port(),
            password: None,
            timeout: Duration::from_secs(1),
            initial_volume: 65,
        });
        let cancel = CancellationToken::new();
        backend
            .initialize(cancel.clone())
            .await
            .expect("initialize MPD");
        backend
            .play(Path::new("music/track.mp3"), 0.0, cancel.clone())
            .await
            .expect("play MPD");
        let status = backend.status(cancel).await.expect("status MPD");
        assert_eq!(status.state, "play");
        assert_eq!(status.position_seconds, 12.0);
        assert_eq!(status.duration_seconds, Some(120.0));
        backend
            .shutdown(CancellationToken::new())
            .await
            .expect("shutdown MPD");
        server.abort();
    }

    #[tokio::test]
    async fn fake_mpv_ipc_exercises_json_commands_and_status() {
        let directory = tempdir().expect("MPV socket directory");
        let socket_path = directory.path().join("mpv.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind MPV fixture");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept MPV");
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.expect("read MPV command") == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(line.trim()).expect("MPV JSON");
                let request_id = request.get("request_id").cloned().expect("request id");
                let property = request
                    .get("command")
                    .and_then(Value::as_array)
                    .and_then(|command| command.get(1))
                    .and_then(Value::as_str);
                let data = match property {
                    Some("pause") => Value::Bool(false),
                    Some("idle-active") => Value::Bool(false),
                    Some("time-pos") => Value::from(4.0),
                    Some("duration") => Value::from(42.0),
                    Some("filename") => Value::String("music/track.mp3".to_owned()),
                    Some("volume") => Value::from(70),
                    _ => Value::Null,
                };
                let response =
                    serde_json::json!({"error":"success","request_id":request_id,"data":data});
                write_half
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .expect("MPV response");
            }
        });
        let backend = MpvBackend::new(MpvConfig {
            socket_path,
            timeout: Duration::from_secs(1),
            initial_volume: 80,
        });
        let status = backend
            .status(CancellationToken::new())
            .await
            .expect("status MPV");
        assert_eq!(status.backend, "mpv");
        assert_eq!(status.position_seconds, 4.0);
        assert_eq!(status.duration_seconds, Some(42.0));
        assert_eq!(status.current_file.as_deref(), Some("music/track.mp3"));
        backend
            .shutdown(CancellationToken::new())
            .await
            .expect("shutdown MPV");
        server.abort();
    }
}
