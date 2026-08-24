# ADR-0156: MPD/MPV playback backend connectivity and lifecycle

- **Date:** 2026-08-24
- **Status:** Accepted
- **Decision owners:** Feature Completion Program Phase 0 (explicitly user-assigned)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-WORK-003, PRD-WORK-005, PRD-STREAM-001, PRD-STREAM-002,
  PRD-USER-004, PRD-OBS-001, PRD-AUTH-002

## Context

C# Melodee drives *local* playback through the `IPlaybackBackend` abstraction
(`src/Melodee.Common/Services/Playback/`) with two concrete backends selected by a string type in
`PlaybackBackendFactory.cs:39`: **MPD** (`MpdPlaybackBackend.cs`, a TCP socket to
`mpc`/`mpd` using the `clear`/`add "<path>"`/`play`/`pause 1|0`/`stop`/`seekcur {pos}`/`volume`
command protocol) and **MPV** (`MpvPlaybackBackend.cs`, an IPC JSON socket to an `mpv` process,
configurable path/audio device/extra args/socket path/initial volume). The factory caches a
singleton per type, reads `SettingRegistry` keys (`MpvPath`, `MpvAudioDevice`, `MpvExtraArgs`,
`MpvSocketPath`, `MpvInitialVolume`, `MpvEnableDebugOutput`, `MpdHost`, `MpdPort`, `MpdPassword`,
`MpdTimeoutMs`, `MpdInitialVolume`, `MpdEnableDebugOutput`), and degrades to `null` for unsupported
types. Backend capabilities (`CanPlay/CanPause/CanStop/CanSeek/CanSkip/CanSetVolume/CanReportPosition/IsAvailable`)
and the `MpdVersion` backend-info string feed the now-playing UI.

Rust currently implements only the *configuration surface* for local playback: the native
`/api/v1/playback/settings` route (`native/preferences.rs:113-127`) persists a `PlaybackSettings`
row (crossfade, gapless, volume normalization, replay gain, audio quality, equalizer preset,
last-used device) per user, and `native/discovery.rs:1027` seeds a static `MPV Backend`
`native_playback_endpoints` row with a fixed capabilities blob. There is **no live MPD/MPV control
loop**: no TCP/IPC client, no play/pause/seek/volume commands, no position/status polling, and no
per-backend lifecycle. PRD-WORK-003 (scheduled now-playing/session cleanup) and the user-facing
playback surface implied by the Blazor MusicPlayer route therefore have an unimplemented
local-playback half.

## Decision

1. **Backend abstraction.** A `PlaybackBackend` trait in the application layer (not the domain
   crate — it depends on `tokio`/`std::net`) with:
   - `capabilities() -> PlaybackCapabilities` (canPlay, canPause, canStop, canSeek, canSkip,
     canSetVolume, canReportPosition, isAvailable, backendInfo/version).
   - `play(file_path: &Path, song_api_key: Uuid, start_position_secs: f64)`, `play_queue_item(index: i32)`,
     `pause()`, `resume()`, `stop()`, `seek(position_secs: f64)`, `set_volume(0..=100)`,
     `status() -> PlaybackStatus` (state, position, duration, current file).
   - All methods take a `CancellationToken` and a bounded timeout.
2. **MPD backend.** A Rust `MpdBackend` that opens a single, lock-guarded TCP connection to
   `host:port` (settings `mpd_host`, `mpd_port`, `mpd_password`, `mpd_timeout_ms`,
   `mpd_initial_volume`) and speaks the mpd text protocol: `password` on auth, `clear`,
   `add "<path>"` (with path quoting), `play [index]`, `pause 1|0`, `stop`, `seekcur {pos}`,
   `volume {0..=100}`, `status`, `idletimeout`/`idle` for change notification (or a bounded poll
   loop if idle is unavailable). One connection per backend instance; reconnect with bounded retry
   on loss.
3. **MPV backend.** A Rust `MpvBackend` that speaks the MPV IPC JSON protocol over a Unix domain
   socket (`mpv_socket_path`) or named-pipe: commands `loadfile {path, start}` / `loadfile next`,
   `set volume`, `set pause 0|1`, `seek`, `get_property {position, duration, pause,
   filename, playtime}`. `mpv_path`, `mpv_audio_device`, `mpv_extra_args`, `mpv_initial_volume`,
   `mpv_enable_debug_output` settings preserved. The server **does not** launch the `mpv` process
   by default; it attaches to an already-running `mpv` IPC socket (operator-controlled lifecycle).
   An optional, configuration-gated "spawn mpv" mode is allowed but defaults to off.
4. **Configuration.** The existing layered settings service is the single source for backend type
   (`mpd` | `mpv` | `none`), connection params, and per-user playback settings. The
   `native_playback_settings` row (already persisted) stores per-user preferences (crossfade,
   gapless, volume normalization, replay gain, audio quality, equalizer preset, last-used
   device). The **backend selection and connection** are server-level settings; the **playback
   preferences** are per-user.
5. **Lifecycle.** Backend instances are created lazily on first use and cached in a `DashMap`
   (or `OnceCell`) keyed by backend type, mirroring the C# `GetOrCreateMpvBackendAsync` /
   `GetOrCreateMpdBackendAsync` singleton cache. They are `Arc`-wrapped so multiple concurrent
   requests can share one backend without blocking on a global lock (only the MPD socket and the
   MPV IPC stream are guarded by internal `tokio::sync::Mutex`). On shutdown, backends are
   dropped and connections closed cleanly.
6. **Error semantics.** A backend that is not connected (MPD TCP refused, MPV socket missing)
   reports `is_available = false` and `capabilities()` with all `can* = false`; play/pause/seek
   calls return a typed `BackendUnavailable` error (surfaced to the API as a `503` with the
   capability reason) rather than panicking or hanging. Connection timeouts are bounded by
   `mpd_timeout_ms` / a fixed MPV IPC deadline.
7. **State polling / now-playing.** The now-playing status (`position`, `duration`, `state`,
   `current file/song_api_key`) is obtained from `status()` (MPD `status` / MPV `get_property`)
   on each `GET /api/v1/now-playing` call, not cached across requests (the server does not
   maintain a local shadow of playback state; the backend is the source of truth). A background
   poller is **not** run; the API call triggers the status query (bounded by the connection
   timeout).
8. **Security.** `mpd_password` and any IPC socket path are treated as secrets (never logged,
   never in `external_provider_cache`); the MPV IPC socket is a filesystem resource whose
   permissions are the operator's responsibility. No backend control route is exposed to
   unauthenticated or non-owner principals.
9. **Native API routes.** New native-v1 routes (additive, not a change to existing routes):
   - `GET /api/v1/playback/backends` — list available backends with capabilities.
   - `GET /api/v1/playback/status` — current playback status (state, position, duration,
     current track).
   - `POST /api/v1/playback/play` — play a specific track (by `song_api_key`).
   - `POST /api/v1/playback/pause` / `POST /api/v1/playback/resume` /
     `POST /api/v1/playback/stop`.
   - `POST /api/v1/playback/seek` — seek to a position.
   - `POST /api/v1/playback/volume` — set volume (0-100).
   These routes are additive; the existing `GET/POST /api/v1/playback/settings` route is
   unchanged.

## Alternatives considered

### No local playback backends (server is a stream-only source)

Rejected by the user-assigned product direction: PRD-WORK-003 (the scheduled
now-playing/session-cleanup leg) and the Blazor MusicPlayer page imply server-side local
playback control for the MPD/MPV use case (a Melodee server on the same machine as the audio
output).

### Port the C# `IPlaybackBackend` class hierarchy

Rejected per the ground-up rewrite rule: the C# class structure (static helpers, `Task`-based
async, `IDisposable`) does not translate to idiomatic Rust; we preserve the *command protocol*
and *capability surface*, not the class layout.

### A single `PlaybackManager` actor (mailbox)

Rejected: an actor adds a serialization bottleneck for concurrent play/pause/status calls. A
lock-guarded connection per backend (with `tokio::sync::Mutex` on the socket) is simpler and
sufficient for the expected concurrency (one active playback at a time per backend).

## Rationale

The C# MPD/MPV backends are thin protocol wrappers over TCP/IPC sockets with no proprietary
dependency; a Rust reimplementation is a bounded, well-scoped workstream. The singleton cache and
settings-driven configuration are preserved to keep the operator's existing MPD/MPV setup
working. The additive API routes keep the existing playback-settings surface intact.

## Consequences

### Positive

- Closes the local-playback half of the now-playing/scheduled-cleanup surface (PRD-WORK-003) and
  the user-facing player surface.
- Operator's existing MPD/MPV configuration (settings keys) works without change.
- No new runtime dependency (mpd/mpv are external processes; the server only speaks their
  protocol).

### Negative / trade-offs

- A live TCP/IPC connection adds an external dependency (mpd/mpv must be running and
  network-reachable); fail-degraded to `is_available = false` when not.
- The server now holds a long-lived socket; shutdown hygiene and reconnect logic add
  operational surface.

### Follow-up work

- Feature Completion Phase 2 (PLAYBACK): `MpdBackend` and `MpvBackend` with protocol
  conformance tests against a mock MPD server and a real MPV IPC socket fixture; lifecycle
  tests (connect, reconnect, shutdown); API route tests; now-playing integration.
- Real MPD/MPV acceptance legs require an operator-controlled environment and are deferred
  to that phase with documented authorization.

## Validation

- `MpdBackend` protocol test: a mock MPD TCP server (tokio `TcpListener`) accepts `password`,
  `clear`, `add`, `play`, `pause`, `stop`, `seekcur`, `volume`, `status` in sequence; the
  backend reports the expected `PlaybackStatus` from `status` output.
- `MpvBackend` IPC test: a Unix socket mock (or `mpv --input-ipc-server` in a test container)
  accepts `loadfile`, `set pause`, `get_property`; the backend reports position/duration.
- Fail-degraded test: backend with unreachable MPD host reports `is_available = false` and
  play returns `BackendUnavailable` (503).
- API route tests: each new route returns the expected shape for a healthy backend and
  `503` for an unavailable one.
- No existing native route/DTO/status changes (golden diff).

## References

- [PRD.md](../../PRD.md) §22 (PRD-STREAM-001/002), §23 (PRD-WORK-003/005), §27 (PRD-USER-004),
  §45 (PRD-WORK-003/005)
- [ADR-0139](ADR-0139-admin-spa-session-strategy-and-admin-api-gating.md) (auth gating for new routes)
- `src/Melodee.Common/Services/Playback/` (C# legacy backends, behavioral oracle)
