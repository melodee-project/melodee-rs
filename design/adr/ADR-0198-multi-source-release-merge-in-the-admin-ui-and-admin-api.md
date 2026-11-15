# ADR-0198: Multi-source Release merge in the Admin UI and Admin API

- **Date:** 2026-11-15
- **Status:** Accepted
- **Decision owners:** Catalog curation, Admin API, Admin UI
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-ADMIN-002, PRD-CATALOG-001, PRD-ARTIST-010,
  PRD-UI-005

## Context

ADR-0196 exposed the durable, audited, reversible Release merge as
`POST /api/admin/v1/catalog/releases/merge` (one source, one target) plus preview and split.
That covered harnesses and the CLI-style single correction, but not the workflow an
administrator actually performs on a duplicated catalog: several Releases that represent one
release — typically imported under different Artists — are selected together and folded into the
one the operator designates as canonical.

The C# baseline had no such UI. Its Blazor "Merge Albums" button opened an informational dialog
whose only action closed it (`Data/Albums.razor` at commit `4028b58d`); the only working
Album merge was `ArtistService.MergeAlbumsAsync`, reachable from the
`AlbumFindDuplicateDirsCommand` CLI, keyed by filesystem directories, artist-scoped, deleting
the source Album rows and moving/deleting the source directories. That shape contradicts this
repository's rules: paths never determine identity (SPEC), corrections are audited and
reversible, and a Release of Tracks must never be recreated as one source Release's row while
another source's row disappears.

Three gaps had to be closed to make the operator flow real and complete:

1. **Batch semantics.** Selecting N sources must be one atomic decision, not N independent
   merges an operator can half-apply.
2. **Artwork.** Both the operator override (`catalog_entity_images`) and provider artwork
   (`catalog_provider_images`) are keyed by Release, so a source's catalog artwork would
   otherwise never surface on the survivor.
3. **Release-level billing.** `release_artists` rows reference `observed_artists` through a
   composite foreign key that is **not** deferrable, while merge moves those observations with
   their Tracks. Any Release whose billing was observation-linked would fail the merge outright.

## Decision

1. Add `POST /api/admin/v1/catalog/releases/merge/batch` accepting
   `{"sourceReleaseIds": [...], "targetReleaseId": "...", "reason": "..."}`. Selection validation
   is shared with the existing single-source route: 1–100 distinct sources, the target may not
   also be a source, every Release must exist and be `active`. The whole batch executes in one
   PostgreSQL transaction through `CatalogStore::merge_releases`, records one
   `release_merge_events` row per source and returns the `(sourceReleaseId, mergeEventId)`
   pairs (so each source can be split independently), and rolls back entirely if any source
   cannot merge.
2. The Admin UI exposes it on **Catalog → Releases**: checkbox selection across the page (any
   Artist), a **Surviving Release** select, an audit reason, the server-calculated impact preview,
   and a typed `MERGE` confirmation. The page-scoped selection mirrors the existing
   multi-Artist merge, so both identity corrections read the same way.
3. **The surviving Release keeps the target's metadata, Release artist, and artwork.**
   Selection never changes who the Release is credited to; the operator chooses the canonical
   Release, and the merged result inherits its Release-level billing. Source `release_artists`
   rows stay on the merge tombstone as evidence.
4. **Merge scope beyond Tracks.** For each source, in the same transaction as its Tracks,
   observations, and contributions:
   - Release billing rows whose observation moves with a Track are detached
     (`observed_artist_id = NULL`) immediately before the observation moves, and the exact
     `(release_artist_id, observed_artist_id)` pairs are recorded in the merge event;
   - per-user Release state (star, hate, rating, pin, play count) moves for every user who has no
     state on the surviving Release; a user who has both keeps the survivor's row, and the moved
     user ids are recorded so the split can move the rows back;
   - the source operator cover is adopted only when the target has none, and only from the
     lowest-id source, so the choice is deterministic;
   - provider artwork is adopted per provider the target has no image for;
   - source artwork that cannot be adopted is preserved on the source tombstone rather than
     deleted.
5. **Immutable policy evidence is never rewritten.** `track_novelty` and
   `release_novelty_evaluations` are append-only (ADR-0120) and the aggregate is unique per
   evaluated Release, so novelty evidence cannot follow a merged Track. The rows keep the Release
   whose evaluation recorded them. Migration `0048` therefore narrows
   `track_novelty_exact_track_fk` from the composite
   `(track_id, release_id, audio_identity_id, media_asset_id)` reference to `track_id` alone.
   Without that change, **no** policy-evaluated Release could be merged at all: the composite
   reference pinned the Track's release coordinate, which a merge must change, while the trigger
   correctly forbids updating the evidence. A merge therefore leaves the surviving Release's own
   evaluation as its policy history and does not re-evaluate it; that limitation is explicit and
   re-evaluation remains a separate future workflow.
6. **Reversibility is extended, not weakened.** Migration `0048` adds
   `release_merge_events.moved_assets jsonb NOT NULL DEFAULT '{}'::jsonb` holding the detached
   billing links, the adopted operator cover (with its adoption timestamp), and the adopted
   provider image ids. `CatalogStore::split_release_merge` restores Tracks, observations,
   contributions, billing links, and artwork. A cover adopted by the merge is left in place when
   an operator uploaded a newer one after the merge: newer curation wins, and the guard is the
   recorded adoption timestamp.
7. `POST /api/admin/v1/catalog/releases/merge/preview` accepts the batch shape
   (`sourceReleaseIds`), still accepts the legacy single `sourceReleaseId`, and returns the
   aggregated impact plus the new `artwork` section and `sources` array. Existing response fields
   (`source`, `impact.*`, `metadataConflicts`, `warnings`) keep their meaning, so the original
   ADR-0196 contract remains valid.
8. No native-v1, OpenSubsonic, or Jellyfin route changes. Merged Releases remain redirect
   tombstones with the same public ids, and no media asset, Track, or file is deleted or moved.

## Alternatives considered

### Merge Releases by artist-scoped directory like the C# CLI

Rejected. It requires identical directory ancestry, cannot express "these two Releases under
different Artists are the same release", mutates canonical media, and yields no reversible audit
trail. Release identity is a catalog fact, not a path fact.

### Move `release_artists` rows to the target

Rejected. It would credit the surviving Release to the source Artists — exactly the outcome the
operator is correcting — and would collide with the target's `(release_id, presentation_order)`
uniqueness. It would also contradict the stated requirement that the chosen Release's Artist wins.

### Delete the source billing rows

Rejected. Billing is evidence; deleting it would silently discard provenance that the tombstone
is designed to retain. Detaching the observation pointer keeps the row and is restored by split.

### Copy adopted artwork bytes instead of moving the rows

Rejected. `catalog_entity_images` holds up to 10 MiB per entity and its pair-uniqueness is the
contract; moving the row keeps one owner per Release and makes restoration exact.

### Require the operator to fix artwork and billing before merging

Rejected. It pushes mechanical cleanup onto the operator and leaves the common case (a duplicate
with the only cover) broken. Deterministic adoption plus a recorded, reversible event is simpler
to reason about and to audit.

## Rationale

The operator-visible task is "these several Releases are one release, and *that* one is
canonical". Expressing it as one bounded, audited transaction keeps the catalog consistent at
every commit boundary, keeps the surviving Release's identity under explicit operator control,
and extends the existing merge/split evidence model instead of inventing a second one.

## Consequences

### Positive

- Administrators can fold several Releases from any Artist into one with preview, reason, and
  typed confirmation — the workflow the C# product never provided.
- Tracks, observations, contributions, artwork, and billing evidence are all preserved; the split
  restores exactly what moved.
- Single-source API clients keep working unchanged.

### Negative / trade-offs

- Policy novelty evidence keeps its evaluated Release rather than following moved Tracks, so the
  surviving Release is not re-evaluated by a merge and its stored novelty aggregate no longer
  covers the moved Tracks. Re-evaluation is a separate workflow; the evidence itself is never
  rewritten or lost.
- `track_novelty_exact_track_fk` is narrower than before: it guarantees the referenced Track
  exists, not that the Track still belongs to the recorded Release. A merged Track legitimately
  belongs to another Release.
- Source Release-level billing stays on the tombstone rather than being coalesced, so a split
  needs no billing re-derivation, but the tombstone retains billing rows whose observation link
  was detached. The recorded pairs make this explicit and reversible.
- Per-user Release state is not summed: a user with state on both Releases keeps the surviving
  Release's row and the source row stays on the tombstone. This preserves every user's data
  exactly and keeps the split lossless.
- Only one source cover can win; other source covers remain on their tombstones, which the split
  exposes again.
- A second, independent merge of the same target can no longer adopt artwork, because the target
  already owns a cover after the first adoption. That is intentional and deterministic.

### Follow-up work

- Re-evaluating the surviving Release's policy facts after a merge, which requires allowing a new
  `release_novelty_evaluations` row per Release (currently one immutable row per Release).
- A Release duplicate-review screen that surfaces
  `GET /api/admin/v1/catalog/duplicates/releases` candidates with their evidence and blockers,
  pre-filling the merge dialog.
- Optional preview of the exact Tracks that will share audio identities after a merge.

## Validation

- `crates/melodee-persistence/tests/catalog_release_merge.rs`: multi-source fold, target artist
  retention, artwork adoption and split restoration, cover-replacement guard, billing detach and
  restore, per-user Release state move/retain/restore, immutable novelty evidence surviving a
  merge of a policy-evaluated Release (previously impossible), whole-batch rollback, and
  selection validation.
- `apps/melodee-server/tests/admin_duplicates_api.rs`: batch preview/apply over three Releases
  from three Artists, adoption through the API, bounded validation errors (`duplicate_source`,
  `self_merge`, `invalid_reason`, `release_not_mergeable`), and split restoration.
- `admin-ui/src/screens/catalog/CatalogListScreens.test.tsx` and
  `admin-ui/e2e/catalog.spec.ts`: page-scoped selection, surviving-Release choice, preview
  request shape, typed confirmation, and the batch request payload.
- `crates/melodee-persistence/tests/catalog_import.rs` and
  `apps/melodee-server/tests/admin_duplicates_api.rs` keep the pre-existing single-source and
  reversible-merge proofs green.

## References

- [ADR-0162](ADR-0162-durable-provider-enrichment-tasks-and-release-external-evidence.md)
- [ADR-0196](ADR-0196-expose-duplicate-candidate-scans-and-release-identity-correction-in-the-admin-api.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
