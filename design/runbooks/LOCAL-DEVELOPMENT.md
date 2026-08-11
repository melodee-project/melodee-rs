# Local Development

Phase 6 is the current implementation boundary. Phases 0–5 are complete: the server implements
durable inbound processing through immutable manifest v3, the bounded PostgreSQL catalog and
identity model, and explainable classification/novelty/policy evaluation with operator commands.
The authoritative Phase 5 decision is in `design/evidence/phase-5/PHASE-5-REPORT.md`. Phase 6 search
and MQL are implemented under Candidate review; compatibility APIs, playback, and authentication
remain later-phase work.

## Start PostgreSQL and the server

```bash
cp .env.example .env
docker compose -f compose.dev.yaml up -d postgres
mkdir -p /tmp/melodee-work /tmp/melodee-storage
mkdir -p /tmp/melodee-search
set -a
source .env
set +a
./scripts/bootstrap.sh
cargo run -p melodee-server
```

The development compose file allocates 512 MiB of PostgreSQL shared memory so parallel integration
test migrations do not exhaust Docker's 64 MiB default. If the dev database container predates
that setting, recreate only that container:

```bash
# Optional but recommended when this is a production-relevant database:
pg_dump --format=custom --file=/safe/backup/location/melodee-pre-compose-shm.dump "$MELODEE__DATABASE__URL"

# Safe recreation keeps the named volume (data) intact:
docker compose -f compose.dev.yaml up -d --force-recreate --no-deps postgres

# Verify the new /dev/shm size inside the running container:
docker exec "$(docker compose -f compose.dev.yaml ps -q postgres)" sh -c "df -h /dev/shm"
```

This is development-only and does not change production behavior.

`cargo` does not load `.env` itself; exporting the file as shown above is required. The example
PostgreSQL connection uses host port `5433`. The server connects and applies migrations before it
binds the listener. Invalid ingestion/work configuration or a missing database fails startup rather
than silently disabling required behavior.

The canonical listener key is `MELODEE__SERVER__BIND`; the old `MELODEE_BIND` bootstrap key remains
a compatibility fallback. Keep the loopback default unless the diagnostics routes are protected by
trusted network controls: the current pre-compatibility server has no application authentication
layer.

## Apply and recover the Phase 4 catalog migration

Migration `0007_phase4_catalog.sql` is additive. It creates catalog, identity, contribution, audit,
overlay, import-receipt, participation, and outbox relations and extends the durable work-state
constraints for cataloging. Applying it does not open, move, rewrite, or delete inbound or canonical
media. Before applying it to a database that matters, create and validate a database backup using an
explicit path outside the repository:

```bash
pg_dump --format=custom --file=/safe/backup/location/melodee-pre-0007.dump "$MELODEE__DATABASE__URL"
pg_restore --list /safe/backup/location/melodee-pre-0007.dump >/dev/null
cargo run -p melodee-cli -- migrate
psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command \
  "SELECT version, description, success FROM _sqlx_migrations WHERE version = 7"
psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command \
  "SELECT to_regclass('public.artists'), to_regclass('public.catalog_imports'), to_regclass('public.catalog_projection_outbox')"
cargo run -p melodee-cli -- catalog status
```

An operational binary downgrade leaves migration 0007 and all catalog, identity, negative
assertion, merge/split, audit, and import evidence in place. Stop catalog workers first, verify the
exact older binary against an isolated copy of the current post-0007 database, and only then change
the running binary. If that build cannot tolerate Phase 4 work states and relations, do not downgrade
it in place; use the verified full-restore path below. Do not approximate rollback by dropping Phase
4 tables: there is intentionally no destructive down migration.

A full schema rollback is only a verified restore of the pre-0007 database backup. Restore into a
new empty database, validate it, and repoint the service according to the site's database cutover
procedure; keep the current database intact until verification completes. A local restore rehearsal
can use a separately named empty database:

```bash
createdb melodee_phase4_restore_check
pg_restore --exit-on-error \
  --dbname=postgres://melodee:melodee-dev@127.0.0.1:5433/melodee_phase4_restore_check \
  /safe/backup/location/melodee-pre-0007.dump
psql postgres://melodee:melodee-dev@127.0.0.1:5433/melodee_phase4_restore_check \
  --set ON_ERROR_STOP=1 --command \
  "SELECT version, description, success FROM _sqlx_migrations ORDER BY version"
```

The restore target must be empty and disposable. This procedure restores PostgreSQL only; it does
not modify or remove media. Retain Phase 4 evidence by default unless the explicitly chosen,
verified full-backup restore is required.

## Apply and recover the Phase 5 policy migration

Migration `0008_phase5_policy.sql` is additive. It adds independently asserted catalog traits,
AudioIdentity no-match/alternate-recording facts, Release classification and evidence, Track
novelty, versioned rule-set, durable decision/audit, and policy-test relations. It does not open,
move, rewrite, or delete inbound or canonical media. Create and verify a PostgreSQL backup at an
explicit path outside the repository before applying it to a database that matters:

```bash
pg_dump --format=custom --file=/safe/backup/location/melodee-pre-0008.dump "$MELODEE__DATABASE__URL"
pg_restore --list /safe/backup/location/melodee-pre-0008.dump >/dev/null
cargo run -p melodee-cli -- migrate
psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command \
  "SELECT version, description, success FROM _sqlx_migrations WHERE version = 8"
psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command \
  "SELECT to_regclass('public.release_catalog_trait_assertions'), to_regclass('public.audio_identity_catalog_assertions'), to_regclass('public.audio_identity_alternate_recordings'), to_regclass('public.release_classifications'), to_regclass('public.track_novelty'), to_regclass('public.policy_rule_sets'), to_regclass('public.policy_decisions'), to_regclass('public.policy_decision_audit'), to_regclass('public.policy_test_runs')"
```

An operational binary downgrade retains migration 0008 and all catalog-trait, AudioIdentity,
classification, novelty, policy, test-run, and audit evidence. Stop the policy worker, verify the
exact older binary against an isolated copy of the post-0008 database, and only then change the
running binary. If it cannot tolerate the additive relations or Phase 5 work-item behavior, do not
downgrade in place. There is no destructive down migration.

A full schema rollback is only a verified restore of the pre-0008 database backup into a new empty
database. Keep the current database intact until the restored copy passes migration-history and
catalog integrity checks:

```bash
createdb melodee_phase5_restore_check
pg_restore --exit-on-error \
  --dbname=postgres://melodee:melodee-dev@127.0.0.1:5433/melodee_phase5_restore_check \
  /safe/backup/location/melodee-pre-0008.dump
psql postgres://melodee:melodee-dev@127.0.0.1:5433/melodee_phase5_restore_check \
  --set ON_ERROR_STOP=1 --command \
  "SELECT version, description, success FROM _sqlx_migrations ORDER BY version"
psql postgres://melodee:melodee-dev@127.0.0.1:5433/melodee_phase5_restore_check \
  --set ON_ERROR_STOP=1 --command \
  "SELECT count(*) AS releases FROM releases; SELECT count(*) AS catalog_imports FROM catalog_imports"
```

The restore target must be empty and disposable. Restoring PostgreSQL does not modify media bytes;
retain the post-0008 database and its audit evidence unless an explicitly verified full restore is
required.

## Apply and recover the Phase 6 search migration

Migration `0009_phase6_search.sql` additively extends the catalog projection outbox and adds search
checkpoints, generations, rebuild jobs, and the active-generation pointer. It does not read or alter
media. Back up and validate PostgreSQL before applying it to a database that matters:

```bash
pg_dump --format=custom --file=/safe/backup/location/melodee-pre-0009.dump "$MELODEE__DATABASE__URL"
pg_restore --list /safe/backup/location/melodee-pre-0009.dump >/dev/null
cargo run -p melodee-cli -- migrate
psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command \
  "SELECT version, description, success FROM _sqlx_migrations WHERE version = 9"
psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command \
  "SELECT to_regclass('public.search_projection_checkpoint'), to_regclass('public.search_generations'), to_regclass('public.search_rebuild_jobs'), to_regclass('public.search_active_generation')"
```

A binary downgrade leaves migration 0009 and its operational evidence intact. Stop search workers,
test the exact older binary against an isolated database copy, and do not drop search tables in
place. A full schema rollback is a drained restore of the verified pre-0009 backup into a new empty
database. Tantivy generations are disposable: after forward migration, use
`melodee search rebuild --actor <administrator-id>` rather than copying or editing generation files.

## Check the running process

```bash
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
cargo run -p melodee-cli -- doctor
cargo run -p melodee-cli -- ingest status
cargo run -p melodee-cli -- catalog status
cargo run -p melodee-cli -- policy test --limit 100 --actor <administrator-id>
cargo run -p melodee-cli -- search status
```

`/healthz` is liveness only. `/readyz` performs a bounded PostgreSQL query and, when search is
enabled, verifies that the active generation opens with the expected schema. It returns HTTP `503`
with `status: "not_ready"` when a configured dependency is unavailable. It does not claim that media
tools, free space, or every background worker are ready. `/api/admin/v1/system/info` returns the
same service/version liveness metadata as `/healthz`; the other `/api/admin/v1/ingest/*` routes are
read-only queue and lease diagnostics.

## Configure inbound media processing

Set either `MELODEE__INGEST__SOURCE_ROOT` or `MELODEE__INGEST__SOURCES_JSON` in `.env` to an existing
inbound directory, export the file again, and restart the server. With no configured source, the
server stays live and ready but does not start ingestion workers. Phase 3 treats inbound trees as
read-only: do not point temporary work configuration into an inbound tree.

The inspection and normalization concurrency settings are independent and explicitly bounded.
`MELODEE__NORMALIZATION__WORK_AREA` is temporary, non-authoritative storage and should have enough
space for attempt-isolated output. It is not a canonical media root. Canonical verification and
publication use independently bounded `MELODEE__VALIDATION__CONCURRENCY` and
`MELODEE__PUBLICATION__CONCURRENCY` settings. Publication is disabled unless
`MELODEE__STORAGE__ROOT` is explicitly configured; keep that root separate from every inbound and
normalization work directory. Phase 3 publication never moves or deletes the inbound source.

Catalog import is independently enabled and bounded by `MELODEE__CATALOG__ENABLED`,
`MELODEE__CATALOG__CONCURRENCY`, `MELODEE__CATALOG__BATCH_SIZE`, and
`MELODEE__CATALOG__POLL_INTERVAL_SECONDS`. It consumes the authoritative PostgreSQL manifest-v3,
plan, and receipt; it does not reopen inbound or canonical files. Disabling inbound discovery does
not disable cataloging of already-published work. Each claimed item renews its lease immediately
before its bounded catalog transaction. Graceful shutdown releases claimed items that have not
started that transaction and waits for an already-started atomic transaction to finish.

Policy evaluation is independently enabled with `MELODEE__POLICY__WORKER__ENABLED`; its polling
interval, batch, and concurrency bounds are configured with
`MELODEE__POLICY__WORKER__POLL_INTERVAL_SECONDS`, `MELODEE__POLICY__BATCH_SIZE`, and
`MELODEE__POLICY__CONCURRENCY`. Invalid values fail server startup. The worker consumes only
authoritative PostgreSQL Release, classification, AudioIdentity, byte-hash, catalog-context, and
sealed-manifest fingerprint facts. It does not reread media. A graceful stop releases work that has
not begun its fenced transaction and lets an already-started atomic evaluation finish.

Greatest Hits behavior is restricted to safe configured choices:
`MELODEE__POLICY__REDUNDANT_GREATEST_HITS_DISPOSITION` accepts `ignore` or `delete`,
`MELODEE__POLICY__NOVEL_GREATEST_HITS_DISPOSITION` accepts `review` or `accept`, and
`MELODEE__POLICY__UNCERTAIN_GREATEST_HITS_DISPOSITION` accepts `review` or `quarantine`. Any other
value fails startup. A `delete` preference records a pending, inspectable decision and routes the
Release for review; Phase 5 does not remove media or report destructive work as completed.
Rule sets are immutable by name and version. When changing any policy preference, increment
`MELODEE__POLICY__RULE_SET_VERSION`; attempting to start with different rules at an already-stored
version fails closed instead of silently changing the meaning of historical decisions.

Search is independently enabled with `MELODEE__SEARCH__ENABLED=true` and requires an explicit,
absolute `MELODEE__SEARCH__ROOT`. The root must not overlap inbound, work, or canonical storage.
The worker consumes the transactional catalog outbox in bounded batches, builds generations from
bounded PostgreSQL pages, and never opens media. A graceful stop releases a rebuild lease at a safe
page boundary; an in-flight Tantivy commit completes before PostgreSQL acknowledgement.

Useful one-shot development commands are:

```bash
cargo run -p melodee-cli -- ingest reconcile
cargo run -p melodee-cli -- ingest inspect
cargo run -p melodee-cli -- ingest normalize
cargo run -p melodee-cli -- ingest validate
cargo run -p melodee-cli -- ingest publish
cargo run -p melodee-cli -- ingest manifest <work-item-uuid>
cargo run -p melodee-cli -- ingest publication <work-item-uuid>
cargo run -p melodee-cli -- catalog import
cargo run -p melodee-cli -- catalog status
cargo run -p melodee-cli -- catalog inspect <work-item-uuid>
cargo run -p melodee-cli -- policy test --limit 100 --actor <administrator-id>
cargo run -p melodee-cli -- policy evaluate <release-uuid>
cargo run -p melodee-cli -- policy inspect <release-uuid>
cargo run -p melodee-cli -- policy decide <release-uuid> review --actor <administrator> --reason <reason>
cargo run -p melodee-cli -- search status
cargo run -p melodee-cli -- search rebuild --actor <administrator-id>
cargo run -p melodee-cli -- search query 'artist:"Colin Hay" AND year>=2000' --limit 50
cargo run -p melodee-cli -- search explain 'performer:"Guest Artist"'
cargo run -p melodee-cli -- work show <work-item-uuid>
```

`policy test` rejects missing/blank administrator identity and limits outside `1..=10000`; a
successful test run stores that exact actor with its aggregate audit summary. The default server
integration tests enforce the Phase 5 load gate; run it directly and retain its JSON metrics with:

```bash
cargo test -p melodee-server --test policy_worker \
  two_hundred_releases_converge_across_bounded_worker_passes \
  -- --nocapture
```

These commands use the same exported hierarchical configuration as the server. They are operator
tools, not authenticated remote APIs. Local Phase 0 compatibility captures remain beneath
`compatibility/captures/` and are ignored by Git.
