# ADR-0186: durable acoustic identity evidence and exact-fingerprint novelty matching

- **Date:** 2026-11-12
- **Status:** Proposed
- **Decision owners:** catalog/policy maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-FP-001, PRD-FP-002, PRD-FP-003, PRD-POLICY-002

## Context

Normalization runs `fpcalc` for every source file and retains the Chromaprint
fingerprint in the sealed v3 manifest (`normalization.fingerprints`, ~3 KiB per
track). Nothing promoted that observation into `audio_identity_evidence`, whose
vocabulary already contains `acoustic_fingerprint` and whose declaration says an
AudioIdentity is "confirmed" only by durable fingerprint/match evidence.

Two consequences were observed on a 50-release live import:

1. `audio_identity_evidence` was empty, and every Track's novelty was
   `uncertain` with the reason "AudioIdentity lacks durable fingerprint or match
   confirmation" — a statement that was about to become untrue and, more
   importantly, told an operator nothing actionable.
2. A copy of an already-catalogued Release whose bytes differ only by tags
   (identical audio, different byte hash, therefore a fresh AudioIdentity) was
   also `uncertain`, because byte-hash equality was the only cross-Release
   signal available.

The expensive observation existed in the manifest but no durable, queryable
conclusion did, so duplicate/novelty analysis was inert and the fingerprint cost
was paid for nothing.

An existing decision is preserved deliberately: a locally computed fingerprint is
*evidence*, not a confirmed identity
(`phase5_handoff::cataloged_policy_input_exists_without_readable_source_or_canonical_media`).
Novelty still requires a catalogue comparison conclusion before it claims a Track
is new; an audited `no_catalog_match` assertion remains the only route to
`new_track`.

## Decision

1. **Promote the observation to durable evidence.** At catalog import, each Track
   whose normalized output is a whole file gets one
   `audio_identity_evidence` row of kind `acoustic_fingerprint`, polarity
   `supports`, confidence 9000, source `manifest-v3:chromaprint`, carrying
   `fingerprint_digest` (BLAKE3 over the trimmed fingerprint string),
   `fingerprint_length`, and `media_asset_id`. The raw fingerprint stays in the
   sealed manifest; only the fixed-width digest is durable and indexed. No audio
   is re-read (PRD-FP-001).
2. **Do not change identity confirmation.** `has_confirmed_audio_identity` keeps
   its existing meaning (an audited no-match assertion or a confirmed
   alternate-recording relation). A fingerprint alone does not make a Track
   "confirmed", so `new_track` is never inferred from it.
3. **Match acoustically across Releases on the digest.** Novelty evaluation loads
   the set of `fingerprint_digest` values recorded for *other* Releases and
   classifies a Track whose digest is in that set as `known_audio_identity`
   with evidence `audio_identity_match` and confidence 9000. The comparison key
   is the Track (`audio_identity_evidence.track_id`), not the AudioIdentity, so
   an identity merge cannot fabricate or hide an acoustic match.
4. **Keep the comparison exact.** Duplicate detection compares digests for
   equality; it is not a similarity search. Re-encoding at a different bitrate
   changes Chromaprint output (verified: a 96 kbps re-encode of a catalogued
   album produced six different fingerprints), so such copies remain `uncertain`
   rather than being mislabelled as new or as known.
5. **Correct the uncertainty text.** The `uncertain` detail becomes "No catalog
   comparison conclusion for this AudioIdentity", which is what is actually
   missing.

Migration `0043_acoustic_identity_evidence_indexes` adds two partial indexes
(evidence-by-identity for confirmation reads, digest for the cross-Release set).
Both are additive; a downgrade leaves evidence rows in place and simply stops
using them.

## Alternatives considered

### Alternative A — treat a durable fingerprint as identity confirmation

Cheapest, and it would have flipped every `uncertain` Track to `new_track`. It
contradicts the preserved decision that a local fingerprint is evidence, not a
conclusion, and it would assert novelty for audio that may exist elsewhere in a
different encode. Rejected.

### Alternative B — similarity search over Chromaprint (index + threshold)

Would catch bitrate re-encodes and near-duplicates. It needs a similarity index,
a threshold policy, and an operator-visible false-positive story; that is its own
ADR, not a defect fix. Rejected for now; the digest comparison is the safe
subset that needs no new infrastructure.

### Alternative C — record an audited `no_catalog_match` assertion automatically

Would make every fresh import `new_track`. An audited assertion claims "this
audio is not in the catalogue", which exact-digest comparison alone cannot
support (a re-encode differs). Rejected as an overclaim.

## Rationale

The fingerprint cost is already paid during normalization, so making its
conclusion durable is nearly free; the digest is small, indexed, and computed
without touching audio. The change adds exactly one cross-Release signal that is
true by construction (identical Chromaprint output means identical analyzed
audio), leaves every existing novelty conclusion reachable, and keeps the
conservative `uncertain` outcome where the evidence genuinely does not support a
claim.

## Consequences

- A copy whose bytes differ only by tags is now `known_audio_identity` instead of
  `uncertain` (verified live on a real 50-release catalogue).
- Identity screens and match decisions gain real acoustic evidence rows.
- Novelty reads one extra indexed set per evaluation; bounded by the catalogue's
  distinct fingerprints.
- Releases imported before this change keep their stored `uncertain` rows until
  reprocessed; reprocessing re-derives novelty with the new evidence.
- Follow-up (not in scope): similarity matching for re-encodes, and surfacing
  acoustic evidence in the identity review UI beyond the existing evidence list.

## Validation

- `crates/melodee-persistence/tests/catalog_import.rs` — an import writes an
  `acoustic_fingerprint` row with a 64-hex digest.
- `crates/melodee-policy/tests/policy_acceptance.rs` — a digest present in the
  catalogue set yields `known_audio_identity`; the same Track without the
  catalogue-side match stays `uncertain`.
- `crates/melodee-persistence/tests/phase5_novelty.rs` — commit revalidation
  accepts the acoustic conclusion and a merged AudioIdentity does not fabricate
  one.
- Live: 50-release catalogue, then a retagged copy → `known_audio_identity`
  ("Canonical acoustic fingerprint exists in another Release"); an exact copy →
  `exact_duplicate`; a 96 kbps re-encode → `uncertain` with the corrected text.
