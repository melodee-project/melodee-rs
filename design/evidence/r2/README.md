# R2 — Mandatory Runtime Implementation Gaps

Date: 2026-08-16. Status: In progress. Depends on: R1 (`2c54be5`). Scope per
`IMPLMENTATION_OUTSTANDING.md` §7: R2.1 media-format matrix, R2.2 CUE/sidecar, R2.3 configured
transcoding, R2.4 bounded resource classes, R2.5 observability/health, R2.6 scheduled/provider
behavior, R2.7 performance/fault evidence — plus the five bounded-input fixes assigned to R2 by
the R1.3 security baseline inventory (R2-SEC-1..5).

## 1. R2-SEC — Security-inventory bounded-input fixes

Source: [`../r1/security-baseline-inventory.md`](../r1/security-baseline-inventory.md) follow-up
table. Four fixes implemented; one rejected on compatibility grounds. No external contract
changed: each fix tightens only server-internal handling on paths whose captured behavior is
preserved byte-for-byte.

### 1.1 Dispositions

| ID | Inventory finding | Disposition | Proof |
|---|---|---|---|
| R2-SEC-1 | Last.fm 256 KiB response cap was post-read (`bytes()` buffered the full body before the limit check) | **Fixed.** `lastfm.rs::parse_response` now rejects early on a declared `Content-Length` over `MAX_RESPONSE_BYTES` and otherwise enforces the cap per chunk while streaming `bytes_stream()`, so a hostile or proxied endpoint cannot force an unbounded pre-check allocation | `tests/lastfm_scrobble.rs::oversized_chunked_response_is_capped_while_streaming` — mock endpoint returns a chunked body (no `Content-Length`) of 40×8192 bytes; the client must surface `MalformedResponse` without buffering it. Suite: 10 passed |
| R2-SEC-2 | ffprobe argv lacked the `--` separator fpcalc carries | **Fixed.** Pure `ffprobe_args(path)` in `crates/melodee-media/src/lib.rs` appends `--` before the path operand (defense in depth; all current paths are absolute, but the separator keeps the operand interpretation if that invariant is ever relaxed) | Unit test `ffprobe_argv_separates_options_from_the_media_path` asserts the separator position and sole trailing operand. Suite: 3 passed |
| R2-SEC-3 | Playlist-image upload does not enforce declared Content-Type equality with the sniffed type (artwork/avatar uploads do) | **Rejected — no wire change.** Rationale in §1.2 | Compatibility review below |
| R2-SEC-4 | OPML import had no explicit route-level body limit (manual 2 MiB check + axum implicit default fallback) | **Fixed.** `native/podcasts.rs` pins `DefaultBodyLimit::max(MAX_OPML_BYTES)` (2 MiB) on `POST /api/v1/podcasts/opml/import`, making the effective bound explicit at the route instead of relying on axum's implicit 2 MiB default coinciding with the manual check. No observable contract change: the effective limit was already 2 MiB | Oversize assertion (2 MiB + 1 byte → 413) added to `tests/native_podcasts_api.rs::all_native_podcast_routes_are_tenant_scoped_bounded_and_authorized`. Suite: 3 passed |
| R2-SEC-5 | Canonical artwork serve path verified only the recorded byte size, so an equal-length content swap would go undetected | **Fixed.** `native/images.rs::canonical_artwork_bytes` now verifies the BLAKE3 hash of the served bytes against the recorded publication `byte_hash`; mismatch logs a warning and falls back to the captured no-artwork placeholder (the frozen 404/placeholder behavior) instead of serving unverified bytes | `tests/admin_artwork_api.rs::canonical_artwork_with_corrupted_storage_falls_back_to_the_captured_placeholder` — seeds a real artwork asset, corrupts one byte same-length in a tempdir canonical root, asserts the placeholder is served. Suite: 6 passed |

Dependency note: R2-SEC-5 adds `blake3` to `apps/melodee-server`'s dependency list. BLAKE3 is
already a pinned workspace dependency (exact `=` version, committed `Cargo.lock`) used by the
domain crate; no new third-party crate enters the tree, so no ADR or dependency decision is
required.

### 1.2 R2-SEC-3 rejection rationale (compatibility review)

The inventory proposed enforcing equality between the client-declared `Content-Type` and the
sniffed image type on the native playlist-image upload (`native/playlists.rs`), matching the
artwork/avatar upload validation. The change is rejected for release 1.0 on compatibility
grounds:

1. **The route is frozen.** `POST /api/v1/playlists/{id}/image` is native API v1 route matrix id
   104 (Phase 7 acceptance). The compatibility rules forbid changing any existing external
   route's validation behavior without (a) a compatibility test showing current behavior, (b) a
   product decision, (c) an accepted ADR, and (d) an explicit migration/client plan. None exist
   for this route's Content-Type handling.
2. **Current behavior is not fully captured.** The frozen capture set covers only the empty-body
   scenario for this route. The legacy server's response to a declared-vs-sniffed mismatch is not
   documented by any accepted capture, so adding a rejection would invent new observable
   behavior rather than preserve known behavior — exactly what the frozen-contract rule exists
   to prevent.
3. **The underlying risk is already contained server-side.** Uploaded bytes are not served
   back as-is: the stored representation is canonicalized through the bounded decode/re-encode
   image pipeline, and the served `Content-Type` derives from that canonical stored form, not
   from the client-declared header. A lying `Content-Type` therefore cannot smuggle unsniffed
   bytes to a downstream client. The route already performs magic-byte sniffing (R1.3 §1.3
   inventory row), which is the control that carries the actual risk.

Disposition: rejected; no wire change; this record satisfies the backlog requirement that
security-inventory follow-ups end in either a fix or an explicit accepted decision. PRD-SEC-003
remains `planned` with owner R5, whose review scope includes this disposition.

## 2. R2.1 — Content-based media detection (extension-mismatch closure)

Work package: `IMPLMENTATION_OUTSTANDING.md` §7.2 — "Detect media from probed content/streams
rather than trusting only filename extension."

### 2.1 Verification result

Audit of the detection path against the frozen legacy corpus case
`compatibility/media/cases/not-mp3-named-mp3.json` (WAV content in `.mp3` filenames; expected
`conversion: mp3 despite extension`):

| Detection surface | Before this increment | Content-based? |
|---|---|---|
| Retention vs. conversion (`normalize.rs::is_retainable_mp3`) | Decided from probed `codec` + `container` + audio-stream count | Already content-based ✓ |
| Technical observation (`AudioTechnicalObservation`) | All fields deserialized from the ffprobe stream/format document | Already content-based ✓ |
| File-level `mime_type` (`ObservedFile`) | **Extension-derived only.** `ProbeResult.mime_type` was hardcoded `None`, so the fill-from-probe branch in `inspect_file` was dead code; a WAV-content `.mp3` was recorded as `audio/mpeg` | **Gap — fixed below** |

### 2.2 Fix

- `run_ffprobe_cancellable` now derives `ProbeResult.mime_type` from the probed container via
  `mime_for_container`, which is deliberately restricted to the MIME vocabulary
  `mime_for_path` already establishes (mp3, flac, ogg, aac, ac3, wav, asf→wma, mov/mp4/m4a
  family). Unmapped containers (ape, wv, …) yield `None` — content detection never invents new
  MIME values.
- `inspect_file`: a probed container MIME overrides the extension-derived guess; when both exist
  and disagree, a Warning finding `media.extension_mismatch` is recorded with
  `{extension_mime_type, probed_mime_type, container, codec}` evidence. Classification
  (`ObservedFileKind`) is unchanged — it is the extension vocabulary deciding *which* files are
  probed; the *detection* record now reflects probed content.
- No external contract touched: the manifest is the rewrite's internal observation document;
  catalog media-asset MIME derives from the verified canonical (MP3) output, and no frozen route
  consumes the observation field.

### 2.3 Compatibility alignment

The legacy capture stays frozen (exitCode 1 tags defect, zero albums under the legacy
metadata-container gate F1). Its `expected.conversion = "mp3 despite extension"` intent is now
executable in the rewrite:
`tests/normalization_real.rs::wav_content_in_mp3_named_files_is_detected_from_content_and_converted`
generates WAV content in the case's three `.mp3` filenames, asserts inspection records
`container=wav`, `codec=pcm_s16le`, `mime_type=audio/wav` plus the mismatch finding, then
normalizes and asserts all three outputs are non-retained, verified MP3 at the canonical sample
rate with the source tree byte-identical.

### 2.4 Tests and gate (R2.1 increment)

- `cargo test -p melodee-media --lib` — 31 passed, including the pre-existing format matrix
  test (probed container MIME agrees with the extension vocabulary for every standard format, so
  no mismatch findings fire on well-named media) and the new
  `probed_container_maps_to_the_established_mime_vocabulary` unit test.
- `cargo test -p melodee-media --test normalization_real` — 6 passed, including the new
  content-detection/conversion test.
- `./scripts/check-smoke.sh` at the commit boundary.

Documentation impact:
- None for `/docs` — the change corrects an internal observation field and adds an internal
  manifest finding code; public docs already describe inspection as obtaining technical facts
  from ffprobe and do not enumerate finding codes. Internal: this report.

## 3. R2.1 — Format-matrix completion and pathology fixtures

Second R2.1 increment: make the Phase-3 media matrix executable per format and per failure
mode, and close the remaining fixture gaps named by `IMPLMENTATION_OUTSTANDING.md` §7.2
("malformed, truncated, zero-length, unsupported-codec, oversized-tag, missing-duration, and
multi-stream fixtures").

### 3.1 Format matrix

- The lib matrix test is extended to nine rows (WAV added) and renamed
  `ffprobe_format_matrix_covers_the_required_format_vocabulary`; every row now asserts the
  probed codec, observed duration (present and plausible for the 0.2 s fixture), stream count,
  and content-derived MIME — detection, stream properties, and duration are exercised per
  format, with native metadata asserted where the container carries tags.
- `design/evidence/phase-3/media-inspection-and-manifest/media-matrix.json` is updated in place
  to name the executable evidence per row and per invalid case. APE's previously implied claim
  is replaced with an explicit limitation record: the bundled toolchain ships an APE decoder but
  no encoder (`ffmpeg -encoders` lists none), so no synthetic fixture can be generated and none
  is committed under the R2.1 distributability rule. The limitation is recorded, not silently
  skipped; a real APE file would route through the standard ffmpeg decode-to-MP3 normalization
  path.

### 3.2 Pathology fixtures (all in `tests/normalization_real.rs` unless noted)

| R2.1 fixture | Behavior proven | Test |
|---|---|---|
| Malformed | `media.ffprobe_failed` finding; typed normalization failure | `corrupt_audio_is_rejected_by_normalization` (pre-existing) |
| Truncated | STREAMINFO probes full declared duration; normalization fails (`Ffmpeg`/`Verification`), nothing published, source unchanged | `truncated_audio_fails_normalization_without_publishing` |
| Zero-length | New `media.empty_file` Error finding at inspection (ffprobe's raw demuxer still returns a degenerate no-duration observation for empty `.flac`; the observation records that truth); ffmpeg refuses the encode | `zero_length_audio_is_an_actionable_finding_and_blocks_normalization` |
| Multi-stream | True stream count observed; typed `Verification` rejection — Melodee never guesses which stream is the music | `multi_stream_audio_is_observed_and_rejected_actionably` |
| Oversized tag | Embedded artwork over the 16 MiB observation bound is skipped with `artwork.embedded_oversized`; audio unaffected | `oversized_embedded_artwork_is_bounded_and_recorded` |
| Unsupported content | Video-only container under an audio extension → `media.ffprobe_failed` + `unsupported_or_corrupt_audio`; normalization rejects | `video_only_container_is_recorded_unsupported_and_rejected` |
| Missing duration | Not deterministically synthesizable on disk with bundled ffprobe; failure mode pinned at unit level as `MissingField("source duration")` | `normalize.rs::missing_source_duration_is_a_typed_verification_failure` |

New finding code `media.empty_file` (Error) is the only production-code addition: a zero-byte
file can never carry media, and flagging it at inspection is more actionable than the
downstream encode failure alone.

### 3.3 Remaining format rows and the large-file streaming rule

- Per-format normalization-input rows for AAC (ADTS), AC-3, WMA, and Opus:
  `normalization_real.rs::aac_ac3_wma_and_opus_inputs_normalize_to_verified_mp3` generates each
  with the bundled toolchain, asserts the probed observation, and proves verified canonical MP3
  conversion (non-retained, 48 kHz) with the source tree byte-identical. Together with the
  pre-existing matrix (FLAC/Ogg/WAV/M4A/MP3-retained/multichannel), every synthesizable format
  in the §7.2 vocabulary now has detection **and** normalization-input evidence.
- Large-file streaming: `hash_file` hashes through a fixed 128 KiB buffer;
  `lib.rs::hashing_streams_large_files_in_bounded_chunks` proves multi-chunk operation on a
  sparse 256 MiB file (2048 chunks) against a streamed reference. Probing runs through
  subprocess ffprobe/fpcalc pipelines with bounded captured output; no media path materializes a
  whole file. Production audit: the only whole-file `fs::read` in `melodee-media` is artwork
  header inspection, reachable only after the `max_artwork_bytes` (16 MiB) size gate — bounded,
  and not a media path.
- `PRD-MEDIA-001` is closed (`tested`) per the R1 disposition map with this evidence;
  requirement artifacts regenerated with `extract_requirements.py`.

### 3.4 Tests and gate (matrix increment)

- `cargo test -p melodee-media` — 33 lib + 12 integration passed, zero failures.
- `python3 scripts/extract_requirements.py --check` — index current (239 requirements).
- `./scripts/check-smoke.sh` at the commit boundary.

Documentation impact:
- None for `/docs` — all additions are internal test/evidence artifacts plus one internal
  inspection finding code; no documented user-visible behavior changes. Internal: this report
  and the media-matrix artifact.

## 4. Tests and gate (R2-SEC increment)

- `cargo test -p melodee-server --test lastfm_scrobble` — 10 passed.
- `cargo test -p melodee-media --lib` ffprobe argv tests — 3 passed.
- `cargo test -p melodee-server --test native_podcasts_api` — 3 passed.
- `cargo test -p melodee-server --test admin_artwork_api` — 6 passed.
- `./scripts/check-smoke.sh` at the commit boundary (fmt, clippy `-D warnings`, workspace lib
  tests, doc/requirement validation).

Documentation impact:
- None for `/docs` — all four fixes are server-internal hardening with no user-visible behavior
  change; the R2-SEC-3 disposition preserves the documented frozen contract. Internal: this
  report and the evidence registry index.

## 5. R2.2 — CUE/sidecar behavior (ADR-0146)

Work package: `IMPLMENTATION_OUTSTANDING.md` §7.3 — "CUE/sidecar behavior; fixtures for CUE
sheets (UTF-8 and legacy encodings; malformed/missing timestamps; multiple files; pregaps) and
NFO/M3U/SFV/LRC sidecars (parse, ignore, reject behavior)". Governing decision:
`design/adr/ADR-0146-cue-driven-track-segmentation-in-normalization.md` (Accepted), which
ratifies the SPEC §5.5 sidecar-taxonomy amendment and the §6.4/§8.2 normalization output
profile v2 (profile `2`).

### 5.1 Legacy oracle alignment

Frozen Phase 0 capture `compatibility/media/cases/cue-split.json` records the legacy behavior:
one WAV plus one CUE sheet yields one album of per-track MP3 segments ("Albums Processed: 1,
New Songs: 3"; staging tree `0001 First Light.mp3`, `0002 Second Wave.mp3`,
`0003 Third Echo.mp3`). The capture stays frozen (its `exitCode 1` tag defect and
`Valid Albums: 0` outcome are recorded legacy defects, not behavior to preserve). Its
sidecar intent — `cue -> ffmpeg split per track` — is now executable in the rewrite:
`tests/normalization_real.rs::cue_governed_source_splits_into_verified_per_track_segments`
builds one 6 s WAV plus a 3-track CUE with the capture's album/track titles, then proves the
ADR-0146 contract end to end with real FFmpeg:

- one verified MP3 segment per CUE track (3 outputs, 48 kHz, each within 300 ms of its
  two-second boundary), and the governed source's whole-file output suppressed;
- deterministic segment identity `album.wav#cue-track-01/02/03` in both `track_key()` and the
  work-area output paths, byte-identical across a fresh attempt id (retry safety);
- manifest Tracks replaced per segment with sheet-derived observations (titles, track numbers,
  track total, album, album artist, and a track-level performer override), joined on the
  segment pseudo-path — the inspection-time whole-file Track is gone;
- segment metadata tags written from the sheet and verified with ffprobe (including a Unicode
  title and a quoted FILE with spaces);
- the governing sheet retained as a parsed sidecar observation, and the staged source tree
  byte-identical after normalization — segments are never split in place.

### 5.2 Parser and governance unit evidence (`normalize.rs` unit tests)

| §7.3 dimension | Test | Behavior proven |
|---|---|---|
| Timestamps at full 75 fps precision | `cue_timestamps_parse_at_full_frame_precision` | `mm:ss:ff` → milliseconds against the exact frame duration |
| First-INDEX-wins, pregaps/postgaps | `cue_model_first_index_wins_and_pregaps_are_unapplied` | INDEX 00/02+ never move the segment start (pregap semantics) |
| Multiple FILE references | `cue_model_collects_every_file_reference`, `cue_governance_rejects_multi_file_traversal_and_missing_sources` | multi-FILE sheets produce findings and never govern; path traversal and missing referenced audio reject with findings |
| Malformed/missing timestamps, ordering, range | `cue_governance_rejects_unordered_and_out_of_range_boundaries` | unordered or out-of-range boundaries are findings, not silent clamps |
| Legacy encodings | `windows_1252_sidecar_fallback_is_structured` | non-UTF-8 bytes decode through the accepted windows-1252 fallback with the fallback recorded |
| Malformed sidecars of every type | `malformed_sidecars_produce_findings` | garbage NFO/M3U/SFV/LRC/CUE content yields structured findings, never a panic or silent drop |

### 5.3 NFO/M3U/SFV/LRC parity positioning

Per the SPEC §5.5 amendment ratified by ADR-0146: these types are observed sidecars with parsed
semantics (M3U EXTINF, SFV paths, NFO type, LRC multi-timestamp) recorded on the manifest as
`ParsedSidecar` observations with structured findings; they never govern segmentation. CUE is
the only governing sidecar, matching the frozen corpus (no legacy capture shows NFO/M3U/SFV/LRC
influencing outputs). The pre-existing five-type real-file sidecar matrix and two-run
determinism coverage in
`tests/normalization_real.rs::real_format_matrix_is_verified_source_safe_and_deterministic`
continues to prove the observation path for all five types against real files.

### 5.4 Deliberate deviations and data impact (per ADR-0146)

- MP3 direct-retention is suppressed for CUE-governed sources only; all other sources retain
  unchanged. Recorded in the ADR, not a silent behavioral fork.
- Validation profile `2` supersedes profile `1` (CUE model semantics: no pregap application,
  no whole-file outputs for governed sources, catalog join on `track_key()`).
- An open migration catalogues pre-existing legacy split rows (`track_key IS NULL` media
  assets whose sibling tracks carry `#cue-track-NN` paths) with per-run findings; it rewrites
  nothing.
- No legacy staging-name scheme (`0001 Title.mp3`) is reproduced; canonical identity is the
  segment pseudo-path, because staging names embed legacy validation defects.

### 5.5 Tests and gate (R2.2 increment)

- `cargo test -p melodee-media` — 38 lib + 13 integration passed, zero failures, including the
  new end-to-end segmentation test.
- Requirements traceability: `PRD-MEDIA-006` closed (`tested`) with this evidence; index
  regenerated with `extract_requirements.py`.
- `./scripts/check.sh` at the commit boundary.

Documentation impact:
- None for `/docs` — CUE-driven segmentation is an internal normalization behavior not covered
  by any current public doc page; no frozen route or user-visible surface changes. Internal:
  this report, ADR-0146, and the SPEC §5.5/§6.4/§8.2 amendments.

### 5.6 Validation/catalog join-key fix (R2.2 residual, closing increment)

The ADR-0146 landing left a residual defect in the Phase 3.3 structural gate and the catalog
import join, caught by the end-to-end ingestion test
`tests/normalization.rs::real_mp3_flac_ogg_wav_unicode_artwork_and_sidecars_reach_published`:
four single-track-CUE fixtures regressed to `needs_review` instead of `published` because the
gate and the catalog still joined on the raw `source_file` while the normalizer (ADR-0146) keys
segment Tracks and outputs by `track_key()` (`<source>#cue-track-NN`).

- `crates/melodee-ingestion/src/validate.rs` — Tracks↔outputs inventory now joins on
  `track_key()`; the governing-source inventory is compared against observed audio through the
  outputs' `source_file`; canonical media assets carry the track key as `source_path` so the
  catalog join resolves; per-source governance shape fails closed (ungoverned sources have
  exactly one whole-file output; governed sources have only segment outputs; mixing or
  duplicate whole-file outputs are `IncompleteManifest`); segment identities must cite a parsed
  CUE sidecar and a non-zero CUE TRACK number.
- `crates/melodee-persistence/src/catalog.rs` — the prepared-track/output join keys on
  `track_key()`, matching the manifest Tracks and canonical assets.
- Focused regression coverage in `tests/publication.rs`:
  `cue_segment_outputs_validate_and_join_by_track_key` (valid CUE-governed manifest yields
  track-keyed canonical assets and builds a publication bundle) and
  `cue_segment_identity_pathologies_fail_closed` (duplicate segment identities, track number
  zero, unparsed CUE citation, and whole-file/segment mixing each fail closed).
- Gates after the fix: `cargo test -p melodee-ingestion` — 82 passed, zero failures (all eight
  binaries, including the previously failing end-to-end test);
  `cargo test -p melodee-persistence` — zero failures across all 49 binaries;
  `./scripts/check-smoke.sh` — green (fmt, clippy `-D warnings`, workspace lib tests,
  doc/requirement validation).

Documentation impact:
- None for `/docs` — internal gate/join correction with no user-visible behavior change beyond
  the ADR-0146 contract already recorded above. Internal: this report.

## 6. R2.3 — Configured transcoding (ADR-0147)

ADR-0147 (accepted) supersedes exactly two frozen clauses of ADR-0130: the decision-2
silent-ignore of `format`/`maxBitRate` and the decision-4 sentence "There is no transcoding"
(plus the Jellyfin 400 transcode rejection derived from it). Every other ADR-0130 decision is
unchanged, including the frozen `transcodeOffset[1]` advertisement and the `timeOffset` 404+70
envelope. Native v1 exposes no format/bitrate parameters and is untouched; device profiles stay
metadata-only (PRD-DEVICE-003 boundary preserved).

### 6.1 Contract amendment inside ADR-0147

Decision 2's seed templates reference `%s` as the source input. The implementation substitutes
`pipe:0` and hands FFmpeg the *validated, no-follow-opened* media file as child stdin
(`Stdio::from(file)` produced by `CanonicalAssetRoot::open_asset`), preserving the dirfd
containment guarantee and removing the path-reuse race a filesystem path would reintroduce. A
template whose argv never contains `%s` fails closed. `%bk` renders with FFmpeg's `k` suffix
(`-b:a 128k`); a bare number is bits/second, which libopus rejects and lame silently clamps —
caught by the ffprobe re-parse test after the first integration run exposed the clamp.

### 6.2 Implementation

- `apps/melodee-server/src/transcode.rs` — shared `TranscodingService`: `OutputFormat`
  (aac/mp3/opus + raw), `TranscodeTemplate` parse from the `transcoding.command.*` settings
  (serde_json + `shlex::split`, placeholders `%s`/`%t`/`%bk`), argv-only `tokio::process::Command`
  (no shell anywhere in the pipeline), non-blocking semaphore
  (`transcoding.maxConcurrent`, default 4, `0` disables), `kill_on_drop(true)` cancellation,
  64 KiB bounded stderr capture, and child exit-status logging. The streamed body is a
  `try_unfold` over `ChildStdout`; dropping the body kills FFmpeg and releases the permit.
- `crates/melodee-persistence/src/settings.rs` — the three `transcoding.command.*` seed defaults
  were malformed (unbalanced single-quoted pseudo-JSON since ADR-0108); they are corrected to
  parseable template JSON, and `transcoding.maxConcurrent` (Integer, min 0, default 4) is added.
- OpenSubsonic `media.rs` — `OsStreamPlan::{Direct, Transcode, Reject}` resolution after the
  catalog row load and before direct play: `format=mp3|aac|opus` transcodes; `maxBitRate` (kbps)
  below the source bitrate (byte_size/duration derivation) transcodes to mp3 at
  `min(requested, template bitrate)`; `format=raw`/absent and non-triggered requests keep the
  frozen direct-play path byte-identical. Transcoded responses drop `Content-Length`/`ETag`,
  keep the frozen `Cache-Control: no-store`/`Content-Duration`/CORS headers, ignore `Range`
  (never 206), and swap the download `Content-Disposition` extension. Failures answer the
  Subsonic error-0 envelope (HTTP 200, captured `status: "error"`, `error.code 0`).
- Jellyfin `audio.rs` — the captured 400 rejection is replaced by plan resolution:
  `static=true`/`universal` direct play unchanged; `stream.{mp3|aac|opus}` or `audioBitRate`
  (bits/second) transcodes with default mp3; HEAD answers 200 + `Content-Type` only and spawns no
  FFmpeg process; unsupported containers keep a 400 (`"Unsupported transcoding container."`);
  template/capacity/asset failures answer 503 ProblemDetails.

### 6.3 Compatibility disposition

- Frozen goldens `os-stream-maxbitrate-ignored` (silent-ignore) and
  `jf-playback/stream-extension-no-static` (400 rejection) are retained as historical evidence.
  Both scenario files carry RETIRED annotations and the differential runner whitelists the two
  steps with ADR-0147 registry entries in `scripts/run_phase8_differential.py`.
- `python3 scripts/run_phase8_differential.py` from this commit: all scenario groups replayed;
  both retired steps diverge exactly as registered and every difference (470) is classified
  known — zero unexpected differences ("All captured scenarios match the frozen goldens after
  normalization and the enumerated intentional differences").

### 6.4 Tests and gate (R2.3 increment)

- `cargo test -p melodee-server --test transcode_service` — 5 passed (semaphore saturation +
  recovery, client-disconnect permit release and child kill, spawn-failure mapping, DB template
  override layering + malformed-template fail-closed, real FFmpeg WAV→MP3 whose output re-parses
  under ffprobe with a >20 KB real-audio floor).
- `cargo test -p melodee-server --test opensubsonic_media_api` — 31 passed (mp3/aac transcodes,
  maxBitRate cap trigger and below-cap direct play, `format=raw`+high-cap direct play,
  unsupported-format error-0, disabled-transcoder error-0, malformed-template error-0, Range
  ignored on transcode, download attachment extension swap; all frozen direct-play tests
  byte-identical).
- `cargo test -p melodee-server --test jellyfin_media_api` — 17 passed (real-FFmpeg
  stream.mp3/aac/opus outputs with container-magic assertions, audioBitRate default-mp3, HEAD
  headers-only, 503 when disabled, 400 unsupported container; frozen direct/range/416 tests
  unchanged).
- `cargo test -p melodee-server` lib unit tests cover template parse/render, malformed legacy
  seed fail-closed, missing-`%s` rejection, and format tag parsing.
- Requirements traceability: `PRD-STREAM-002` closed (`tested`); index regenerated with
  `extract_requirements.py`.
- Full gate `./scripts/check.sh` from this tree: repository/doc validation, requirements
  index, 140 Python capture tests, admin-ui gate (typecheck, lint, i18n, 248 unit tests,
  build), `cargo fmt --check`, workspace Clippy `-D warnings`, 984 Rust tests (zero
  failures), and `cargo deny check` (advisories/bans/licenses/sources ok) all pass.
  `python3 scripts/run_phase8_differential.py` re-run after the gate: zero unexpected
  differences against the frozen goldens.

Documentation impact:
- Updated — `docs/src/content/docs/installation/configuration.md` (new "Transcoding" section:
  settings table, template placeholders, `%bk` `k`-suffix rendering, fail-closed and
  cancellation semantics, per-adapter failure shapes),
  `docs/src/content/docs/clients/jellyfin-clients.md` (stale "Transcoding is not available"/
  "No transcoding" claims replaced with the configured behavior and its limits),
  `docs/src/content/docs/clients/opensubsonic-clients.md` (`format`/`maxBitRate` behavior and
  the error-0 envelope). Internal: this report, ADR-0147, the scenario annotations, and the
  differential registry entries.

## 7. R2.4 — Bounded resource classes

Scope per `IMPLMENTATION_OUTSTANDING.md` §7.5: enumerate every runtime resource class, confirm
an independent bound, define backpressure explicitly, and expose utilization/queue-age/saturation
through the existing admin diagnostics surface. No new operator-facing setting was required
(R2.3 added `transcoding.maxConcurrent` under ADR-0147); every class below reuses an existing
configuration knob or a fixed code constant, so no configuration ADR was needed.

### 7.1 Class enumeration

Request-path semaphore classes (all now instrumented through
`apps/melodee-server/src/resource_bounds.rs` and reported on the admin health surface):

| Class | Bound | Source |
|---|---|---|
| `streams.direct_play` | `MELODEE__NATIVE__STREAM_CONCURRENCY` (default 32) | `native_stream.rs::DirectPlayService` |
| `streams.transcoding` | `transcoding.maxConcurrent` (default 4, 0 disables) | `transcode.rs::TranscodingService` (ADR-0147 D5) |
| `images.processing` | fixed 4 | `native/images.rs::ImageProcessor` |
| `podcast.discovery.http` | fixed 8 | `native/podcasts.rs` iTunes discovery client |
| `notifications.dispatch` | fixed 4 | `notifications.rs::NotificationService` SMTP dispatch |
| `provider.lastfm.dispatch` | fixed 4 | `lastfm.rs` provider dispatch |

Worker/background classes (bounded independently and observed through durable PostgreSQL queue
metrics — `WorkerPoolMetrics` on the existing workers surface — rather than the in-process
gauge registry, which is request-path only):

| Class | Bound | Source |
|---|---|---|
| PostgreSQL pool | `DatabasePoolConfig::max_connections` | `melodee-persistence/src/config.rs` |
| Work-claim batch | `MELODEE__WORK__CLAIM_BATCH_SIZE` lease-based claiming | `melodee-persistence/src/config.rs` |
| Inspection | `inspection_concurrency` + bounded claim batch + ffprobe timeout | `melodee-ingestion/src/config.rs`, `inspect.rs` |
| Normalization (FFmpeg/fpcalc) | `normalization_concurrency` + per-tool timeouts | `melodee-ingestion/src/config.rs`, `normalize.rs` |
| Validation/publication | `validation_concurrency` / `publication_concurrency` JoinSet workers | `melodee-ingestion/src/publication_worker.rs` |
| Catalog write | `CatalogConfig::concurrency` × `batch_size` with renewal/cancellation | `melodee-catalog/src/config.rs`, `importer.rs` |
| Search outbox/index | `claim_batch_size`, finite retry attempts, checkpoint fencing | `melodee-search/src/config.rs`, `worker.rs` |

`claim batch × per-item parallelism` cannot multiply unboundedly: each worker's claim batch is
bounded (`claim_batch_size`) and per-item work runs inside the fixed per-class concurrency set
above; file copies, probes, hashes, and subprocesses all execute inside those admitted slots.

### 7.2 Backpressure definition

Backpressure is durable-and-queued everywhere: request-path classes reject immediately
(non-blocking acquire → typed capacity error mapped per adapter contract, e.g. ADR-0147's
error-0 envelope / 503) or queue visibly (direct-play blocking acquire with gauge-observed
waiters); worker classes leave work durable in PostgreSQL with lease fencing rather than
dropping or spawning without limit. Cancellation while waiting for a permit never admits (the
acquire future is dropped before `begin_wait` completes to `admit`), and client disconnect
releases both the semaphore permit and the gauge slot through RAII guards.

### 7.3 Instrumentation and exposure

`resource_bounds.rs` implements a process-local registry of named gauges
(`ResourceBounds`/`ResourceGauge`/`GaugePermit`, RAII drop releases the in-flight slot on
client disconnect, handler completion, or cancellation). `main.rs` installs the registry at
startup before any instrumented service is constructed and attaches gauges to the direct-play
and transcoding services via `with_gauge` builders; image processing, podcast discovery,
notification dispatch, and Last.fm dispatch self-register through `ResourceBounds::global()`.
Wiring is optional everywhere, so tests that construct services directly stay ungauged.

`GET /api/admin/v1/system/health` (and the dashboard payload that embeds it) now carries an
additive `resourceBounds` array — per-class `class`, `max`, `inFlight`, `waiting`,
`oldestWaitMs`, `admittedTotal`, `rejectedTotal`, `saturated` — serialized from the registry
snapshot per request. The field is additive to the Phase-10 admin contract; the admin-ui
`SystemHealth` type mirror carries it (fixtures updated), and no route, status code, or
existing property changed.

### 7.4 Tests and gate (R2.4 increment)

- `apps/melodee-server/src/resource_bounds.rs` unit tests — admission/release accounting,
  saturation at capacity and while waiting, multi-class registry snapshot, oldest-waiter age.
- `apps/melodee-server/tests/transcode_service.rs::gauge_records_saturation_rejection_and_release`
  — saturation rejection counted, in-flight released on body drop, class listed in the registry
  snapshot.
- `apps/melodee-server/tests/native_stream.rs::direct_play_gauge_reports_in_flight_waiters_and_release`
  — blocking-acquire waiter visibility (`waiting`, `oldestWaitMs`), admission after release,
  in-flight returns to zero on body drop.
- `apps/melodee-server/tests/admin_system_api.rs::system_health_exposes_registered_resource_bounds`
  — health payload lists the registered class with correct counters; without a registry the
  field is an empty array (additive contract).
- Existing saturation/cancellation/shutdown/recovery suites remain the load-bearing evidence:
  `transcode_service.rs` (semaphore saturation + recovery, client-disconnect child kill),
  `native_stream.rs` (disconnect capacity release), ingestion/normalization/reconciliation
  200-release suites (bounded concurrency under lease loss and restart).

Requirements traceability: `PRD-INGEST-009` stays `in-progress` (R2.7 performance/fault
evidence still exercises these classes at scale); its implementation/test rows were extended
with the R2.4 instrumentation and the tests above. Index regenerated with
`extract_requirements.py`.

Documentation impact:
- Updated — `docs/src/content/docs/api/admin-api.md` (the `/system/health` payload documents
  the `resourceBounds` array and its per-class fields). Internal: this report and the
  `resource_bounds.rs` module docs. No other user-facing behavior changed: admission decisions,
  route contracts, and settings are exactly as before; the surface is observational only.

## 8. R2.5 — Observability and operational health

**Status: complete.** Correlation propagation across every PRD-OBS-001 boundary, the
PRD-OBS-002 health surface (including the fpcalc fingerprinting-tool probe that was missing
from the media-tools preflight), the extended subsystem status vocabulary, operator retry via
Admin UI and CLI, and staging-handoff provenance through the existing durable event history
are all implemented and gated below. R2 overall remains in progress (R2.6 scheduled/provider
work, R2.7 performance/fault evidence, R2.9 exit criteria).

Scope per `IMPLMENTATION_OUTSTANDING.md` §7.6. Design assumptions recorded before
implementation; each is constrained by frozen external contracts and the existing Phase-10
admin surface:

1. **HTTP correlation boundary.** `admin.rs` has documented since Phase 10 that error-envelope
   `correlationId` is "sourced from the `x-request-id` header when present, otherwise
   generated" — the sourcing was never implemented (every envelope generated a fresh UUID).
   R2.5 implements exactly the documented contract: a middleware parses `x-request-id` as a
   UUID (malformed values never reject — public-adapter error shapes are frozen — they yield a
   generated id), records it on the request tracing span, and scopes a task-local so
   error-envelope constructors on the request task reuse it. No response header is added: the
   Phase-8 differential fails on any extra response header beyond `content-length`, and the
   envelope itself already proves the sourced id. Captured requests carry no `x-request-id`, so
   captured behavior is byte-identical; a client that sends the header gets the documented
   sourcing.
2. **Cross-task propagation.** Fire-and-record provider/notification dispatches are spawned
   tasks, so the task-local does not reach them; the dispatch functions capture the in-flight
   request correlation id explicitly and instrument the spawned future, preserving the span
   parentage without touching dispatch semantics (still single-attempt, never propagated).
3. **Worker boundaries.** Durable work items already carry a durable `correlation_id`;
   ingestion/policy/publication spans and events that carry `work_item_id` gain
   `correlation_id = %item.correlation_id` so a work item's processing chain is greppable from
   one token. Media-tool (ffmpeg/ffprobe/fpcalc) invocations get a bounded span logging tool,
   outcome, and duration — never absolute source roots (file-name-only arguments).
4. **Health extension is additive and observational.** `HealthDto` gains `podcastStorage`,
   `disk`, and `workers` subsystems; the subsystem status vocabulary extends from
   `ok|degraded|disabled` with `unavailable` (dependency probe failed), `readOnly` (root
   exists, not writable), `misconfigured` (invalid/absent required configuration), and
   `capacityExhausted` (free disk below the fixed threshold). The top-level status keeps the
   Phase-10 rule (`ok` when PostgreSQL is reachable and Tantivy is healthy-or-disabled,
   otherwise `degraded`): the pre-existing `storage`/`ffmpeg` preflights are routinely degraded
   in legitimately unconfigured environments, and letting them or the new R2.5 subsystems flip
   the top-level indicator would change the Phase-10 dashboard health semantics beyond the
   assigned scope. The new subsystems report their own statuses without flipping the top-level
   indicator; operators read them individually on the diagnostics surface. Disk thresholds are
   fixed constants (no new operator setting): degraded below 5% free or 1 GiB free on any
   checked root. `workers` reports stale (expired, not yet recovered) lease counts and the
   maximum durable queue age across pools; degraded when stale leases exist or the oldest queue
   age exceeds 24h. Worker queue metrics stay durable-backed (no second monitoring database,
   per the package constraint).
5. **Staging handoff provenance** is already durable: `Created`/`Transitioned` work events plus
   the work item's `source_path`/`current_path`/`target_path` give the full inbound → staging →
   publication chain, and both the Admin UI work-item detail and `melodee-cli work show` render
   the event history. R2.5 verifies this with a test rather than adding schema.
6. **CLI retry gap closed with the admin semantics.** The Admin UI reprocess endpoint
   (`failed|quarantined -> ready`, `TransitionKind::Operator`) has no CLI counterpart;
   `melodee-cli work retry <id> [--reason ...]` reuses the identical store transition so both
   surfaces enforce the same state-machine arm.

### 8.1 Implementation (R2.5 increment)

- **Correlation boundary** — `apps/melodee-server/src/correlation.rs`: middleware parses
  `x-request-id` as a UUID (malformed values yield a generated id, never a rejection), records
  it on the request span (`make_http_span` declares the field), and scopes a task-local.
  `envelope_id()` sources the admin and native error-envelope `correlationId` across all 16
  constructor sites. No response header is echoed (Phase-8 differential constraint).
- **Cross-task propagation** — last.fm `spawn_scrobble`/`spawn_now_playing` and the
  notifications dispatch capture the in-flight request correlation id and instrument the
  spawned future, preserving span parentage without changing single-attempt dispatch
  semantics.
- **Worker spans** — ingestion/normalize, publication, and policy workers emit per-item spans
  carrying `work_item_id` and `correlation_id = %item.correlation_id`; the search outbox pass
  logs a pass-level span. Durable `correlation_id` on the work item anchors the whole chain.
- **Media-tool instrumentation** — `crates/melodee-media` `ToolInvocation` drop-guard logs
  tool, outcome, and duration with file-name-only arguments for ffmpeg/ffprobe/fpcalc; a
  redaction audit test proves SMTP-password-class secrets never reach delivery-failure logs.
- **Health extension** — `admin_system.rs` `aggregate_health` composes `postgresql`
  (probe failure maps to `unavailable`), `tantivy`, `storage`, `ffmpeg`, `podcastStorage`
  (root writability via `directory_is_writable`, `disabled` when the podcast worker is
  unconfigured), `disk` (`rustix::fs::statvfs` per root; `capacityExhausted` below 1 GiB or 5%
  free; `misconfigured` on invalid ingestion configuration; the statfs seam is
  dependency-injected for tests), and `workers` (durable `stale_lease_count` aggregate plus
  per-pool queue metrics; `degraded` on stale leases past the 1h grace or queue age over 24h).
- **Admin UI** — `SystemHealthPanel` renders the three new subsystem rows; the
  `SubsystemHealthStatus` union and `STATE_BY_STATUS` maps (panel + storage screen) cover all
  seven statuses; the seven new `system.health.*` keys are in the en-US catalog (other locales
  fall back per the locale checker).
- **CLI** — `melodee-cli work retry <id> [--reason ...]` performs
  `failed|quarantined -> ready` with `TransitionKind::Operator` and the audited reason,
  reusing `WorkItemStore::transition` exactly as the admin reprocess endpoint does.

### 8.2 Tests and gate (R2.5 increment)

- `admin_system_api.rs::error_envelope_sources_the_request_correlation_header` — the
  documented envelope sourcing works end to end and captured requests (no `x-request-id`)
  stay byte-identical.
- `admin_system_api.rs::system_health_reports_stale_leases_and_queue_age` — `workers` reports
  `ok` on a clean queue and degrades with an operator-readable detail once a lease is aged
  past the grace window.
- `admin_system.rs` unit tests (`disk_health_*`) — `capacityExhausted` under the absolute and
  ratio floors, `degraded` on unreadable filesystems, `disabled` without roots, tightest-root
  detail, all through the injected statfs seam.
- `notifications.rs::delivery_failure_logs_never_contain_the_smtp_password` — redaction audit.
- `apps/melodee-cli/tests/work_retry.rs` (3 tests) — retry requeues a failed item with the
  audited operator-reason event, refuses non-failed/quarantined items without mutating state,
  and fails on unknown ids.
- Admin UI: 248 unit tests pass with the extended fixtures; typecheck, lint, and the i18n
  locale check pass.
- Staging handoff provenance (assumption 5) is covered by the existing work-item event
  history surfaces and the `work show` CLI rendering; no schema change was required.
- Full repository gate (`./scripts/check.sh`) passes after the increment: Rust workspace
  (unit + integration), 140 Python capture tests, admin-ui gate, and `cargo deny` all green.

