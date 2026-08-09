# Phase 1B Evidence — Durable Inbound Discovery and Settling

**Date:** 2026-08-09  
**Status:** Complete  
**Authority:** SPEC.md sections 11, 11.3, 11.4, 12–14; PRD-INGEST-003/004/005; PRD-WORK-003/004

## 1) Scope implemented

- Added `melodee-ingestion` crate with:
  - inbound source model/config
  - bounded periodic reconciliation loop
  - deterministic filesystem discovery of candidate Releases
  - persistent settling snapshots
  - durable state transitions `discovered -> settling -> ready`
  - ready-marker handling and marker-shortcut logic
  - duplicate-safe concurrency semantics
  - CLI diagnostics for one-shot reconcile/status
- Extended persistence schema to track settling evidence and stable state:
  - `release_work_items.latest_settling_snapshot_id`
  - `release_work_items.stable_since`
  - `release_work_items.last_observed_at`
  - unique source-identity constraint on `(source_library_id, source_path)`
  - new `release_work_item_settling_snapshots`
  - new `release_work_item_settling_snapshot_files`
- Integrated reconciliation startup into `melodee-server`.

## 2) Architecture and schema

### Source identity and idempotent discovery

- `source_library_id + source_path` is the canonical identity.
- `create_or_get` uses `ON CONFLICT (...) DO NOTHING` against
  `COALESCE(source_library_id::text, '__default__') || '|' || source_path`.
- Repeated scans for the same candidate re-use a single durable item.

### Settling persistence model

Each candidate gets durable snapshots in `release_work_item_settling_snapshots` with:

- observed timestamp
- deterministic signature
- file_count / total_bytes
- ready-marker seen flag
- bypassed-settling flag

Per-file metadata is stored in `release_work_item_settling_snapshot_files` (`relative_path`, `file_type`, `file_size`, `modified_at`), keyed by snapshot id.

`release_work_items.stable_since`, `latest_settling_snapshot_id`, `last_observed_at` are updated in-place during reconciliation.

## 3) Snapshot algorithm

- Deterministically traverse every entry (`read_dir` sorted by filename) including nested files.
- Build file entries with:
  - relative path (POSIX-style)
  - file type (`dir`, `file`, `symlink`, `other`)
  - file size
  - modified time in UTC nanos
- Sort entries by path/type/size
- Signature = concatenation of row fields across all entries (no byte-content hashing)
- The signature is compared against latest snapshot for stability decisions.

## 4) Settling algorithm

For each scanned candidate:

1. ensure/create durable work item (`Discovered` if new)
2. compute snapshot
3. persist new snapshot when signature changed
4. transition logic (system-owned):
   - `Discovered -> Settling`
   - `Settling -> Settling` and reset stable time when signature changed
   - `Settling -> Ready` when unchanged for configured quiet period
   - `Ready -> Settling` when a stable candidate changes (re-entrant)
5. keep items durable when source disappears; do not delete the queue row.

Marker behavior:

- ready marker default `.melodee-ready`
- when marker seen, quiet period can be shortened via `MELODEE__INGEST__READY_MARKER_SHORTCUT_SECONDS`.

## 5) Filesystem safety

- Root and candidate discovery are canonicalized and normalized.
- Relative path derivation does not rely on simple string-prefix checks.
- Symlink candidate entries are resolved and rejected if the resolved target escapes source root.
- Discovery only targets immediate child directories of each inbound root (unless path policy changes).

## 6) Observability evidence

Reconciliation and settling paths emit structured trace events for discovery and transitions:
`reconciliation_started`, `reconciliation_completed`, `release_discovered`, `snapshot_changed`, `settling_started`, `ready_marker_observed`, `release_ready`, `source_missing`.

## 7) Scheduling and lifecycle

- Server starts periodic reconcile loop when inbound config is present/enabled.
- Scheduler deduplicates overlapping scans in-process.
- Loop supports graceful stop (`reconcile_handle.stop()` on shutdown).
- CLI `ingest reconcile` runs one-shot reconciliation for diagnostics and automation.

## 8) Tests and scenario evidence

- Required integration tests in `crates/melodee-ingestion/tests/reconciliation.rs` cover:
  - create/discover/reconcile durability
  - idempotent rediscovery
  - concurrent duplicate discovery
  - settled transition and stable period behavior
  - ready-marker shortcut
  - restart persistence
  - late file change -> Ready reset
  - missing/unreadable candidates are retained
  - lost watcher notification recovery via reconciliation sweep
  - concurrent settle race transitions to Ready
  - 200 candidate discovery and settle scenario
- Concurrency tests also in domain/persistence (claim semantics, duplicate-free claims, etc.) provide broader invariant support.

## 9) Performance evidence

`design/evidence/phase-1b/perf-settling-benchmark.json` was produced from ignored benchmark test
`melodee-ingestion::performance_snapshot_settling` with:

- small Release baseline pass
- large nested Release pass
- 200-Release initial reconciliation pass

## 10) Traceability

Requirements status in:

- `design/requirements/REQUIREMENTS-INDEX.md`
- `design/requirements/requirements.json`
- `design/requirements/status-overrides.json`

## 11) Limitations

- Watcher integration is not required by this phase; periodic reconciliation is the source of truth.
- Snapshot comparison uses path/type/size/mtime only (no metadata parsing, no checksum/hash of file contents).
