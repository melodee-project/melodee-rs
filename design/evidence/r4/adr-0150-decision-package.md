# ADR-0150 decision package — legacy media metadata boundary

Status: Decision required. ADR-0150 remains **Proposed** and has not been
implemented or accepted by this work.

Date: 2026-08-20
Source evidence: the current committed surface reports
[`dual-run-report-native.json`](dual-run-report-native.json),
[`dual-run-report-opensubsonic.json`](dual-run-report-opensubsonic.json), and
[`dual-run-report-jellyfin.json`](dual-run-report-jellyfin.json). They record
181, 330, and 17 unexplained field differences respectively. The native report includes
the current native-v1 playlist compatibility correction; no comparator mask is added here.

## Decision requested

The release owner must choose and record the ADR-0150 disposition for legacy
metadata that has no authoritative native-v1 Rust field. The choice affects
genres, bit depth, free-form metadata, aggregate play counters, and the
ordering consequences of legacy nullable `SortName` values.

## Affected evidence

| Cluster | Current count or evidence | Compatibility impact |
|---|---:|---|
| Native catalog list projection/order | 117 field differences: 63 `catalog/albums-list`, 32 `catalog/artists-list-page2`, 22 `catalog/artists-list` | Native v1 list pages contain different order/identity positions after migration. The legacy captures sort nullable/legacy metadata differently from the Rust non-null catalog invariant. |
| Genre/reference routes | 20 fields/routes, including `catalog/genres-list`, `native-user-extra/genre-*`, and starred/hated genre lists | Legacy genre values have no accepted authoritative Rust catalog field; implementing a mapping would change native and OpenSubsonic projections. |
| Podcast discovery | 10 live/provider fields across trending and OPML | Values are external/provider-time dependent and cannot be replayed as frozen catalog authority. |
| User state and counters | Scattered differences in ratings/starred flags, play counts, and recent artist payloads | Source fixture and historical captures exercise different state boundaries; changing either side risks client-visible user-state semantics. |
| Biography and timestamps | Scattered biography and account timestamp fields | Biography is not imported as authoritative catalog data; fixture user timestamps are generated at seed time. |
| Shares/playlists | Description/order and update behavior differences | Requires explicit compatibility disposition before changing the native share contract. |
| Disabled playback backend | 3 response fields in `native-discovery/backend-register` | Must be reviewed under PRD-PARTY-003 and ADR-0123; absence of Party/Jukebox behavior is not a blanket comparator waiver. |
| Provider-backed artist lookup | 1 candidate result difference | ADR-0136 defers non-Last.fm providers; the captured provider result cannot be synthesized locally. |
| Jellyfin metadata companion replay | 17 fields: 9 genre projections, 3 instant-mix cardinality fields, and 3 playlist Overview fields; migrated WAV container/codec and artist ChildCount residuals are corrected by focused tests | Genres and instant-mix membership depend on the unresolved metadata boundary. Playlist `Comment` versus native description is a separate cross-product projection conflict and needs an explicit compatibility decision; it is not masked. |

The total is a field-level count, not a count of independent product defects.
The exact scenario, field, expected value, and actual value remain in the
machine-readable report.

## Current cross-surface impact inventory

The following inventory makes the decision scope explicit without pretending
that overlapping field families are independent defects. Counts are calculated
from the current `unexpected` arrays; family rows may overlap when one
projection exposes more than one unresolved source value.

| Surface / family | Current evidence | Compatibility impact | Schema impact if implemented | 1.0 recommendation |
|---|---:|---|---|---|
| Native catalog order/projection | 117 positional/list fields | Page membership, IDs, names, counts, and URLs differ together | None for reference-only; an accepted ordering/schema decision is required for a durable presentation order | Decide via the separate catalog-ordering package; do not mask |
| Native legacy metadata/state families | 64 fields outside the 117-entry order cluster, including genre, counters, ratings/starred state, biographies, shares, backend errors, and provider lookup | Client-visible fields and some statuses differ; semantics are not interchangeable | Reference-only retains issue evidence; projected fields require schema ownership, backfill, and rollback | Keep reference-only unless the owner selects a field-specific projection |
| OpenSubsonic metadata/state projections | 330 total; family counts remain overlapping and are reported in the machine-readable surface report | XML/JSON fields, genre lists, annotations, playlist/share views, and counts differ | Any durable genre, bit-depth, comment, or counter projection requires a schema ADR and adapter-specific mapping | Do not implement or mask before disposition |
| Jellyfin companion projections | 17 total; 11 genre-related, 3 playlist Overview, and 3 instant-mix/similar cardinality fields | Music metadata, playlist descriptions, and recommendation cardinality differ | Genre/comment projections require durable ownership; instant-mix requires an explicit membership rule | Preserve current authority or obtain a field-specific decision |
| Live/provider-backed values | 10 native podcast fields plus one provider artist lookup field | Values are provider/time dependent and cannot be established from the frozen catalog | No catalog schema should be invented for live responses | Validate externally or retain as an explicit exclusion under accepted authority |

The 64-field native remainder is `181 - 117`; it is a residual scope count,
not a waiver. The OpenSubsonic family counts intentionally overlap and therefore
must not be summed to derive a total.

## Decision matrix

| Disposition | Compatibility contract | Migration/data impact | Operational and rollback impact | Recommendation |
|---|---|---|---|---|
| Reference-only metadata | Native authority remains unchanged; affected legacy-only adapter fields/routes are explicitly excluded with a client migration note | No schema change; retain source-derived values as durable migration issues and never use them as identity | Lowest risk; restore and rebuild remain deterministic from PostgreSQL authority | Recommended for 1.0 |
| Accepted schema-backed projection | Each field gets an owner, normalization/update rule, surface-specific projection, and compatibility tests | New migration, backup/restore coverage, backfill, and rollback plan are mandatory | Requires capacity, rebuild, failure recovery, and cutover evidence before release | Only after a new accepted schema ADR |
| Bounded adapter reconstruction | Only named fields are reconstructed from retained source evidence at the adapter boundary | Requires a bounded retained source, lifecycle, restore semantics, and no shared DTO or identity use | Differential and client tests must prove every selected field and failure path | Viable only for explicitly selected fields |

The release owner must record the selected disposition for each affected field
family, not merely accept a broad label. A decision that selects a projection
must name the schema authority, migration plan, rollback trigger, compatibility
surfaces, and client communication before implementation begins.

## Alternatives

1. **Keep reference-only metadata out of authoritative catalog fields**
   (recommended under the current proposed ADR). Preserve every value as a
   durable migration issue, retain the source evidence, and document that the
   corresponding legacy-only projections are not part of the native Rust
   authority. This requires an explicit release exclusion for affected client
   fields/routes and a migration/client plan.
2. **Add an accepted schema-backed metadata projection.** Define ownership,
   normalization, update semantics, API projections, OpenSubsonic/Jellyfin
   consequences, backfill behavior, and rollback before implementation. This
   requires a schema ADR and forward/recovery migration; it is not safe to
   implement from this package alone.
3. **Compatibility-only adapter reconstruction.** Reconstruct selected legacy
   fields at the adapter boundary from retained reference evidence. This keeps
   core authority separate but requires a precise field-by-field contract,
   bounded storage source, and client/migration plan. It must not become a
   shared wire DTO or a hidden identity key.

## Recommendation

Accept alternative 1 for 1.0 unless the release owner explicitly requires
specific legacy metadata projections. If alternative 2 or 3 is selected, keep
R4.4 open, update ADR-0150 and the affected requirements, add migration and
compatibility tests first, and rerun all three surface differentials. Do not
add broad volatile-field masks to make the current report green.

## Open decisions outside ADR-0150

### Playlist `Comment` versus native `description`

The frozen source contains both values for the public playlist (`Comment` is
“Synthetic public playlist spanning multiple Releases.” and `Description` is
“Fixture playlist covering four different Releases.”); the private playlist
has a `Comment` and no `Description`. The target currently has one
`native_playlists.description` field, so native v1 correctly projects the
imported `Description` while OpenSubsonic and Jellyfin differ on three
captured `comment`/`Overview` fields. This is a real three-product projection
conflict, not a volatile-field mask.

The release owner must choose one of these compatible dispositions:

1. **Preserve native authority and accept the adapter gap for 1.0.** Keep the
   current target schema and document the OpenSubsonic/Jellyfin playlist
   comment limitation with a client migration note.
2. **Add a durable legacy-comment projection.** Introduce a schema-backed
   source-comment field/table through a new accepted schema ADR, migrate and
   back it up, then project it only in the OpenSubsonic/Jellyfin adapters.
3. **Use a compatibility-only retained source projection.** Define an
   explicit bounded migration evidence source and lifecycle for the comment;
   this still requires an accepted ADR and restore/rollback tests and must not
   alter native v1 or core identity.

Recommendation: choose option 1 for 1.0 unless current OpenSubsonic/Jellyfin
clients require the comment. Options 2 and 3 both keep R4.4 open until the
decision, schema/evidence authority, migration plan, and all three differential
replays are complete.

This package also does not decide the separate catalog ordering, user-state,
share/update, playback-backend, live podcast, or provider differences. Each
needs either an implementation correction supported by a focused test or a
named product/ADR disposition. ADR-0149 and ADR-0151 remain Proposed as well.
