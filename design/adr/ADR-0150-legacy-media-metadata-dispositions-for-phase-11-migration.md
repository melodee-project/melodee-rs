# ADR-0150: Legacy media metadata dispositions for Phase 11 migration

- **Date:** 2026-08-18
- **Status:** Accepted
- **Decision owners:** Release owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MIG-001, PRD-MIG-002, PRD-MIG-007, PRD-STORAGE-004

## Context

The frozen legacy `Songs` table contains metadata that is not represented by the current Rust
catalog/media schema. The R4 migrated-snapshot replay exposed this as a compatibility boundary:
legacy `Genres`, `Moods`, `BitDepth`, free-form descriptive fields, and aggregate play facts do
not have an authoritative target column or event model. Treating `Genres` as `ReleaseTrait`,
inventing play events from a counter, or adding columns during migration would silently change
semantics and would violate the no-schema-change-without-accepted-ADR rule.

Two audio fields do have existing target representations. Legacy `BitRate` is captured in two
forms in the frozen/test corpus: ordinary legacy rows use the historical UI unit (kbps, for
example `1411`), while some imported rows already carry bits per second (for example `900000`).
Legacy `BPM` maps to the existing `native_track_audio_features.tempo` field. The migration must
preserve both without changing public API contracts or modifying source media.

## Decision

Pending release-owner acceptance, the migration applies the following dispositions:

| Legacy field | Disposition |
|---|---|
| `Songs.BitRate` | Map to `media_assets.bitrate_bps`; values below `100000` are treated as historical kbps and multiplied by `1000`, while larger values are retained as already-bps. Invalid/non-positive values become NULL. |
| `Songs.BPM` | Map non-negative values to `native_track_audio_features.tempo`; preserve zero as an observed value. |
| `Songs.Genres`, `Songs.Moods` | Do not map to `ReleaseTrait` or fabricate catalog taxonomy. Record field names in a warning issue. |
| `Songs.BitDepth`, `Songs.IsVbr` | Record as reference-only warning fields; no current authoritative media columns exist. |
| `Songs.Comment`, `Lyrics`, `PartTitles`, `Tags`, `Notes`, `Description`, `AlternateNames` | Record non-empty fields as reference-only warning fields; do not copy free-form legacy text into unrelated Rust fields. |
| `Songs.PlayedCount`, `LastPlayedAt`, `LastMetaDataUpdatedAt` | Record present facts as reference-only warning fields. Do not synthesize `native_track_plays` events from aggregate counters. |
| Canonical path presentation | Preserve the Rust canonical adoption path (`artist/release/file`) required by ADR-0148. Do not change compatibility route presentation to make a comparator pass. |

Each affected imported Track receives one `legacy_migration.issues` row with code
`metadata.reference_only`, severity `warning`, and a detail object containing field names only;
legacy metadata values and secrets are not copied into the issue trail. The issue is emitted in
the same transaction as the Track and is therefore restart-safe and auditable. No target schema,
public identifier, route, status, or wire DTO changes are authorized by this proposed ADR.

Acceptance of this ADR is required before R4 can treat these exclusions as final. A future
representation of any excluded field requires a new accepted schema/compatibility decision and a
backfill/client plan.

## Alternatives considered

### Alternative A

Add nullable genre, bit-depth, free-form metadata, and aggregate-play columns during migration.
Rejected for this release closure: it expands the authoritative schema without an accepted schema
ADR, conflates legacy counters with event history, and creates new compatibility obligations.

### Alternative B

Drop the fields without an issue. Rejected because ADR-0148 §D.8 requires every unrepresented
field to have an explicit disposition and the migration audit must not silently lose source facts.

## Rationale

The proposal preserves the fields that have a stable Rust meaning, keeps unsupported data visible
without inventing semantics, and leaves the PostgreSQL catalog authoritative. It also explains
why the remaining OpenSubsonic replay differences are migration-data/compatibility evidence rather
than permission to redesign the adapter contract.

## Consequences

### Positive

- Bitrate and BPM survive migration in the existing target representations.
- Unsupported metadata is visible per Track in the durable issue trail and can be reviewed before
  cutover.

### Negative / trade-offs

- The current OpenSubsonic/Jellyfin manifest surfaces cannot expose genres or bit depth for rows
  imported without a manifest-v3 publication receipt; replay classification remains required.
- Reference-only fields are not queryable as authoritative Rust catalog data until a future ADR
  provides a representation.

### Follow-up work

- Release-owner acceptance is required before the reference-only dispositions are a final 1.0
  exclusion.
- R4 must attach the native/OpenSubsonic/Jellyfin differential classifications and affected counts.
- If genre, bit-depth, or legacy play-counter compatibility is required for 1.0, supersede this
  proposal with an accepted schema and client migration plan before cutover.

## Validation

The current implementation is covered by:

- `map::legacy_bitrate_bps` unit tests for kbps, already-bps, zero, and NULL values;
- the full legacy import integration test, including a durable reference-only issue assertion;
- the 12,000-Track R4 rehearsal and native/OpenSubsonic replay reports; and
- the full repository gate.

Acceptance additionally requires production-snapshot affected counts, external differential
classification, and the representative OpenSubsonic/Jellyfin client workflows.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0148: Legacy migration contract](ADR-0148-legacy-migration-contract.md)
- [ADR-0149: Migration rehearsal, freeze, cutover, and rollback](ADR-0149-phase-11-rehearsal-freeze-cutover-and-rollback-procedures.md)
- ADR-0150 decision package and current cross-surface inventory (historical artifact removed)
- Catalog ordering decision package (historical artifact removed)
- R4 evidence (historical artifact removed)
