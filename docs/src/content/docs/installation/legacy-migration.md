---
title: Legacy C# migration tooling
description: Inventory, plan, baseline, import, verify, delta, freeze, validate, and status commands for migrating a frozen C# Melodee database to the Rust server.
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
   lets the tooling read the PostgreSQL system identifier. If unavailable, the tooling uses
   connection facts only as input to a redacted source-identity digest; it never emits the
   host, port, database name, or connection URL.

4. Initialize the Rust target database with the normal schema migrations
   (`melodee-cli migrate`) before planning.

### Rehearse an existing snapshot

The repository rehearsal harness can consume an already-created, isolated snapshot. This mode
does not seed, write, or drop the source database, and it requires an explicit library mapping
and an existing canonical root before it creates the disposable Rust target databases:

```bash
python3 scripts/legacy_rehearsal.py drill \
  --source-db "$SNAPSHOT_URL" \
  --library-path-map /srv/music=library \
  --canonical-root "$CANONICAL_ROOT" \
  --source-backup "$LEGACY_BACKUP" \
  --output target/verification/migration/rehearsal-drill-existing-snapshot.json
```

The harness records source counts, migration/search/backup/restore/freeze measurements, and
checks that the source and canonical root were not changed by the rehearsal. It leaves the run
in `delta-ready` unless `--acknowledge-cutover` is supplied with the operator's externally
performed freeze acknowledgement. Metadata-only media observation is the default so a large
library is not read twice; add `--hash-media` only when the operator has budgeted a complete
byte-level before/after hash. `--source-backup` is required in existing-snapshot mode; the file
must be a regular, non-symlink custom-format PostgreSQL dump and is validated with
`pg_restore --list` before any Rust target database is created. This remains rehearsal evidence:
it does not authorize production traffic switching or a release cutover.

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

The plan records the source snapshot marker. Capture the immutable baseline from that same
snapshot before importing; the command fails closed if the marker has changed. The JSON artifact
has schema version `1`, records only non-sensitive source identity/read-only facts, hashes
non-secret setting values with SHA-256, and records secret setting keys only with `redacted: true`.
It never prints the source connection URL or setting values:

```bash
melodee-cli legacy baseline \
  --run <uuid> \
  --source-db postgres://user:pass@host/legacy_snapshot \
  --json > legacy-baseline.json
```

Keep `legacy-baseline.json` with the rehearsal evidence. Do not commit a production URL, password,
private host name, canonical root, or raw configuration export.

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
  each adopted file: containment, no-follow path safety, existence, regular-file type, and
  expected size. Missing or mismatched files are recorded as warning issues; a path escaping the
  root, traversing a symlink, or resolving to a special file is an error and that track is
  skipped. Without this flag, adoption is metadata-only and the run records one informational
  `media.verification_skipped` issue.

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
synthetic play events. Review those issues before cutover; the final 1.0 disposition is governed
by ADR-0150.

#### Account profiles and User Images avatars

The users importer preserves the C# `Description`, `TimeZoneId`, `PreferredLanguage`, and
`PreferredTheme` values within their C# storage bounds. A malformed but bounded timezone is
retained with a `profile.time_zone_invalid` warning; an over-bound value is not truncated and
becomes an explicit `profile.*_unsupported` issue.

The avatar importer reads the one C# `Libraries` row with `Type = 4` (`UserImages`) from its
recorded `Path`. The operator must mount that directory read-only at the same path visible to the
migration process. Only a direct regular file named `<legacy numeric user id padded to eight
digits>.gif` is eligible. The importer validates ownership, file type, source size, image decoding,
and normalized output bounds, then stores normalized `image/png` bytes of at most 1 MiB in
`native_user_avatars`. It reports `avatar.library_*`, `avatar.unsupported_filename`,
`avatar.unsafe_file`, `avatar.unmapped_owner`, `avatar.source_size_unsupported`, and
`avatar.image_unsupported` issues as applicable. It never writes, renames, deletes, or uses a
legacy filesystem path as identity.

The avatar phase is checkpointed as `user_avatars` and compares normalized target bytes before
writing, so a resumed import does not create duplicate avatar data or audit events. Before cutover,
review every profile/avatar issue and verify representative target rows and PNG content. The
importer does not recreate per-user device-profile semantics; those remain the reference-only
disposition accepted by ADR-0148 and assigned to MEL-5.

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

### `legacy validate`

Validates adopted media for a run under an existing absolute canonical root. It is read-only by
default and emits stable JSON counts plus path digests, never private paths:

```bash
melodee-cli legacy validate --run <uuid> \
  --canonical-root /srv/melodee/canonical --json
```

If a regular file is present but target `byte_size` metadata is stale, review the deterministic
plan and apply only that derived metadata repair explicitly:

```bash
melodee-cli legacy validate --run <uuid> \
  --canonical-root /srv/melodee/canonical \
  --fix --confirm --actor migration-operator --json
```

Repair is allowed only before cutover on a `delta-ready` run. A no-follow descriptor observation
updates target `media_assets.byte_size` only when the planned path and old value still match. It
never writes media, changes paths/hashes/identities/relationships, deletes files, or modifies the
C# source. Missing, unsafe, changed, or inaccessible files remain durable issues. Run the
read-only command first; `--fix` always requires `--confirm`.

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

### `legacy freeze`

Runs the final delta and verification pass for a `delta-ready` run. It records the source marker
and freeze duration but leaves the run in `delta-ready` unless the operator explicitly supplies
`--acknowledge-cutover --note "..."`. A changing source marker aborts the pass; it never guesses
which writes should be included.

```bash
melodee-cli legacy freeze --run <uuid> \
  --source-db postgres://user:pass@host/legacy_snapshot \
  --library-path-map /music=library \
  --canonical-root /srv/melodee/canonical
```

The acknowledgement is an operator decision, not a scripted default. Keep the recorded note with
the rehearsal report and perform traffic switching only through the approved deployment runbook.

`freeze` does not freeze the C# server itself. The operator must obtain and record the external
write-freeze approval before supplying `--acknowledge-cutover`. The marker check proves that the
approved source snapshot remained still during the final delta; it does not authorize traffic or
release publication.

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
- Never repairs target media metadata without an explicit, reviewed `legacy validate --fix
  --confirm` plan.
- Never migrates retired Party/Jukebox behavior as an enabled feature, and never copies the
  legacy search index as authority.

## See also

- [Database setup and backup](./database/)
- [Container deployment and release artifacts](./docker/)
- [Storage roles: inbound, staging, work, canonical](./storage/)
- [Upgrading a Rust deployment](./upgrading/)
- [Operational troubleshooting](../troubleshooting/)

## Release-acceptance boundary

The migration commands and synthetic rehearsal are safe preparation, not production approval.
Before using a private snapshot or media root, obtain a named operator and approver, record the
exact source identity and read-only proof, and use the [cutover and rollback runbook](https://github.com/melodee-project/melodee-rs/blob/main/design/runbooks/CUTOVER-AND-ROLLBACK.md).
Do not freeze writes, switch traffic, publish an image, or delete rollback assets without the
corresponding approval and audit record. Historical rehearsal results do not prove a current
release candidate.
