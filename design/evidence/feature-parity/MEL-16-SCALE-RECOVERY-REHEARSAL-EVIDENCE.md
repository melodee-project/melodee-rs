# MEL-16 — Scale and Fault-Recovery Rehearsal Evidence (100-Release Local Rehearsal)

**Status:** Partial — rehearsal evidence only. This is NOT the target-scale acceptance
MEL-16 requires; see "Remaining for full acceptance" below.

**Candidate SHA:** `89373b1c0e90070fa15c89862c4d7ff9f8542cd3`
**Evidence date:** 2026-10-19
**Topology:** Single host (`batman.home.arpa`), development build (`target/debug`),
PostgreSQL 18 container (compose.dev.yaml, `melodee-rs-dev-postgres-1` on host port 5433),
server process `melodee-server` supervised by `scripts/dev_stack.py`, storage root
`/mnt/incoming/melodee_test` (all roots on one filesystem, `/dev/sdb1`).

## Corpus

- 100 unprocessed Release directories (artist – title (year) naming), copied verbatim from
  the operator's real staging corpus (`/mnt/incoming/blackbeard/staging`). MP3-dominant,
  real-world pathologies included. Remaining 1,143 corpus releases held in
  `/mnt/incoming/melodee_test/batch_hold` for later load waves.
- 3 pre-existing work items from 2026-10-17 (already `indexing`) unchanged throughout.

## Timeline (2026-10-18 UTC, from `release_work_item_events` and server log)

| Time | Event |
|---|---|
| 21:05:51 | All 100 releases discovered in one reconciliation pass (batch discovery) |
| 21:06:21 | Settling complete → all items `ready` |
| 21:07:51 | First worker claims; inspection/normalization begins |
| 21:08:22 | First failure detected (corrupt source MP3, "Header missing") — retry scheduled |
| ~21:09:21 | All 95 healthy releases reached `indexing` (cataloged); **batch wall clock ≈ 3m30s** |
| 21:31–21:50 | Failed items retried with backoff (attempts=5), re-failed each time |
| 21:40:29 | **Fault injection:** SIGTERM → graceful shutdown mid-pipeline |
| 21:45:52 | Restart; search projection and rebuild worker started |
| 21:48:13 | Search projection checkpoint caught up: **6851/6851 outbox events applied** |

## Results

| Metric | Value |
|---|---|
| Work items discovered | 100 (+3 pre-existing = 103) |
| Cataloged (steady state `indexing`) | 98 (= 95 batch + 3 pre-existing) |
| Failed (media pathology) | 5 (all `normalized output verification failed: source MP3 failed complete decode (exit 183): Header missing`) |
| Catalog releases / tracks | 99 / 1080 |
| Catalog projection outbox events | 6851, all applied (checkpoint `tantivy.catalog` = 6851) |
| Search index generation | `gen_01a14816-e03b-70b4-a908-065fd611b39d`, 5.4 MB on disk |
| Data loss / duplicates after restart | 0 |
| Needs-review items | 0 |

## Fault-recovery findings

1. **Graceful shutdown mid-pipeline converged with no operator action.** SIGTERM during the
   `indexing` steady state caused a clean drain; on restart the search projection worker
   applied the outstanding outbox to the exact pre-shutdown high-water mark. No item was
   lost, duplicated, or reprocessed incorrectly.
2. **Retry/backoff behaves per contract.** Each corrupt-media item retried on the
   `retry.eligible` schedule (5 attempts across ~42 minutes), then parked in `failed` with
   the full decoder evidence in `state_reason` — quarantine, not deletion
   (PRD-PRINCIPLE-006).
3. **All 5 failures verified as genuine source pathologies.** Independent `ffmpeg -v error`
   complete-decode scans of the source directories confirm corrupt MP3 frames ("Header
   missing") in `5150 - Captured In My Brain (2026)`, `Antidot - Beyond Trance And Time
   (2026)`, `B.D.R - Reaper Syndrome (2026)`, `Ba Pace And Loman - Popeyes Spinach Vol. 3
   (2026)`, `Barriers - Time Crisis (2026)`. No system defect is implicated.
4. **`indexing` is the documented steady state.** Work items remain in `indexing` after the
   search projection is current; projection currency is tracked by the outbox checkpoint,
   not by a work-item transition to `Indexed` (the domain transition exists and is tested
   illegal-from-other-states, but no production actor performs `Indexing → Indexed`). This
   is consistent across all cataloged items including the 2026-10-17 cohort and is recorded
   here as an observation for MEL-18 reconciliation, not a defect.

## Performance observations (single host, debug build — non-binding)

- Batch ingestion: 100 releases discovered→cataloged in ≈3m30s wall clock, bounded worker
  concurrency visible in claims (batch=2 per claim tick), zero failures attributable to
  load.
- Catalog import passes: constant-time no-op passes (duration_ms=1–3) once the batch
  drained — consistent with the set-based, indexed import shape required by PRD-SCALE-001.
- These are debug-build single-host numbers; they demonstrate *no regression under load*
  but do not establish the PRD-SCALE/PERF budgets.

## Remaining for full MEL-16 acceptance

- Representative replacement topology approved by the release owner (this run: single dev
  host, debug build).
- Target-scale corpus and fault matrix per PRD-SCALE-001/002/003 and PRD-PERF-003/004,
  with the documented budget measurements and resource caps (the existing
  `cargo test` 200-Release fault scenarios plus `scripts/run_benchmarks.py` harnesses
  remain the reproducible measurement path).
- Release-owner approval of the final report, versioned with the release candidate SHA.

## Reproducibility

- Corpus source: operator-provided staging directory (private; not committed). Any
  100+ Release directory set with real-world pathologies is a suitable substitute.
- Start stack: `make up` with `MELODEE__*` roots pointing at the rehearsal tree.
- Ingest: copy Release directories into `MELODEE__INGEST__SOURCE_ROOT`; observe via
  `melodee-cli work list` / `work show`.
- Fault injection: `python3 scripts/dev_stack.py down` (SIGTERM path) then `up`;
  convergence verifiable via `search_projection_checkpoint` vs
  `count(catalog_projection_outbox)`.
