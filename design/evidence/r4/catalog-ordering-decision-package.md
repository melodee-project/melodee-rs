# Catalog ordering decision package for R4 differential closure

Status: **Decision required**. This package records the observed frozen-capture evidence and
options; it does not change runtime ordering or accept an ADR.

Date: 2026-08-20  
Decision owner: Release owner, with compatibility/product owner input  
Related requirements: `PRD-API-M1-003`, `PRD-API-M1-004`, `PRD-MIG-001`, `PRD-MIG-007`,
`PRD-TEST-001`, `PRD-TEST-002`

## Observed evidence

The current migrated native-v1 replay has 185 unclassified differences. Three concentrated
catalog-list clusters account for 117 entries:

| Frozen scenario | Unclassified entries | Observed behavior |
|---|---:|---|
| `catalog/albums-list` | 63 | The frozen capture returns releases in preserved legacy/source sequence; the current adapter returns the same catalog entities in its default display-name order. Positional IDs, names, artists, counts, dates, and URLs therefore differ together. |
| `catalog/artists-list` | 22 | The first page contains the same migrated Artist population but a different positional sequence; the captured default is not the current adapter's default normalized-name order. Two user-state fields also remain separately visible in this scenario. |
| `catalog/artists-list-page2` | 32 | Page-two membership and positional sequence shift with the default ordering, causing IDs, names, counts, dates, and URLs to differ together. |

The evidence is from the committed report
[`dual-run-report-native.json`](dual-run-report-native.json) and the frozen goldens under
`compatibility/melodee-api-v1/golden/native-catalog/`. The mismatches are not comparator
volatility and must not be hidden with positional masks. The current native implementation uses
explicit deterministic ordering, but its default is display-name based; the captured default
appears to follow source insertion/legacy ordering. The exact contract must be decided before a
runtime change.

## Options

### A — Preserve legacy/source order for migrated entities

Order migrated releases and Artists by the existing durable
`legacy_catalog_id_mappings.legacy_numeric_id`, with a deterministic public-ID fallback for rows
without a migration mapping.

- Compatibility: likely closes the three native clusters for migrated snapshots and preserves
  captured page membership/order.
- Migration: uses existing source-primary-key mapping data; no media changes and no public-ID
  regeneration.
- Schema: no migration schema change if the mapping join is accepted, but runtime queries must
  handle multiple migration runs and unmapped newly ingested rows safely.
- Operational: adds a bounded indexed join or equivalent lookup to browse queries; performance
  must be measured at catalog scale.
- Risk: data-origin-dependent ordering could make newly ingested and migrated catalogs behave
  differently unless the fallback policy is explicit.

### B — Make display-name ordering the accepted 1.0 contract

Keep the current deterministic display-name ordering and record the frozen legacy sequence as an
intentional compatibility deviation with a client/migration note.

- Compatibility: leaves the 117 differences unexplained unless the owner explicitly accepts them;
  no client receives the captured ordering.
- Migration/schema: no change.
- Operational: no query cost.
- Risk: this is a material native-v1 pagination/order difference and cannot be treated as a
  comparator rule without decision-owner approval.

### C — Introduce one authoritative catalog presentation order

Add an explicit, durable presentation-order authority shared by migrated and newly ingested
catalog entities, then use it in every affected surface.

- Compatibility: can reproduce the frozen order and give future writes a documented position.
- Migration/schema: requires a schema ADR, forward/backward migration plan, importer behavior,
  write-order semantics, and tests for insert/reorder/concurrent pagination.
- Operational: adds write and query maintenance and requires backfill/reconciliation evidence.
- Risk: broadest change and highest rollback cost before 1.0.

## Recommendation

The local recommendation is **A only if the decision owner accepts a clearly specified fallback
for unmapped rows**; otherwise choose B explicitly for 1.0 and publish the compatibility impact.
Option C should not be introduced during R4 without a separate accepted schema ADR. No option is
implemented by this package.

## Required decision

Please accept, reject, or revise A/B/C and specify:

1. whether default native-v1 Artist/Release pagination must match the frozen source sequence;
2. the ordering and fallback for entities without a legacy mapping;
3. whether the same ordering is required for OpenSubsonic/Jellyfin projections or remains
   surface-specific; and
4. the performance budget and migration/client plan for the selected option.

Until this is decided, the 117 catalog ordering/projection differences remain unclassified and
R4.4 remains Partial.
