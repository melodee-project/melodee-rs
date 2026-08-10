# Local Development

Phase 5 is the current implementation boundary. Phases 0–4 are complete: the server implements
durable inbound processing through immutable manifest v3 plus the bounded PostgreSQL catalog,
identity, contribution, and operator-diagnostics boundary. The authoritative Phase 4 decision is in
`design/evidence/phase-4/PHASE-4-REPORT.md`. Classification, novelty, and policy are not yet
implemented; compatibility APIs, playback, and authentication remain later-phase work.

## Start PostgreSQL and the server

```bash
cp .env.example .env
docker compose -f compose.dev.yaml up -d postgres
mkdir -p /tmp/melodee-work /tmp/melodee-storage
set -a
source .env
set +a
./scripts/bootstrap.sh
cargo run -p melodee-server
```

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

## Check the running process

```bash
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
cargo run -p melodee-cli -- doctor
cargo run -p melodee-cli -- ingest status
cargo run -p melodee-cli -- catalog status
```

`/healthz` is liveness only. `/readyz` performs a bounded PostgreSQL query and returns HTTP `503`
with `status: "not_ready"` when the database is unavailable. It does not claim that media tools,
storage roots, free space, or background workers are ready. `/api/admin/v1/system/info` returns the
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
cargo run -p melodee-cli -- work show <work-item-uuid>
```

These commands use the same exported hierarchical configuration as the server. They are operator
tools, not authenticated remote APIs. Local Phase 0 compatibility captures remain beneath
`compatibility/captures/` and are ignored by Git.
