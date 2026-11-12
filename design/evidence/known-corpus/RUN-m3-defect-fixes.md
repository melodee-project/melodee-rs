# Live defect fixes verified against a real corpus (not the fixture corpus)

Two defects found by the known-answer exercise (`design/testing/UI-AND-WORKFLOW-AUDIT.md`) and by a
real 50-release import were fixed and verified against that live catalogue rather than against
synthesised fixtures.

## D1 — artwork reads were refused under a normal catalogue browse

`ImageProcessor::try_admit` was non-blocking with a four-permit cap, so a list view that requests one
cover per row was rejected outright.

| Measurement | Before | After |
|---|---|---|
| 24 concurrent `GET /images/album_{id}/small` | 20 × `503`, 4 × `200` | 24 × `200` |
| 24 sequential requests | 24 × `200` | 24 × `200` |

Reads now wait up to `IMAGE_ADMISSION_WAIT` (10 s) for a permit, the bound is unchanged, and
`original`/`source` requests take no decode permit at all because they serve stored bytes. Uploads
keep the non-blocking refusal, which is an honest answer a caller can retry.

## D2 — acoustic fingerprints were computed, then discarded

| Observation | Before | After |
|---|---|---|
| `audio_identity_evidence` rows of kind `acoustic_fingerprint` | 0 | one per imported Track |
| Novelty for a freshly imported Track | `uncertain` | `uncertain` (unchanged by design) |
| Novelty for a **retagged copy** of a catalogued Release | `uncertain` | `known_audio_identity` — "Canonical acoustic fingerprint exists in another Release" |
| Novelty for an **exact copy** | `exact_duplicate` | `exact_duplicate` |
| Novelty for a **96 kbps re-encode** | `uncertain` | `uncertain` — "No catalog comparison conclusion for this AudioIdentity" |

The last row is the honest boundary: re-encoding changes Chromaprint output, so these copies cannot be
matched by digest equality. Measured directly — six tracks, six different fingerprints after
transcoding. Similarity matching is the documented follow-up (ADR-0186, alternative B).

The uncertainty text changed because it was about to become false: the fingerprint *is* now durable,
so what is missing is a catalogue comparison conclusion.

## Corpus generator fix required by the acoustic matcher

The generator synthesised every Track as a plain sine tone whose frequency
depended only on the Track index. A constant spectrum is exactly what Chromaprint
does *not* describe, so fixtures with the same duration produced identical
fingerprints: `Greatest Hits` shared audio with `Broken Transmission` and
`Sidecar Source`, and after the acoustic matcher landed that compilation was
classified as redundant and dispositioned `ignore` instead of the declared
`needs_review` (caught by run `m3`, one failing assertion).

Each fixture now also gets a deterministic per-fixture pitch offset and a slow
per-fixture sweep, so the fixtures are acoustically distinct while remaining
byte-stable across regenerations. Deliberate collisions remain where the fixture
declares them: `08-duplicate-reimport` reuses `01`'s audio, and
`11-fault-missing-album-tag` reuses `10`'s audio with the album tag removed.

Run `m5` with the fixed generator: **323 assertions, 0 failures**, novelty
`exact_duplicate=8` (the declared byte copy) and `uncertain=18` (no catalogue
comparison conclusion — correct for unique audio).

## Notes for the next run

- The dev fixture corpus (11 fabricated Releases) was not needed for this cycle: real media found
  both defects faster than synthesised media, mostly because real tags carry **year-only** dates
  (all 50 landed as `release_date_text`, none as a typed date — correct) and real covers are
  200 KB–800 KB rather than a few kilobytes.
- Three probe Releases created to demonstrate the acoustic behaviour (`Impulse` copy / re-encode /
  retag) cannot be removed through a supported path: deletion is a review-time disposition and these
  items are already `indexed`. They were created by the audit, not by the operator, and are listed
  here so they are not mistaken for imported content.
