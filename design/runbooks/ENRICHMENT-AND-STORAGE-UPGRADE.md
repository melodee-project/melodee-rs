# Runbook — provider eligibility, manifest evidence and outbox retention upgrade

Operator procedure for the change set described by ADR-0212, ADR-0213 and
ADR-0214. Implementation and rehearsal are local; production execution,
destructive maintenance and publication require separate operator direction.

## What changes

| Area | Change | Reversible by |
| --- | --- | --- |
| Provider scheduling | Only enabled, credentialed providers receive work; enablement backfills, disablement suspends | Starting the previous image against the previous schema (provider tables are additive) |
| Manifest evidence | Full manifests move to `MELODEE__STORAGE__ASSET_ROOT`; PostgreSQL keeps a reference and query facts | Restoring the pre-upgrade database **and** asset-store pair |
| Search outbox | Successful detail expires after 7 days (2–90); exact hourly totals kept 400 days | Starting the previous image; detail older than the window is gone by design |

Migrations: `0057_provider_eligibility_and_backfills.sql`,
`0058_search_outbox_retention.sql`, `0059_manifest_evidence_files.sql`.
Existing migration checksums are untouched.

## 0. Prerequisites

- The candidate binary (`melodee`/`melodee-cli`) of the release under test.
- `MELODEE__DATABASE__URL` and `MELODEE__STORAGE__ASSET_ROOT` for every server,
  worker and importing CLI. The asset root must be an existing absolute
  directory on persistent storage.
- PostgreSQL superuser (or table owner) for the migration connection.
- Free space:
  - asset filesystem: at least the total size of all serialized manifests
    (roughly the uncompressed `manifest_payload` bytes; JSONB TOAST may have
    compressed the old copy, so budget 1.0–1.5× the current manifest table +
    TOAST size), plus the existing store;
  - PostgreSQL: WAL and headroom for the small fact/receipt tables and for
    releasing the old TOAST (typically several times the removed payload bytes
    of WAL), plus ordinary vacuum/autovacuum headroom.
- A maintenance window: the migration is not online. Stop **all** writers,
  including the old server, every worker and any importer.

Record the baseline before changing anything:

```bash
git rev-parse HEAD                      # record this commit
date -u +%Y-%m-%dT%H:%M:%SZ
du -sb "$MELODEE__STORAGE__ASSET_ROOT"
psql "$MELODEE__DATABASE__URL" -c "select pg_database_size(current_database());"
```

Stop old writers and verify none remain:

```bash
make down                               # or: docker compose stop melodee
pgrep -af 'melodee|melodee-cli' || true # must print nothing
```

## 1. Baseline and backup pair

```bash
# Database snapshot (custom format) and validation
pg_dump "$MELODEE__DATABASE__URL" --format=custom --file melodee-pre-upgrade.dump
pg_restore --list melodee-pre-upgrade.dump >/dev/null && echo "dump verified"

# Asset store snapshot (files + marker), same restore point
tar -C "$MELODEE__STORAGE__ASSET_ROOT" -czf melodee-assets-pre-upgrade.tgz .
```

Test-restore both into a disposable database and directory, then confirm a known
cover/avatar and a manifest row exist. Only a restore-tested pair is a rollback
asset.

Expected: `pg_restore --list` exits 0; the restore check prints at least one row
from `release_work_item_manifests` and one readable asset file.

## 2. Preflight (read-only)

```bash
melodee-cli migrate --preflight
```

Expected JSON includes:

```json
{
  "databaseReachable": true,
  "migrationsCurrent": false,
  "assetStoreBound": true,
  "manifestInlineRows": 4424,
  "manifestEvidenceRows": 0,
  "storageHeadroomBytes": 1234567890,
  "providerEligibilityResolved": true,
  "blockingConditions": ["migrations are not current (applied 56, available 59)"]
}
```

Failure signals: `assetStoreBound: false` (no store identity yet — the first
write binds it), a `storageHeadroomBytes` smaller than the estimate in step 0,
or any wording that the store root differs from the registered store. Do not
continue on a binding mismatch.

## 3. Upgrade

```bash
melodee-cli migrate
```

Sequence (no flag resumes the normal upgrade):

1. embedded migrations apply and checksums are verified;
2. inline manifests are backfilled in stable ID order, 50 per checkpoint, one
   large manifest at a time, each file read back and compared with its source;
3. an independent verification pass rereads every evidence file, checks
   SHA-256/length, compares reconstructed JSON with any remaining inline
   payload and compares recomputed facts with the stored facts;
4. inline payloads are cleared per row, fenced by the manifest hash;
5. the gate confirms every manifest has a verified reference and receipt.

Expected output: the JSON status document with `pending: 0`,
`inlineRemaining: 0`, `verificationErrors: 0`, `evidenceRows` equal to the
number of manifests and `filesystemBytes` close to the evidence byte total.

Progress and integrity checks (read-only, safe to repeat):

```bash
melodee-cli migrate --status
melodee-cli migrate --verify     # nonzero exit on any discrepancy
```

Failure signals and recovery:

| Signal | Meaning | Recovery |
| --- | --- | --- |
| `asset store binding is missing` / store root mismatch | The configured root is not the store this database was bound to | Point every service at the correct `MELODEE__STORAGE__ASSET_ROOT`; retry |
| `evidence could not be read` / digest mismatch | A published object is missing or corrupt | Keep the database (inline source is intact), restore the asset store from the pair, retry |
| `no space left on device` | Asset filesystem or PostgreSQL out of space | Free space, retry; progress resumes from the last committed row |
| `reconstructed JSON differs from its inline payload` | Concurrent edit or corrupt object | Stop, keep both sources, investigate; do not clear inline data |
| `manifest evidence upgrade is incomplete` | A bare server start found missing receipts | Run `melodee-cli migrate` to completion |
| verification nonzero exit | Any discrepancy after finalization | Restore the backup pair; do not delete evidence files |

An interrupted run is safe: re-running `melodee-cli migrate` resumes. It never
removes inline evidence before verification.

## 4. Start the candidate and reconcile the inherited queue

```bash
# docker compose up -d --no-deps melodee
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
melodee-cli doctor --deep
```

The first enrichment pass reconciles eligibility and the installed backlog:
never-attempted work for ineligible providers is removed, historically attempted
work becomes `dormant`, and eligible providers receive a revisioned backfill.
Watch:

```bash
docker compose logs -f melodee | grep -E 'provider eligibility changed|backfill'
```

Expected: a `provider eligibility changed` line with a revision, then
`backfill_pages`/`backfill_tasks` progress in the worker pass log. Enabling a
provider later repeats the same path automatically.

Operator rollback of the queue is possible by enabling/disabling providers in
settings; no data deletion is needed.

## 5. Verify search and statistics

```bash
melodee-cli search status
melodee-cli search prune-outbox --dry-run
```

Compare the 24-hour "Tracks indexed" series against the pre-upgrade chart (or a
pre-upgrade export). The finalized hours are identical; only hours that had not
closed at capture can differ.

Then run the cleanup preview, apply it, and run it again:

```bash
melodee-cli search prune-outbox --apply
melodee-cli search prune-outbox --apply   # second pass reports rowsPruned: 0 when settled
```

Expected: only rows older than the retention window are deleted; `protected`
and `failed` stay; `blockingRebuild` is null; the second pass deletes nothing.

## 6. Check playback and compatibility surfaces

- Native: sign in, open a Release, request a Track, request lyrics.
- OpenSubsonic: `getSong` and `getAlbumList` for a known Release (bit depth comes
  from the indexed fact, not from a manifest read).
- Player: one range stream from a known Track.

## 7. Forward recovery and rollback rehearsal

Forward recovery: interrupt `melodee-cli migrate` (SIGINT) during backfill, run
`--status`, then re-run `migrate`; it must resume and finish with
`--verify` clean.

Rollback to the previous binary:

1. stop the candidate;
2. restore the pre-upgrade database into an empty database instance;
3. restore the matching pre-upgrade asset-store directory;
4. start the previous image and verify migration history, `/readyz`, catalog
   counts and one playback.

Do not mix a new database with an old asset store or vice versa. The evidence
references and the store marker are bound together.

## 8. After acceptance

- Keep the pre-upgrade and post-upgrade digests, the verified backup pair and
  the `--verify` output for the acceptance window.
- Verify `VACUUM`/autovacuum has reclaimed the released TOAST before claiming
  recovered space. Reusable space inside the database is not space returned to
  the operating system; do not run `VACUUM FULL` or table rewrites as automatic
  maintenance.
- Provider quotas may limit catch-up after enabling a provider. The local
  measurement (1,980 jobs in 42 s with a 100 ms stub) is not a production
  provider quota statement.
