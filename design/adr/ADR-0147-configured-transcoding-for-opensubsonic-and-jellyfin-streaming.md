# ADR-0147: Configured transcoding for OpenSubsonic and Jellyfin streaming

- **Date:** 2026-08-16
- **Status:** Accepted
- **Decision owners:** R2.3 configured-transcoding work package (assigned product decision to
  replace silent-ignoring of format/bitrate/transcode parameters with configured behavior)
- **Supersedes:** ADR-0130 decision 2 (only the clause "transcode parameters remain silently
  ignored (captured: full original bytes at 200)") and ADR-0130 decision 4 (only the sentence
  "There is no transcoding: `maxBitRate`/`format` are silently ignored, matching the captured
  baseline."). Every other ADR-0130 decision stands unchanged, including the byte-for-byte frozen
  extension advertisement, the unconsulted `apiKey` parameter, correct single-pass range
  handling, and the captured 404 + error 70 failure envelope class.
- **Superseded by:** None
- **Related requirements:** PRD-STREAM-002 ("Configured transcoding paths"), PRD-MEDIA-003,
  PRD-API-OS-001..007; ADR-0102 (separate adapters), ADR-0108 (external FFmpeg tooling),
  ADR-0130 (partially superseded as scoped above)

## Context

Phase 8 froze the legacy baseline on all three streaming surfaces:

- **OpenSubsonic** `stream.view`/`download.view` silently ignore `maxBitRate` and `format`,
  always returning full original bytes (ADR-0130 decisions 2 and 4; captured in the
  `os-stream-*` goldens).
- **Jellyfin** `Audio/{id}/stream[.{container}]` rejects any transcoding request —
  `static != true` combined with a non-empty `container` or `audioBitRate` — with HTTP 400
  "Transcoding is not supported. Use static=true for direct streaming." (frozen golden
  `compatibility/jellyfin/golden/playback/jf-playback-stream-extension-no-static.json`).
- **Native v1** accepts no format/bitrate parameters and is unaffected.

The legacy server never transcoded at all: ADR-0108's Phase 0 ratification recorded that the
seeded streaming-transcode command templates (`transcoding.command.aac/mp3/opus`) are malformed
(unbalanced quotes in the seed literals) and unconsumed by any legacy code path. PRD-STREAM-002
nevertheless requires "Configured transcoding paths", and the R2.3 work package assigns the
replacement of silent-ignoring with configured behavior as a product decision. There is
therefore no legacy transcode-output contract to preserve; the frozen contracts being changed
are (a) the OS silent-ignore/full-bytes response when `format`/`maxBitRate` are present and
(b) the Jellyfin 400 rejection. Both changes follow the compatibility-rule process: existing
behavior is captured (goldens above), the product decision is the R2.3 assignment, this ADR is
the accepted decision record, and the migration/client plan is in Consequences.

`timeOffset` is explicitly out of scope: the captured contract answers 404 + error 70 when it
is present, that rejection is not silent-ignoring, and the frozen `transcodeOffset[1]`
extension advertisement remains a preserved false claim under the unchanged part of ADR-0130
decision 2.

## Decision

1. **A shared transcoding service owns all FFmpeg streaming execution.** It lives behind a
   typed interface (per ADR-0108), builds an argv array (never a shell), launches FFmpeg with
   `tokio::process` and piped stdout as the response body, captures bounded stderr for
   sanitized diagnostics, and kills and reaps the child on client disconnect, response
   completion, or server shutdown. No wall-clock timeout is imposed on an established
   transcode stream; cancellation is the bound (a stream is inherently bounded by track
   duration and client consumption).
2. **Configuration reuses the existing seeded settings surface.** `transcoding.command.mp3`,
   `transcoding.command.aac`, and `transcoding.command.opus` hold JSON template objects
   `{ "format": ..., "bitrate": ..., "command": ... }`; the `command` string is split into an
   argv array (shell-words rules, no shell invocation) with placeholder substitution: `%s` →
   source file path, `%t` → start offset in seconds (always `0` today; `timeOffset` stays
   rejected), `%bk` → target bitrate in kbps. `transcoding.default` remains `raw` and is not
   consulted by adapter routes (every transcode is explicitly requested by a route parameter).
   The malformed seed defaults are corrected to parseable templates; an unparseable or
   non-substitutable template fails closed (request rejected, sanitized log).
   *(Amended 2026-08-16 during R2.3 implementation: `%s` substitutes the input specifier
   `pipe:0`, not a filesystem path — the validated, no-follow-opened asset file
   (`CanonicalAssetRoot::open_asset`, catalog size-checked) is passed as the FFmpeg child's
   stdin. This preserves the dirfd race-resistance guarantee end to end: no joined path is
   ever re-opened by name, so no symlink/TOCTOU window exists between validation and
   decode. Shipped default templates are pipe-safe (mp3/aac adts/ogg-opus demux and encode
   all operate on non-seekable streams); operator templates requiring seekable input are
   unsupported by this service.)*
3. **OpenSubsonic `stream.view`/`download.view` honor `format` and `maxBitRate`:**
   - `format` absent or `raw`, and no bitrate-driven transcode (below) → unchanged direct
     play, including all frozen headers, range behavior, and the 404 + error 70 failure class.
   - `format=mp3` (or `aac`/`opus` with a configured parseable template) → transcode at
     `min(maxBitRate, template bitrate)` when `maxBitRate > 0`, else the template bitrate.
   - `maxBitRate` alone (> 0) → transcode with the mp3 template at `maxBitRate` kbps only
     when the source bitrate is unknown (lossless) or exceeds `maxBitRate`; otherwise direct
     play.
   - Any other `format` value, or a requested format whose template is missing/unparseable →
     the standard OpenSubsonic error envelope, code 0 (generic), HTTP 200 — a new shape for a
     new failure class, recorded here.
   - `timeOffset` present → unchanged captured 404 + error 70.
   - Transcoded responses: HTTP 200, `Content-Type` `audio/mpeg` / `audio/aac` / `audio/ogg`,
     no `Content-Length`, no `Content-Range` (Range headers are ignored on transcoded
     requests), no `ETag`, the same no-store cache/CORS/`Content-Duration` headers as direct
     play. Transcoded `download.view` keeps `Content-Disposition: attachment` with the source
     basename's extension replaced by the target format's.
4. **Jellyfin `Audio/{id}/stream[.{container}]` honors transcoding requests:** `static != true`
   with `container` (extension or query) or `audioBitRate` now transcodes instead of returning
   the retired 400. `container` maps `mp3`/`aac`/`opus` to the corresponding template;
   `audioBitRate` (bits per second, Jellyfin convention) overrides the template bitrate as
   `audioBitRate / 1000` kbps; a missing `container` defaults to mp3. An unsupported
   `container` → 400 "Unsupported transcoding container." (new message; the retired message is
   not reused). `static=true` or `/universal` (which forces `static=true`) → unchanged
   captured direct play, including the on-disk-existence-check ordering. HEAD on a transcode
   request returns headers only and spawns no FFmpeg process.
5. **Transcoding concurrency is bounded independently of direct play.** A new setting
   `transcoding.maxConcurrent` (Integer, default 4) sizes a dedicated semaphore; acquisition
   is non-blocking — exhaustion rejects the request immediately (OpenSubsonic: error 0
   envelope at HTTP 200; Jellyfin: HTTP 503 problem detail). Direct-play capacity accounting
   (`maxConcurrentStreams`) is unchanged and does not meter transcoded streams.
6. **Native v1 is unchanged.** It exposes no format/bitrate parameters; its direct-play,
   range, capacity, and disconnect behavior is untouched.
7. **Device profiles remain metadata-only.** Transcoding decisions consult only request
   parameters and the `transcoding.*` settings; the Phase 9 device-profile surfaces are not
   wired into execution.

## Alternatives considered

### Keep silent-ignoring permanently (ADR-0130 status quo)

Rejected: PRD-STREAM-002 lists configured transcoding as required streaming behavior, and the
R2.3 work package assigns it. The frozen baseline was evidence of a legacy gap (ADR-0108:
templates malformed/unconsumed), not a desirable contract; every real Subsonic/Jellyfin server
honors these parameters, so clients already handle transcoded responses.

### Advertise `transcodeOffset` truthfully by implementing `timeOffset`

Rejected for this increment: `timeOffset` has a captured rejection contract (404 + error 70)
that is not silent-ignoring, and no inventoried client evidence requires seek-into-transcode.
The `%t` placeholder exists in the template grammar so a future ADR can enable it without
changing configuration shape.

### Queue transcode requests when the semaphore is full instead of rejecting

Rejected: an unbounded or deep queue couples slow clients to FFmpeg processes holding memory
and decode CPU, violating the bounded-concurrency/backpressure standard. Immediate rejection
keeps resource use explicit; clients already retry or fall back to direct play parameters.

## Rationale

The work package requires configured transcoding with the least contract churn. Reusing the
seeded `transcoding.command.*`/`transcoding.default` settings (ADR-0108's designated mechanism)
avoids inventing parallel configuration; correcting the malformed seeds is safe because no
legacy code consumed them. Limiting behavioral change to requests that explicitly ask for a
format/bitrate keeps every captured direct-play golden byte-identical, so the differential
suite continues to guard the unchanged surface. A single shared service keeps FFmpeg process
management, cancellation, and bounding identical across adapters while each adapter keeps its
own wire shapes (ADR-0102).

## Consequences

### Positive

- PRD-STREAM-002's "Configured transcoding paths" leg becomes implementable and closable.
- OpenSubsonic clients requesting `format=mp3`/`maxBitRate` (mobile bandwidth controls) receive
  real MP3 at the requested bitrate instead of silently receiving full original bytes.
- Jellyfin clients requesting transcoding succeed instead of hard-failing with 400.
- FFmpeg execution gains typed argv construction, bounded stderr diagnostics, independent
  concurrency bounding, and guaranteed kill-and-reap on disconnect/shutdown.

### Negative / trade-offs

- Two frozen behaviors change: OS `format`/`maxBitRate` requests no longer return full original
  bytes, and the Jellyfin no-static 400 is retired. Mitigation: both changes are gated on
  parameters clients only send when requesting transcoding; every parameterless/direct-play
  capture is unchanged and still differentially tested.
- Transcoded responses are unseekable (no Range/Content-Length), matching standard Subsonic
  server behavior but differing from this server's direct-play richness.
- Corrected template seeds change the settings rows new installations see; databases already
  carrying the malformed legacy strings fail closed with a sanitized error until the setting is
  updated (documented in the implementation's data/migration note).

### Migration / client plan (compatibility-rule requirement)

- **OpenSubsonic:** clients send `format`/`maxBitRate` only to request transcoding, and every
  reference Subsonic server answers with transcoded bytes, so client decode paths already
  exist. The `os-stream-*` direct-play goldens (no transcode parameters) are untouched and
  remain in the differential set.
- **Jellyfin:** the golden `jf-playback-stream-extension-no-static.json` is retained on disk as
  historical evidence but removed from active differential expectations, with its scenario
  entry annotated to reference this ADR. `static=true` and `/universal` behavior is unchanged.
- **New fixtures:** transcode success/failure responses are recorded as Rust-behavior fixtures
  (no legacy oracle exists — ADR-0108); they are labeled as such, not differential captures.

### Follow-up work

- R2.3 evidence section in `design/evidence/r2/README.md`; requirement traceability update for
  PRD-STREAM-002 once tests land.
- `compatibility/COVERAGE-MATRIX.md` D8 note updated to point at this ADR for the transcode
  parameter clause.
- User documentation (`/docs`) streaming/transcoding pages updated in the same change as the
  implementation.

## Validation

- Route-level Rust tests per adapter: transcode success bytes and headers, unsupported format/
  container rejection, template-parse failure fails closed, semaphore saturation rejection,
  client-disconnect kills the FFmpeg child (process reaped), HEAD spawns no process.
- Integration test with real FFmpeg (available per ADR-0108 toolchain note): a tiny generated
  WAV transcoded to MP3 produces `audio/mpeg` bytes that ffprobe re-parses.
- Differential replay of the existing `os-stream-*`, Jellyfin `static=true`, and native stream
  goldens shows zero drift (direct-play regression guard).
- The retired Jellyfin golden is removed from active expectations by explicit scenario
  annotation, not deletion.

## References

- [PRD.md](../../PRD.md) (PRD-STREAM-002, PRD-MEDIA-003)
- [SPEC.md](../../SPEC.md) §19.2, §20, §24
- [ADR-0108](ADR-0108-use-ffmpeg-ffprobe-and-chromaprint-tools.md)
- [ADR-0130](ADR-0130-phase-8-opensubsonic-authentication-extension-advertisement-and-streaming-corrections.md)
- Retired-behavior evidence: `compatibility/jellyfin/golden/playback/jf-playback-stream-extension-no-static.json`
