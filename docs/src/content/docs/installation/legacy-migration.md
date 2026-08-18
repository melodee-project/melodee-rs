---
title: Legacy C# migration tooling
description: Inventory, plan, import, verify, delta, and status commands for migrating a frozen C# Melodee database to the Rust server.
sidebar:
  order: 6
---

:::caution[Rehearsal status]
The `melodee-cli legacy` command group is implemented and covered by the repository integration
suites (import-twice idempotency, crash/restart convergence, fencing, read-only source
enforcement, unsupported-schema fail-closed, and production-shaped cardinality). The
production-sized rehearsal, dual-run differential, cutover, and rollback proof against a real
C# production snapshot is the remaining release gate before this path is called fully
supported. Treat the tooling as rehearsal-ready, not cutover-proven.
:::

The Rust server migrates an existing C# (legacy) Melodee PostgreSQL installation in place at
the database level while **adopting the canonical media library without moving, re-encoding,
re-hashing, or deleting it**. The migration is governed by the accepted migration contract
(ADR-0148) and is designed to be repeatable, restart-safe, and auditable.

## Principles

- **The source is never modified.** The legacy database is opened strictly read-only
  (`default_transaction_read_only = on`); a snapshot or replica is strongly recommended. Source
  media files are validated, never written.
- **Public identity is preserved.** Legacy `ApiKey` GUIDs (and share `ShareUniqueId` values)
  become the Rust `public_api_id` values verbatim, so existing clients, playlists, shares, and
  bookmarks keep working without a synchronized client upgrade.
- **Nothing is silently dropped.** Any source row that cannot be represented becomes a durable,
  queryable issue record with a severity and code.
- **Resume is idempotent.** Every batch commits with its checkpoint under a fenced run lease; a
  crashed or interrupted import is resumed with `import --run <uuid>` and creates no duplicate
  authority.
- **Search is rebuilt, not migrated.** The Tantivy index is regenerated from migrated
  PostgreSQL state after import; no legacy search data is copied.

## Prepare the source snapshot

1. Stop or quiesce the legacy server (or take a consistent PostgreSQL snapshot/physical
   replica).
2. Create an isolated snapshot database and verify its schema fingerprint matches the frozen
   baseline. The tooling fails closed on any other schema version.

   ```bash
   pg_dump -U melodee -d melodee --format=custom > legacy-snapshot.dump
   createdb -U melodee legacy_snapshot
   pg_restore -U melodee -d legacy_snapshot legacy-snapshot.dump
   ```

3. Grant the migration user `SELECT` on all tables. `pg_monitor` (or superuser) additionally
   lets the tooling record the source system identifier; without it a host/port/database
   identity is used instead.

4. Initialize the Rust target database with the normal schema migrations
   (`melodee-cli migrate`) before planning.

## Command reference

All commands take `--source-db` (or `MELODEE_LEGACY_SOURCE_DATABASE_URL`) for the read-only
source and use `MELODEE__DATABASE__URL` for the Rust target. Add `--json` for stable
machine-readable output suitable for rehearsal comparison.

### `legacy inventory`

Read-only source inventory: schema fingerprint, per-domain row counts and ID ranges, media
file/byte totals (from stored metadata — no media is scanned), and pre-import anomalies such as
duplicate public IDs, broken foreign keys, invalid timestamps, unsupported credential
algorithms, and unadoptable file metadata. Writes nothing to the target.

```bash
melodee-cli legacy inventory --source-db postgres://user:pass@host/legacy_snapshot
```

### `legacy plan`

Compares the inventory with target state and records a durable `planned` run without writing
target domain data. Library roots from the legacy `Libraries` table map to canonical-relative
prefixes with repeatable `--library-path-map`:

```bash
melodee-cli legacy plan \
  --source-db postgres://user:pass@host/legacy_snapshot \
  --library-path-map /music=library
```

### `legacy import`

Imports the catalog (artists, releases, tracks, contributions), identity (users, roles,
portable bcrypt credentials), user state (playlists and ordering, shares, bookmarks,
starred/rating state, play history), and operational state (podcast channels, radio stations).
Original release dates are preserved separately from edition dates. Media assets are adopted in
place under the mapped canonical-relative paths; legacy file hashes are adopted only when
already valid, otherwise recorded as a hash-pending sentinel for optional bounded backfill —
the library is never re-hashed up front.

```bash
melodee-cli legacy import \
  --source-db postgres://user:pass@host/legacy_snapshot \
  --library-path-map /music=library \
  --canonical-root /srv/melodee/canonical
```

- `--run <uuid>` resumes an existing run after interruption.
- `--batch-size <n>` bounds rows per target transaction (default 500).
- `--canonical-root <path>` mounts the canonical storage root for **read-only** validation of
  each adopted file: containment, symlink policy, existence, and expected size. Missing or
  mismatched files are recorded as warning issues; a path escaping the root through a symlink
  is an error and that track is skipped. Without this flag, adoption is metadata-only and the
  run records one informational `media.verification_skipped` issue.

Users whose legacy password hash is not portable bcrypt are imported with an unguessable
placeholder credential and a `credential.unsupported` issue; they must complete a password
reset before cutover. Legacy role flags are preserved as the native `Has*Role` vocabulary;
administrators also retain the effective Editor capability. Legacy settings are recorded as
`settings.reference_only` informational issues because the Rust runtime owns its configuration.

Share bearers that contain at least 128 bits of verified unpredictable material retain their
existing URLs. Weaker legacy bearers are rotated during import, the old URLs stop working, and a
warning issue identifies the affected share so its owner can distribute the replacement URL.

Legacy audio bitrate and BPM are preserved in the existing Rust media/audio-feature fields. The
importer recognizes the frozen legacy bitrate units (ordinary small values are historical kbps;
already-large values are treated as bits per second). Legacy genres, moods, bit depth, VBR flags,
free-form track metadata, and aggregate play counters currently have no authoritative Rust
representation. Non-empty or present values are retained as `metadata.reference_only` warning
issues with field names, rather than being copied into unrelated catalog fields or converted into
synthetic play events. Review those issues before cutover; final 1.0 disposition is tracked by
proposed ADR-0150.

### `legacy verify`

Compares source and target for a run: per-domain counts, exact unmatched samples, public-ID
equality, relationship totals (primary artist links, playable media links, playlist items),
playlist ordering, orphan mappings, duplicate public IDs, and — with `--canonical-root` —
adopted-media existence and size. A clean run transitions to `delta-ready`; any mismatch is
recorded as a durable issue and the run stays in `verifying` for resolution.

```bash
melodee-cli legacy verify --run <uuid> \
  --source-db postgres://user:pass@host/legacy_snapshot \
  --canonical-root /srv/melodee/canonical
```

### `legacy delta`

Applies an incremental delta for a verified run: imports new source rows, applies authorized
display-field updates for rows changed since import, and reports source deletions as
reconciliation issues — never destructive target changes. A delta over an unchanged source is
a convergent no-op.

```bash
melodee-cli legacy delta --run <uuid> \
  --source-db postgres://user:pass@host/legacy_snapshot
```

### `legacy status`

Shows run state, per-domain checkpoints (progress, inserted/updated/issue totals, elapsed
time), and ownership. Omit `--run` to list recent runs.

```bash
melodee-cli legacy status --run <uuid>
```

## Inspect and resolve issues

Every anomaly the tooling encounters — unrepresentable rows, conflicts, deletions, media
mismatches — is a row in `legacy_migration.issues` keyed to the run. Query them directly:

```sql
SELECT domain, severity, code, message, detail
FROM legacy_migration.issues
WHERE run_id = '<uuid>'
ORDER BY created_at;
```

Resolve the underlying cause (fix the source snapshot, adjust the root mapping, mount the
canonical root), then resume the same run: already-imported rows are skipped by the durable
mapping, and resolved rows import normally.

## What the migration never does

- Never writes to the source database or source media.
- Never moves, re-encodes, re-hashes, fingerprints, or deletes canonical media.
- Never deletes target rows to reconcile source deletions (deletions are reported issues).
- Never migrates retired Party/Jukebox behavior as an enabled feature, and never copies the
  legacy search index as authority.

## See also

- [Database setup and backup](./database/)
- [Storage roles: inbound, staging, work, canonical](./storage/)
- [Upgrading a Rust deployment](./upgrading/)
