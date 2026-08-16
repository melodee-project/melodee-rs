# Phase 3.3 Evidence — Verification and canonical publication

**Status:** Complete; final clean-checkout gate results are recorded in the consolidated
[Phase 3 report](../PHASE-3-REPORT.md).
**Authority:** PRD-INGEST-005 through PRD-INGEST-010, PRD-STORAGE-001/002/004, SPEC sections
11–14 and 16, and ADR-0105/0106/0108/0110/0115/0116.

## Result

Phase 3.3 turns a completed normalization manifest and its attempt-isolated work area into a
verified canonical Release bundle. It adds structural validation, reserves one immutable UUIDv7
`ReleaseId`, persists immutable publication intent before target I/O, upgrades the manifest from
v2 to v3, and converges filesystem and PostgreSQL state after retries or process loss.

This is deliberately narrower than Phase 5 policy. The only authorization emitted here is
`phase3.structural-publication` version 1, and the inbound source disposition is always
`retained`. Phase 3 never moves, renames, rewrites, or deletes inbound media.

> Historical evidence note: ADR-0145 (accepted 2026-08-15) supersedes the inbound-retention
> interpretation recorded here. Under the accepted durable-staging lifecycle, canonical
> publication records source disposition `retained` for the *staged* source, and an acquired
> inbound candidate directory is removed after its verified durable staging receipt. Phase 3's
> no-move/no-rename/no-rewrite guarantees apply to inbound bytes before that verified handoff.

## Manifest v3 and validation boundary

`crates/melodee-domain/src/manifest.rs` keeps inspection v1 and normalization v2 readable and adds
publication v3 facts:

- the stable `ReleaseId` reserved for the later catalog;
- structural validation profile, version, outcome, and findings;
- the durable authorization identifier/name/version;
- source disposition `retained`;
- canonical layout version and publication-plan hash; and
- a deterministically sorted inventory mapping source and normalized logical paths to canonical
  asset kind, path, size, and BLAKE3 hash.

The v3 upgrade does not change the semantic manifest hash: publication intent, worker/attempt/time,
absolute roots, and tool execution provenance remain outside the observed-media identity. A v1
manifest cannot publish directly; it must first become a complete v2 normalization manifest.

Structural validation fails closed on missing, duplicate, mismatched, unhashable, non-regular,
symlinked, traversal-bearing, or cross-attempt work assets. Missing or ambiguous local Release or
Track presentation/ordering facts produce durable `NeedsReview` findings instead of guessed Artist
or catalog identity. A passing manifest contains the observations, technical media facts, exact
byte hashes, acoustic fingerprints, normalized artwork/sidecars, and canonical mapping that Phase 4
can consume without reopening audio merely to rediscover those facts. Identity resolution and
catalog persistence remain Phase 4 work.

## Canonical layout v1

ADR-0116's exact storage-root-relative layout is implemented:

```text
releases/
  <shard-0>/
    <shard-1>/
      rel_<release-uuid-simple>/
        manifest.json
        media/<blake3>.mp3
        artwork/<blake3>.jpg
```

The two lowercase hexadecimal shards are the first two bytes of BLAKE3 over the Release UUID's raw
16 bytes. Canonical paths contain lowercase ASCII only and derive exclusively from the immutable
Release ID, asset kind, and byte hash. Artist names, titles, source paths, and the configured
storage root do not determine physical identity. The golden vector is:

```text
ReleaseId 01989f00-7000-7000-8000-000000000001
-> releases/a2/49/rel_01989f00700070008000000000000001
```

`manifest.json` describes the plan but is not recursively listed as one of its own assets.
Identical bytes within a Release share one planned physical asset; global deduplication is not a
Phase 3 requirement.

## Durable convergence protocol

Migration `0006_canonical_publication.sql` adds four additive evidence tables:

1. `release_publication_authorizations` stores the one structural authorization and Release ID per
   work item.
2. `release_publication_plans` stores the immutable layout/version, input semantic manifest hash,
   portable target/manifest paths, complete plan JSON, and plan hash.
3. `release_publication_journal` stores idempotent milestones (`incoming_prepared`, `asset_copied`,
   `bundle_sealed`, `finalized`, `final_adopted`, `target_conflict`, or `failed`).
4. `release_publication_receipts` stores the final Release/plan identity, sealed manifest hash,
   retained disposition, and publication time.

Validation and publication claims use PostgreSQL leases and recheck the current owner/expiry at the
authoritative write. Validation atomically persists the authorization, plan, v3 manifest, event,
and `Validating -> Staged` transition, then clears its lease. Publication claims
`Staged/Publishing`, and `Publishing -> Published` is allowed only after the exact planned assets,
sealed bundle, and matching finalization/adoption journal evidence exist. Completion writes the
receipt, state/event, and lease release transactionally. A stale worker cannot authorize, journal,
or complete another worker's item.

On the target filesystem, publication:

1. validates the plan/manifest/work-root contract before canonical target I/O;
2. rejects overlapping or symlinked inbound, work-area, and canonical roots;
3. takes a per-Release publication lock and writes only to
   `.<release-directory>.incoming` under the final parent;
4. copies regular non-symlink assets through `.copying` files, streaming and verifying size/BLAKE3,
   then fsyncs files;
5. writes and verifies manifest v3, verifies the sealed tree, and fsyncs the incoming tree;
6. uses a kernel-enforced no-replace rename to expose the complete Release, then fsyncs the parent;
   and
7. verifies the final again before returning a receipt for the PostgreSQL completion transaction.

Unix no-follow opens and `renameat(..., RENAME_NOREPLACE)` are provided by the safe `rustix` 1.1.4
API with narrow `fs`/standard-library interop features. Windows retains its non-replacing directory
rename behavior. The dependency is FOSS and introduces no local `unsafe`; its decision and rejected
TOCTOU alternative are recorded in ADR-0116.

## Retry, fault, and safety evidence

| Boundary | Executable evidence | Guaranteed result |
| --- | --- | --- |
| Structural inventory | `fingerprint_and_output_inventories_fail_closed_when_missing_duplicate_or_incomplete` | Incomplete normalization facts never reach canonical I/O. |
| Unsafe path/provenance | `traversal_and_cross_attempt_provenance_are_rejected_before_target_io` | Traversal and sibling-attempt substitution fail closed. |
| Symlink boundaries | `symlinked_work_roots_nested_assets_and_storage_components_are_rejected`; `symlink_final_is_a_no_clobber_conflict` | Publication neither follows nor overwrites a symlink boundary. |
| Byte/plan tampering | `normalized_byte_hash_tampering_is_rejected_without_source_mutation`; `plan_size_tamper_is_detected_before_any_final_is_visible` | Size/hash mismatch leaves no visible final and does not alter source. |
| File/manifest/tree fsync faults | `injected_fsync_failures_leave_no_final_and_retry_converges` | Deterministic `AssetFsync`, `ManifestFsync`, and `SealedTreeFsync` errors leave no final; retry publishes successfully. |
| Partial incoming work | `a_stale_incoming_bundle_is_resumed_and_atomically_finalized` | Retry verifies/reuses matching copied bytes and publishes one complete final. |
| Existing final | `canonical_publication_is_complete_v3_idempotent_and_source_retaining`; `empty_and_mismatching_finals_fail_closed_without_clobbering` | An exact final is adopted; empty/different final content is preserved and reported as conflict. |
| Cancellation | `cancellation_leaves_no_final_and_never_mutates_the_source` | Cancellation cannot expose a final or mutate inbound bytes. |
| Lease/process loss | `expired_publication_lease_remains_convergent_and_directly_reclaimable`; `concurrent_validation_and_publication_are_lease_fenced_to_one_result` | Expired work is reclaimed directly; concurrent workers produce one authority/plan/receipt. |
| Lease theft during copy | `publication_lease_theft_cancels_stale_copy_and_direct_retry_converges` | Only incoming preparation is journaled before theft; the stale worker cannot seal/finalize/receipt, and direct retry publishes once without source mutation or attempt penalty. |
| Rename syscall fault | `injected_final_rename_failure_leaves_no_final_and_retry_converges` | A pre-rename fault leaves no final and retry publishes successfully. |
| Parent fsync after rename | `injected_post_rename_parent_fsync_failure_is_adopted_on_retry` | The complete final remains visible; retry verifies/adopts it rather than copying or overwriting. |
| Rename before database completion | `final_rename_before_database_completion_is_reclaimed_and_adopted_once` | Retry adopts filesystem truth and records exactly one receipt and Published transition. |
| Conflicting final through worker/DB | `conflicting_final_is_not_overwritten_and_remains_directly_reclaimable` | Conflict bytes remain unchanged, the immutable plan remains visible, and a later worker may re-evaluate it. |
| Durable review | `missing_presentation_is_durably_routed_to_needs_review_before_canonical_io` | Ambiguous presentation persists findings and stops before canonical I/O. |
| Database completion evidence | `authorization_stage_publish_is_fenced_and_durable` | A receipt cannot commit without exact asset, sealed-bundle, and final journal evidence. |
| Source disposition | the success, tamper, symlink, cancellation, crash-recovery, pathology, and 200-Release tests compare source snapshots | Every Phase 3 outcome retains the source tree byte-for-byte. (ADR-0145: the retained tree is the durable staged source; the acquired inbound candidate is drained after its verified receipt.) |

All successful publication paths execute file/directory fsyncs, and unsupported directory fsync is
handled only for the platform's explicit `InvalidInput`/`Unsupported` result. Deterministic fault
injection covers asset, manifest, sealed-tree, final-rename, and post-rename parent-fsync boundaries.
The post-rename case correctly treats a visible complete final as filesystem truth and adopts it on
retry.

The feature-gated lease-theft run completed in 2.53 seconds. Before forced theft, the stale worker
reported `claimed=1`, `completed=0`, `failed=0`, and `errors=1`; only `IncomingPrepared` was durable,
with no later journal milestone, visible final, or receipt. The target contained only hidden
incoming/`.copying` partial work. Direct reclaim after expiry reached Published with one receipt,
unique milestones, unchanged inbound bytes, and no retry-attempt penalty.

## Workers, CLI, and readiness

- Validation and publication use independent positive concurrency settings:
  `MELODEE__VALIDATION__CONCURRENCY` and `MELODEE__PUBLICATION__CONCURRENCY` (both default to 2).
- Each stage claims no more than its configured bound and runs one independent leased task per
  Release with cancellation and lease heartbeats.
- Publication is disabled unless `MELODEE__STORAGE__ROOT` is explicitly set and all configured roots
  are absolute/symlink-free/disjoint after resolution.
- `melodee ingest validate`, `publish`, `status`, `manifest`, and `publication` expose bounded stage
  execution and the durable authorization/plan/receipt/journal evidence.
- `/healthz` remains liveness-only. The additive Rust-native `/readyz` probe performs a bounded
  PostgreSQL `SELECT 1`, returns `200/ready` or `503/not_ready`, and never exposes connection data.

No native Melodee v1, OpenSubsonic, or Jellyfin route, DTO, identifier, status, error, auth, range,
or negative-behavior contract changed.

## Phase 3.4 acceptance evidence

`crates/melodee-ingestion/tests/normalization.rs` extends the real-tool vertical pipeline through
validation and publication:

- `real_mp3_flac_ogg_wav_unicode_artwork_and_sidecars_reach_published` covers real MP3, FLAC,
  OGG/Vorbis, and WAV; Unicode and leading-dash paths; normalized artwork; CUE/NFO/M3U/LRC/SFV
  semantics; a rejected outbound symlink; manifest v3; no partial paths; and unchanged sources.
- `two_hundred_releases_reach_published_once_with_zero_loss_and_bounded_concurrency` asserts 200
  durable authorizations, plans, distinct Release IDs, receipts, final journal outcomes, Published
  transitions, and canonical manifests, with zero duplicate authority, failure, partial final, or
  source mutation. Normalization reaches but never exceeds its configured concurrency; validation
  and publication claim bounds are both configured to 8.

Observed output from the final execution (not inferred from assertions):

```text
phase3.4-acceptance={"total":200,"normalized":200,"validated":200,"published":200,"expected_failure":0,"unexpected_failure":0,"lost":0,"duplicate_authoritative_result":0,"source_mutations":0,"publication_receipts":200,"configured_normalization_concurrency":8,"measured_normalization_concurrency":8,"configured_validation_concurrency":8,"configured_publication_concurrency":8,"elapsed_ms":23177}
```

The complete 11-test normalization binary took 78.16 seconds including database creation,
discovery, settling, inspection, normalization fixture work, validation/publication assertions,
and teardown. The `elapsed_ms` value is the measured normalization-through-publication acceptance
window.

## Reproduction

```bash
cargo test -p melodee-domain --test manifest_versions
cargo test -p melodee-persistence --test publication
cargo test -p melodee-ingestion --lib
cargo test -p melodee-ingestion --test publication
cargo test -p melodee-ingestion --test normalization -- --nocapture
./scripts/check.sh
```

The real media/vertical tests require FFmpeg/ffprobe with libmp3lame, Chromaprint `fpcalc`, and the
documented development PostgreSQL service. The final command/result capture is consolidated in
[the Phase 3 report](../PHASE-3-REPORT.md).

## Data, migration, and remaining scope

Migration 0006 is additive and preserves existing work items/manifests. Rollback does not drop its
authorization, plan, journal, or receipt evidence: an older binary may be restored while these
tables remain, and a database backup/restore is required if a full schema rollback is operationally
necessary. There is no destructive media migration.

Phase 4 must consume the reserved Release ID and manifest rather than allocate/re-probe. Phase 5
owns configurable classification/novelty/disposition policy. Phase 11 owns registration of existing
canonical media without forced movement or re-encoding. CUE-driven audio splitting, global physical
deduplication, and stale attempt cleanup remain outside this slice.

Documentation impact:
- Updated — public configuration, storage, inbound processing, diagnostics, Admin API, concepts,
  and troubleshooting pages describe the implemented publication/readiness behavior; internal
  architecture, operations, evidence, traceability, and release status are updated with closure.
