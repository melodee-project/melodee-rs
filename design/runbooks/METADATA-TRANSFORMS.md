# Metadata Transforms Runbook

This is the operator procedure for previewing, applying, and reverting canonical
metadata transformations (title, artist, album, genre, year, cover art) through the
Rust admin metadata service. It validates the implementation against a real
PostgreSQL database and canonical media root, as exercised by the
`admin_metadata_api` integration tests (MEL-8).

The canonical media root and its fixtures are read-only during preview. Apply
mutates the canonical file atomically (write-to-temp then rename) and records an
audit row. Revert restores the exact pre-apply bytes and hash. No command here
deletes a source, target, backup, snapshot, or rollback asset.

References: [ADR-0161](../adr/ADR-0161-meloamp-listener-facing-web-workflows.md),
`apps/melodee-server/src/admin_metadata.rs`, and the
[admin metadata API docs](../../docs/src/content/docs/admin-api/metadata.md).

## Roles and approval gates

- **Metadata operator** — runs preview, apply, revert, verification, and diagnostics.
- **Data owner** — confirms the target release/track, path mappings, and expected hashes.
- **Approver** — authorizes the apply against a canonical root and any revert.

Preview is read-only and requires no approval. Apply and revert against a canonical
root require a separate approval.

## Variables and safety rules

Set these only in the protected operator environment, never in committed evidence or
shell history:

```text
RUST_DATABASE_URL=<approved Rust target URL>
CANONICAL_ROOT=<approved absolute mounted media root>
RUN_ID=<metadata run UUID>
RUST_TARGET=<exact Rust service/container identifier>
BACKUP_DIR=<approved backup directory>
METADATA_OPERATOR=<stable operator identity>
```

Before every step, verify the exact target, expected impact, approval record, and
topology. Stop if the target, database identity, canonical root, or backup differs
from the change record. Redact URLs, credentials, private paths, hashes, tokens, and
session keys from saved output.

## 1. Preflight and baseline

1. Confirm the Rust service is reachable and the admin metadata routes are registered.
2. Record the baseline SHA-256 of each canonical media asset that will be touched.
3. Record the baseline database row (release/track title, artist, genre, year, hash,
   revision) for each affected entity.
4. Verify the operator has an admin session token with the `HasStreamRole` (or
   equivalent admin) claim; non-admin calls must be rejected with `403`.

## 2. Preview (read-only)

1. Call `POST /api/admin/v1/metadata/preview` with the desired transform.
2. Verify the response reports the intended field changes and the new expected
   SHA-256 without modifying any canonical file or database row.
3. Confirm the baseline file hash and database row are unchanged.

## 3. Apply

1. With approval, call `POST /api/admin/v1/metadata/apply` with the same transform.
2. Verify the canonical file is replaced atomically and its SHA-256 matches the
   previewed expected hash.
3. Verify the database row reflects the new title/artist/genre/year/hash and the
   audit row records the operator, timestamp, before/after hashes, and transform.
4. Confirm search/catalog consistency: the search index (Tantivy generation) and
   catalog rows resolve the public ID to the same canonical entity.

## 4. Revert (exact)

1. Call `POST /api/admin/v1/metadata/revert` for the affected asset.
2. Verify the canonical file bytes and SHA-256 match the pre-apply baseline exactly.
3. Verify the database row and hash return to the pre-apply values and a revert
   audit row is recorded.
4. Confirm no decoded-audio drift: the file remains decodable and its audio
   fingerprint (where applicable) is unchanged.

## 5. Failure-injection validation

1. Simulate a missing or failing `ffmpeg` at apply time.
2. Verify the apply does not corrupt the canonical file and leaves the database
   row unchanged (no partial apply).
3. Verify the audit row records the failure and the operation is recoverable.
4. Re-run a successful apply to confirm the path converges.

## 6. Restart and consistency

1. Restart the Rust service before and after a commit.
2. Verify the service re-reads the canonical root and database without data loss.
3. Verify the public ID, canonical path, and stable hash are preserved across
   restart.

## 7. Completion record

Record in the change record:
- Run ID, operator, approver, and timestamps.
- Before/after hashes for each affected canonical asset.
- Before/after database rows and audit rows.
- Result of each acceptance criterion (preview, apply, revert, failure injection,
  restart, catalog consistency).

All evidence (test logs, change record, and this runbook) is retained in the
repository under `design/evidence/` and the committed integration tests in
`apps/melodee-server/tests/admin_metadata_api.rs`.
