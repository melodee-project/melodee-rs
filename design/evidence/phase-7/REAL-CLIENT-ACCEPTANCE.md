# Phase 7 Real-client Acceptance

**Captured:** 2026-08-12

**Rust base commit:** `5c75673d5c8524cd2212aaa4d78bfad5bf081fcc` plus the pending Phase 7 change set

**Server fixture:** `apps/melodee-server/tests/native_client_fixture.rs`

The fixture creates a disposable PostgreSQL database and canonical-media root, imports one native
user with the streaming role, publishes two 30-second PCM/WAV Tracks, creates a two-item playlist,
starts the real configured native router, and prints sanitized connection facts. It owns and drops
its database and temporary media root. It does not read or alter an existing library.

## Android and Android Auto-mode

Client repository: `/home/steven/src/github/melodee-player`, commit
`7008eed187f42940b6614fe3438cf02fa5a07d94`.

Environment: Android 17/API 37 Pixel 9 Pro XL emulator; installed application version 1.8.0. The
client repository was clean after the run.

Client gate:

```text
ANDROID_HOME=<sdk> ANDROID_SDK_ROOT=<sdk> ./gradlew build
```

Result: `BUILD SUCCESSFUL` in 44 seconds.

The installed app was pointed at the disposable Rust fixture through emulator host alias
`10.0.2.2`. Manual phone-mode evidence covered:

- username/password login and bearer restoration;
- Artist/Release/Track/playlist browse and API search;
- HMAC direct stream start, duration, seek to 12.94 seconds, pause/resume, and background playback;
- `nowPlaying` and completed-play scrobble requests returning HTTP 200;
- force-stop/relaunch with `Authentication restored successfully` and the playlist still visible.

The reusable instrumentation source and clean-checkout runner are committed as
`compatibility/clients/android/Phase7RealServerAndroidAutoTest.kt` and
`scripts/run_android_auto_probe.py`. The runner verifies the pinned client commit and a
clean checkout, copies the probe into the external Android project for the build, supplies only the
disposable fixture URL/credentials as instrumentation arguments, and removes the copied source on
exit. It exercises the shipping exported MediaBrowser/MediaSession service rather than a mock
server or copied protocol implementation:

```text
MELODEE_ANDROID_CLIENT_BASE_URL=http://10.0.2.2:<fixture-port> \
  python3 scripts/run_android_auto_probe.py
```

Result:

```text
Time: 2.849
OK (1 test)
```

The probe proved root browse (`Playlists`, `Queue`), the two-Track playlist, API search, play-first,
next, previous, seek to 29.5 seconds, real ended-event auto-advance to the second Track, and
search/voice playback. Logcat showed Android Auto-mode `onSearch`, Rust HMAC stream URLs,
next/previous callbacks, ended-event advance, and restored authentication after process
recreation.

Limitation: the emulator contains Google's stub
`com.google.android.projection.gearhead` 1.2 package and no Desktop Head Unit. This validates the
current app's Android Auto MediaBrowser/MediaSession contract on an emulator, but it is not a
physical-head-unit or DHU certification result.

## MeloAmp browser and Electron

Client repository: `/home/steven/src/github/meloamp`, commit
`096c76d810af91205aeee54da58131766b0f076b`. The repository was not modified.

Client gate:

```text
yarn test --watchAll=false
yarn build
```

Result: 29 suites and 210 tests passed; the production build compiled successfully. Existing React
test `act(...)` warnings remained non-fatal.

The browser application ran against a disposable Rust fixture and produced:

```text
MELOAMP_REAL_CLIENT={"Albums":true,"Artists":true,"Auth":true,
"CredentialRestore":true,"Duration":true,"Playback":true,"Playlists":true,
"Search":true,"Seek":true,"Songs":true}
```

The actual Electron desktop shell was then launched with remote debugging against a second
disposable Rust fixture. `window.electron` and `window.meloampAPI` were both present. The renderer
passed clean-local-storage login, Artist/Release/Track/playlist browse, exact Track search, real
HTMLAudio playback from a Rust-signed HMAC URL, 30-second duration, seek to 12 seconds, reload, and
credential/playlist restoration.

The Electron main process repeatedly reported an existing client defect:

```text
ReferenceError: formatArtUrl is not defined
```

The error originates in MeloAmp's MPRIS album-art IPC. It did not stop renderer playback or any
HTTP workflow above. No workaround was added to Melodee and the external client repository was
left unchanged.

## Result

PRD-STREAM-003's Phase 7 native-client gate is satisfied for Android, the app's Android Auto-mode
media contract, and MeloAmp browser/Electron workflows. The physical-head-unit/DHU limitation and
the independent MeloAmp MPRIS client defect remain explicitly recorded rather than treated as
server behavior.
