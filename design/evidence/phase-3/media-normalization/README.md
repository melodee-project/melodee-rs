# Phase 3.2 Evidence — Media normalization and acoustic fingerprinting

> Subsequent status: Phase 3.3/3.4 validate and publish these temporary outputs as manifest v3
> canonical Release bundles. See [canonical-publication evidence](../canonical-publication/README.md)
> and the consolidated [Phase 3 report](../PHASE-3-REPORT.md). This page preserves the 3.2 slice
> boundary/evidence and its then-current limitations.

## Scope and result

Phase 3.2 implements the leased `Normalizing -> Validating` handoff. It creates
verified, temporary output and publishes manifest schema v2 atomically with the
state transition. At the 3.2 slice boundary, it did **not** implement structural
verification or canonical publication; Phase 3.3 now consumes this result and
publishes manifest v3 bundles as documented in the links above.

The independent hardening review ran on 2026-08-10 with:

- FFmpeg/ffprobe 8.1.2 (Fedora build with `libmp3lame`);
- Chromaprint `fpcalc` 1.6.0;
- PostgreSQL 18.4-alpine on the documented local port 5433;
- Rust 1.97.1.

CI installs `ffmpeg` and `libchromaprint-tools`; the real media tests are normal
correctness gates, not ignored evidence harnesses.

## Conversion profile and MP3 retention

The versioned converted-output profile is
`mp3-libmp3lame-vbr-q4-48khz-v1`. FFmpeg receives separate argv entries (never
a shell command), with stdin closed:

```text
-y
-v error
-xerror
-i INPUT
-vn
-map 0:a:0
-c:a libmp3lame
-q:a 4
-b:a 384k
-ar 48000
-map_metadata 0
OUTPUT
```

This preserves the exact seeded legacy flags captured in
`compatibility/media/baseline.json`. The flags do not mean “VBR with a 384 kbps
maximum”: FFmpeg's libmp3lame implementation selects the VBR branch when global
quality (`-q:a`) is set and only applies `bit_rate` in the non-quality branch.
The `-b:a 384k` argument is therefore inert in this profile. This behavior was
verified against the [official FFmpeg codec documentation](https://ffmpeg.org/ffmpeg-all.html#libmp3lame)
and [libmp3lame wrapper source](https://ffmpeg.org/doxygen/trunk/libmp3lame_8c_source.html).

Conversion forces 48 kHz. Mono and stereo inputs retain their channel count;
formats wider than libmp3lame's mono/stereo support are downmixed to stereo.
`-map_metadata 0` preserves supported container metadata; the real FLAC test
proves title and ReplayGain track gain reach the MP3 output. `-vn` prevents
attached pictures or other video from entering converted audio output. Artwork
is handled separately.

The captured legacy policy retains existing MP3 rather than forcing it through
the conversion settings. Retention requires:

1. an MP3 codec in an MP3 container;
2. exactly one audio stream;
3. successful complete decode with `ffmpeg -xerror -f null`;
4. byte-for-byte copy followed by the same strict output checks used below.

Sample rate, VBR/CBR mode, encoder label, bitrate, metadata, and attached
artwork do not trigger re-encoding of an otherwise valid existing MP3. This is
why the real 44.1 kHz MP3 fixture is retained unchanged. Corrupt,
multiple-audio-stream, and non-MP3 inputs do not qualify by probe fields alone.

Every output must exist, be non-empty and hashable, probe as an MP3 container
with exactly one MP3 audio stream, have the expected sample rate and channel
behavior, and remain within `max(250 ms, 2%)` of source duration. Essential
verification failure prevents publication. Source files are BLAKE3-checked
against inspection evidence and again after processing; the full source tree
snapshot is also compared before and after the attempt.

## Fingerprints

Fingerprint argv is:

```text
fpcalc -json -length 120 -- INPUT
```

The `--` terminator protects leading-dash names. Fingerprints are computed from
source audio, not normalized output. `DURATION` is converted to milliseconds;
missing/non-finite/non-positive duration, empty fingerprint, malformed or
oversized JSON, timeout, cancellation, and non-zero exit are failures. A
fingerprint failure is retryable normalization failure, not structural media
invalidity.

BLAKE3 remains exact byte evidence. Chromaprint remains acoustic evidence.
Neither is promoted to a Track or underlying-performance identity decision in
Phase 3.2; PRD-FP-004 remains planned pending its required naming/schema ADR.

## Leases, cancellation, and concurrency

`MELODEE__NORMALIZATION__CONCURRENCY` is actual simultaneous Release execution,
implemented with a Tokio `JoinSet`. A batch claims at most that many items and
spawns one independent task per item; each task owns one lease, cancellation
flag, blocking media execution, and physical attempt directory. A failed item
does not stop siblings.

The heartbeat interval is:

```text
min(lease_duration / 3, 30 seconds)
```

Non-positive work configuration and durations too short to yield a renewal
tick are rejected. Tokio missed ticks use delay behavior. Renewal uses the
actual current UTC time, not the batch claim timestamp. Publication and failure
recording also use current time, so a stale timestamp cannot make an expired
lease appear valid.

Lease-renewal failure sets only that item's cancellation flag. Graceful server
shutdown sets a separate shared shutdown flag. FFmpeg, ffprobe, and fpcalc wait
loops check both signals at 10 ms intervals, kill the child, and wait to reap it
before returning `Canceled`. Canceled or stale work is never published. The
PostgreSQL publication transaction re-locks the item and checks current lease
owner and expiry as the final authority.

Automated PostgreSQL evidence proves:

- a 2-second lease renews at roughly 667 ms during a 3-second FFmpeg operation,
  recording multiple `lease_renewed` events;
- forced lease theft cancels a long child in under 3 seconds;
- the stale worker publishes no `Validating` transition;
- retry succeeds from a distinct attempt directory;
- graceful scheduler stop cancels in-flight FFmpeg promptly;
- configured concurrency 8 reached a measured maximum of 8 and never exceeded
  8 in the 200-Release scenario.

## Temporary attempt areas

Physical layout is:

```text
{work-area}/
  {work-item-id}/
    attempts/
      {attempt-id}/
        audio/{source-relative-path}.{source-extension}.mp3
        artwork/external/{source-relative-path}.{source-extension}.jpg
        artwork/embedded/{embedded-content-hash}.embedded.jpg
```

No attempt removes its work-item parent or another attempt. Source-relative
directories and original extensions remain in logical names, preventing
same-stem collisions such as `track.wav` versus `track.m4a` or two discs with
`01.flac`. Attempt ids and physical roots are execution provenance excluded
from semantic hashing; logical output paths are deterministic semantic facts.

## Manifest versioning and determinism

Inspection continues to create schema v1. Successful normalization upgrades
the manifest to schema v2. Normalized output hash/codec/rate/channels/duration
fields are required in v2 rather than accepting empty defaults. PostgreSQL
publication updates `schema_version`, payload, hash, provenance, and completion
time in one transaction before the `Validating` state update. A missing
manifest row or stale lease rolls the transaction back.

`normalization_started_at` is set when the attempt is claimed and resets for a
retry; completion/provenance are cleared at claim and set only at successful
publication. Tests prove a stored Phase 3.1 v1 fixture still deserializes, a v2
manifest round-trips, semantic normalization facts affect the hash, and worker,
attempt, tool, time, and physical work-area provenance do not.

Two independent attempts over the same deterministic input produced identical:

- semantic manifest hash;
- normalized logical paths;
- normalized audio and artwork byte hashes;
- Chromaprint observations;
- parsed sidecar observations.

Their attempt ids, timestamps, elapsed time, and physical paths differed as
expected.

## Real format and process matrix

| Case | Real-tool proof |
| --- | --- |
| FLAC -> MP3 | MP3/48 kHz, duration tolerance, output BLAKE3, title and ReplayGain preserved |
| OGG/Vorbis -> MP3 | MP3/48 kHz, duration tolerance, output BLAKE3 |
| WAV -> MP3 | MP3/48 kHz, duration tolerance, output BLAKE3 |
| AAC/M4A -> MP3 | MP3/48 kHz, duration tolerance, output BLAKE3 |
| existing MP3 | complete decode, 44.1 kHz VBR and 320 kbps CBR bytes retained exactly |
| 5.1 WAV | verified libmp3lame-compatible stereo downmix |
| all seven tracks across five formats | source unchanged and non-empty source Chromaprint produced |
| FFmpeg | success, non-zero, timeout, cancellation/reap, unprobeable output, unhashable output |
| fpcalc | success, non-zero, malformed/empty output, timeout, cancellation/reap |
| paths | spaces, Unicode, metacharacters, and leading dash passed as argv without a shell |
| failures | corrupt audio, source change/disappearance, and sibling failure isolation |

Synthetic sine-wave media is generated during tests; no copyrighted fixture is
used.

## Artwork matrix

Artwork is decoded and normalized, not copied into a misleading extension.
Each successful output is a non-empty, hashable 1600x1600 JPEG padded with
black while preserving aspect ratio. Source byte size is bounded at 16 MiB by
default; external headers and embedded inspection dimensions are rejected when
zero, malformed, over 16384 on either axis, or over 100 million pixels.

| Input | Result/test |
| --- | --- |
| external JPEG | real FFmpeg normalization and strict JPEG/hash/dimension verification |
| external PNG | same |
| embedded JPEG | extracted by explicit picture stream index, decoded, normalized to JPEG |
| embedded PNG | decoded rather than copied to a `.jpg` name, normalized to JPEG |
| malformed image | skipped with `artwork.normalization_failed` finding |
| byte-oversized image | rejected before FFmpeg |
| absurd dimensions | rejected by bounded preflight |
| multiple entries | deterministic distinct paths and hashes |

Artwork failure does not discard otherwise valid audio normalization, but no
failed artwork entry with width/height zero or an empty hash is published.

## Sidecar matrix

Sidecar reads are bounded to 4 MiB, accept UTF-8/BOM and record a Windows-1252
fallback finding. Local parse findings are retained both on each
`ParsedSidecar` and in aggregate normalization findings.

| Type | Implemented semantics | Malformed behavior |
| --- | --- | --- |
| CUE | album title/performer; quoted FILE context; TRACK number/type; TITLE/PERFORMER; numbered INDEX timestamps | structured invalid FILE/TRACK/INDEX or no-tracks finding |
| M3U | comments/BOM, EXTINF duration/title associated with following path, paths with spaces | orphan/replaced EXTINF finding |
| SFV | filename preserved through final whitespace, uppercase 8-hex CRC32 | invalid-entry finding |
| NFO | text vs XML-shaped raw content classification | malformed-XML finding from bounded structural check |
| LRC | metadata tags and multiple timestamps per lyric line | unclosed/invalid/untimed finding |

These are Phase 3.2 semantic observations. Legacy CUE audio splitting and a
broader encoding/parity corpus remain later work, so the full PRD-MEDIA-006
status is truthfully `in-progress`.

## 200-Release acceptance result

The non-ignored PostgreSQL vertical test executed discovery, settling,
inspection, normalization, source fingerprinting, atomic manifest publication,
and the transition to `Validating` for 200 synthetic Releases:

```json
{
  "total": 200,
  "successful": 200,
  "expected_failure": 0,
  "unexpected_failure": 0,
  "lost": 0,
  "duplicate_authoritative_result": 0,
  "source_mutations": 0,
  "validating": 200,
  "configured_concurrency": 8,
  "measured_concurrency": 8,
  "elapsed_ms": 22254
}
```

The test binary's total wall time was 75.92 seconds including database creation,
discovery, settling, inspection, fixture work, assertions, and teardown.

## Known limitations and honest requirement boundary

- Work-area output remains temporary and non-authoritative. Completed Phase 3.3
  verifies it before publishing a separate canonical manifest v3 bundle.
- Successful attempt directories must remain until Phase 3.3 publication
  completes. Operational stale-attempt cleanup policy is still not implemented,
  so disk usage must be monitored.
- CUE-driven audio splitting is not implemented. PRD-MEDIA-006 remains
  `in-progress`.
- The full artwork requirement includes later replacement, upload, thumbnail,
  and serving surfaces. PRD-MEDIA-005 remains `in-progress`.
- Separate per-resource pools for every class named by PRD-INGEST-009 and
  external enrichment do not yet exist. The requirement remains `in-progress`
  even though Phase 3.2 Release/process concurrency is bounded and measured.
- Underlying audio identity naming/schema and optional physical deduplication
  remain planned (PRD-FP-004/005).
- NFO XML is retained and structurally classified; Phase 3.2 does not introduce
  a general XML interpretation framework.

## Authorities

- `PRD-MEDIA-002`, `PRD-MEDIA-003`, `PRD-FP-001`, and `PRD-FP-003`;
- ADR-0108 for structured FFmpeg/ffprobe/fpcalc process boundaries;
- ADR-0115 for the Melodee-owned manifest/media boundary;
- `compatibility/media/baseline.json` and
  `design/evidence/phase-0/media-baseline.md` for captured legacy policy;
- `crates/melodee-media/tests/normalization_real.rs`,
  `crates/melodee-ingestion/tests/normalization.rs`, and
  `crates/melodee-persistence/tests/normalization.rs` for executable proof.
