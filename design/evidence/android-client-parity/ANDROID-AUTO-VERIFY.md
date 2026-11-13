# Melodee Android / Android Auto client — API parity verification

`melodee-player` is the reference Android client: Kotlin/Compose handheld UI plus a
Media3 `MediaBrowserServiceCompat` media session for Android Auto. Everything it can
ask the server for is declared in two Retrofit interfaces
(`data/api/MusicApi.kt`, `data/api/ScrobbleApi.kt`), so its server contract is small
and completely enumerable.

Driver: `scripts/verify_melodee_player_api.py` (`make verify-android-client`). It
issues exactly those requests against a running instance, asserts the fields the
app's Kotlin models declare **non-null**, and checks the behaviours the app
implements around them (token refresh, paging, ordering, artist filter, Range
requests, favourites, scrobbles).

## Result

**46/46 contract checks pass** for both an administrator account and a
listener-only account (`HasStreamRole`) on a 50-release catalogue:

| Request (app call site) | Verified |
|---|---|
| `POST auth/authenticate` | 200, `token`/`serverVersion`/`user`, user model fields |
| `GET system/info` | 200, `majorVersion`/`minorVersion` (the app gates login on these) |
| stale access token → `POST auth/refresh-token` → retry | 401 on the stale token, refresh returns a usable pair, new token authorizes `/user/me` |
| `GET user/me` | 200, user model fields |
| `GET user/playlists?page&limit` | 200, `{meta,data}`, playlist model incl. `durationMs`/`durationFormatted`/`songCount`/`owner` |
| `GET playlists/{id}/songs?page&pageSize` | 200, `{meta,data}` |
| `GET artists?q&orderBy&orderDirection` | 200; ascending vs descending lists differ (ordering honoured) |
| `GET artists/{id}/songs`, `artists/{id}/albums` | 200 |
| `GET albums/{id}/songs` (no paging params, as the app sends) | 200, `{meta,data}` |
| `GET search/songs?q=` | 200 with results; a nonsense query is an **empty page, not an error**; `filterByArtistApiKey` restricts results to that artist |
| `streamUrl` (issued by the server) | 200 with audio bytes; `Range: bytes=0-1023` honoured (ExoPlayer seeking) |
| `POST songs/starred/{id}/{value}` | 200 for both `true` and `false` |
| `POST scrobble` played + nowPlaying | 200 with the app's exact payload, including integer `scrobbleTypeValue` |

Model invariants checked against `domain/model/Models.kt`: `Song` (id, streamUrl,
title, artist, album, thumbnail/image URLs, durationMs, durationFormatted),
`Artist`, `Album` (incl. `releaseYear`), `Playlist`, `User`, and
`PaginationMetadata`. Nullable model members (`biography`, `genres`, `genre`) are
allowed to be `null`, as the app declares them.

No server-side defect was found: everything the Android client calls already behaves
as the frozen C# contract does. The MeloAmp pass earlier in the same cycle had already
fixed the defects these two clients share (artist artwork defaults, last-played
history feed, equalizer envelope alias, analytics plural type tolerance, scrobble
timestamp clamping).

One expectation was **corrected during this pass rather than "fixed"**: a rejected
bearer token returns an **empty `401`**, not the JSON error envelope. The frozen
captures show both behaviours and they are route-specific — `auth-user-*` and the
cookie-gated `themes-*` routes answer `401` with no body, while `endpoints-*` and
`stream/*` answer with `{code,message,correlationId}` where the controller itself
rejects. The app's `ErrorHandler` handles either; the probe now accepts the captured
behaviour instead of demanding one shape.

## On-device run (physical device)

The app was then driven on a **Pixel 9 Pro XL running Android API 37 (arm64)** with
the debug APK built from the client checkout, against a melodee-rs instance reached
through `adb reverse tcp:8080 tcp:8080` (so the device's `http://127.0.0.1:8080`
is the host server, and the stream URLs it issues resolve back through the same
tunnel — no public origin juggling, no client-address mismatch).

| Flow | Result |
|---|---|
| Sign-in against `http://127.0.0.1:8080` | `GET /system/info` 200, `POST /auth/authenticate` 200, home renders the account and its playlists |
| Home | playlists with song counts and durations ("Parity Playlist · 9 songs · 0:33:37") |
| Playlist detail | `GET /playlists/{id}/songs` 200; every cover loads (`/images/song_*/80` 200) |
| Search | `GET /search/songs?q=love` 200 → "Displaying 1 to 3 of 3 results" |
| Artist filter | `GET /artists?q=The` 200; selecting an artist re-filters via `search/songs?…&filterByArtistApiKey=…` 200 |
| Artist albums / album songs | `GET /artists/{id}/albums` 200, `GET /albums/{id}/songs` 200 (18 songs) |
| Playback | `GET /song/stream/…` **200 and 206** (ranged), position advances to 3:02 of a 3:27 track |
| Scrobble | `POST /scrobble` nowPlaying 200, then **played 200**; the play row lands in the database with the real duration (104 s) |
| Auto-advance | queue advanced to the next track and kept playing; the media session's custom action reflected the server's favourite state ("Add to Favorites" / "Remove from Favorites") |
| Queue | "Queue (9 songs)" listing the playlist in order |
| Transport clean-up | no unexpected non-2xx across the whole device session; the only failures were `HTTP FAILED: Canceled` image loads, which is OkHttp cancelling requests for recycled list rows |

The media session is live and exported for the car surface: `dumpsys media_session`
shows `com.melodee.autoplayer/Melodee` active, `state=PLAYING(3)`, full metadata
(title, artist, album), a media-button receiver and 10 controllers — this is exactly
what Android Auto binds to, and the manifest declares `<uses name="media"/>` with the
`MediaBrowserService` intent filter.

Not exercised: the Android Auto *car screen* itself. That needs a head unit or the
Desktop Head Unit; the browse tree it renders comes from the same calls verified
above and from the `MediaBrowserService` the session belongs to.

The device was left as found: the app's server URL was restored to the operator's
production instance and the app stopped (its session was logged out by the test, so
one sign-in is needed next time it is opened).

## Why the emulator could not be used

The workspace has the Android SDK, an AVD (`Pixel_9_Pro_XL`, `android-37`
Google-APIs Play-Store, 16 KB pages) and a working toolchain: `./gradlew
:app:assembleDebug` and `:app:testDebugUnitTest` both succeed. The emulator itself
cannot run here — it terminates with a segmentation fault during startup on every
configuration tried (`-gpu swiftshader_indirect`, `-gpu off -accel off`, with and
without snapshots):

```
INFO | Emulator is performing a full startup. This may take upto two minutes, or more.
Segmentation fault (core dumped)
```

`adb devices` therefore never lists a target, so neither the handheld UI nor the
Android Auto browse tree can be exercised on this host. Everything below the UI —
authentication, browse queries, playback URLs and byte ranges, favourites, and
scrobbles — is covered by the probe above, which is why the transport contract is
verified even though the car surface is not.

To close that gap, run the same probe first, then drive the app on a physical device
or a host where the emulator starts: install `app/build/outputs/apk/debug/app-debug.apk`,
point the login screen at the server (an emulator reaches the host at
`http://10.0.2.2:8080`), and watch `adb logcat` — the debug build logs every request
at `HttpLoggingInterceptor.Level.BASIC`, so any non-2xx answer is visible directly.
