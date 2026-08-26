# Local Development

The server, catalog and processing pipeline, client adapters (native Melodee API v1,
OpenSubsonic, Jellyfin), user-domain parity, and the admin SPA are implemented. The
authoritative classification and policy decision record is
[ADR-0120](../adr/ADR-0120-phase5-classification-novelty-and-policy-schema.md); the Phase 5
closure report it accompanied is a withdrawn evidence report. Legacy import, production
cutover, and rollback rehearsal are the remaining release-readiness work.

## Start PostgreSQL and the server

```bash
cp .env.example .env
make bootstrap
docker compose -f compose.dev.yaml up -d postgres
mkdir -p /tmp/melodee-inbound /tmp/melodee-staging /tmp/melodee-work /tmp/melodee-storage
mkdir -p /tmp/melodee-search
set -a
source .env
set +a
cargo run -p melodee-server
```

Bootstrap is database-free and runs only Rust library and binary unit-test targets. PostgreSQL is
required for the server and the full integration-test gate, not for checkout bootstrap.

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

## Apply and recover the catalog and identity catalog migration

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
the running binary. If that build cannot tolerate catalog and identity work states and relations, do not downgrade
it in place; use the verified full-restore path below. Do not approximate rollback by dropping catalog and identity tables: there is intentionally no destructive down migration.

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
not modify or remove media. Retain catalog and identity evidence by default unless the explicitly chosen,
verified full-backup restore is required.

## Apply and recover the classification and policy policy migration

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
running binary. If it cannot tolerate the additive relations or classification and policy work-item behavior, do not
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

## Apply and recover the search and MQL search migration

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

## Apply and recover the native API compatibility native API migrations

Migration `0010_phase7_native_auth.sql` is additive and creates the native User/public-ID,
role/library-access, refresh-family, password-reset, social-login, and cluster-wide rate-limit
authority. Back up before applying it, retain the old database for downgrade, and never drop the
tables in place:

    pg_dump --format=custom --file=/safe/backup/location/melodee-pre-0010.dump "$MELODEE__DATABASE__URL"
    pg_restore --list /safe/backup/location/melodee-pre-0010.dump >/dev/null
    cargo run -p melodee-cli -- migrate
    psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command "SELECT to_regclass('public.native_users'), to_regclass('public.native_refresh_tokens'), to_regclass('public.native_password_reset_tokens'), to_regclass('public.native_rate_limit_counters')"

Native startup requires `MELODEE__NATIVE__AUTH__JWT_KEY` with at least 32 random bytes. The value in
`.env.example` is an intentionally rejected placeholder; generate a local secret with
`openssl rand -base64 48` and keep the same value on every process. Configure
`MELODEE__NATIVE__TRUSTED_PROXIES` only for controlled infrastructure; Melodee strips trusted hops
from the right of the forwarding chain and rejects an all-trusted or malformed chain. List each
browser client origin explicitly in
`MELODEE__NATIVE__CORS__ALLOWED_ORIGINS`. Google authentication remains disabled unless its enabled,
client-ID, HTTPS JWKS, and optional hosted-domain settings validate. Never enable development reset
token responses on a production system.

Migrations `0010` through `0019` are additive. Together they create native auth/public IDs,
per-user catalog state, community and podcast state, genre preferences, discovery, playlists,
endpoint sessions, catalog browse indexes, and bounded route-state constraints. They do not open,
move, or delete catalog media. When separately enabled, the podcast worker may retain or remove only
downloaded enclosures under its marker-owned, non-overlapping podcast root. Back up and validate
PostgreSQL before applying the set to a database that matters:

    pg_dump --format=custom --file=/safe/backup/location/melodee-pre-0010.dump "$MELODEE__DATABASE__URL"
    pg_restore --list /safe/backup/location/melodee-pre-0010.dump >/dev/null
    cargo run -p melodee-cli -- migrate
    psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command "SELECT version, description, success FROM _sqlx_migrations WHERE version BETWEEN 10 AND 19 ORDER BY version"
    psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command "SELECT to_regclass('public.native_users'), to_regclass('public.native_refresh_tokens'), to_regclass('public.native_endpoint_sessions'), to_regclass('public.native_playlists'), to_regclass('public.native_podcast_episodes')"

An older binary may leave these additive relations unused; verify that exact binary against an
isolated copy before downgrade. Do not drop native API compatibility tables in place. Full schema rollback restores
the verified pre-0010 backup into a new empty database and repoints the service only after migration
history and catalog checks pass. A disposable restore rehearsal is:

    createdb melodee_phase7_restore_check
    pg_restore --exit-on-error --dbname=melodee_phase7_restore_check /safe/backup/location/melodee-pre-0010.dump
    psql postgresql:///melodee_phase7_restore_check --set ON_ERROR_STOP=1 --command "SELECT version, success FROM _sqlx_migrations ORDER BY version DESC LIMIT 1"
    psql postgresql:///melodee_phase7_restore_check --set ON_ERROR_STOP=1 --command "SELECT count(*) AS tracks FROM tracks"
    dropdb melodee_phase7_restore_check

Run the rehearsal only against the explicitly named disposable database and retain the post-`0019`
database intact until cutover is verified.

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
server stays live and ready but does not start ingestion workers. Inbound media bytes are immutable
until the verified durable staging handoff, after which the acquired candidate directory is removed
(ADR-0145): keep inbound writable by the runtime UID, and do not point temporary work configuration
into an inbound tree.

The inspection and normalization concurrency settings are independent and explicitly bounded.
`MELODEE__NORMALIZATION__WORK_AREA` is temporary, non-authoritative storage and should have enough
space for attempt-isolated output. It is not a canonical media root. Canonical verification and
publication use independently bounded `MELODEE__VALIDATION__CONCURRENCY` and
`MELODEE__PUBLICATION__CONCURRENCY` settings. Publication is disabled unless
`MELODEE__STORAGE__ROOT` is explicitly configured; keep that root separate from every inbound and
normalization work directory. normalization and publication publication never moves or deletes the inbound source.

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
Release for review; classification and policy does not remove media or report destructive work as completed.
Rule sets are immutable by name and version. When changing any policy preference, increment
`MELODEE__POLICY__RULE_SET_VERSION`; attempting to start with different rules at an already-stored
version fails closed instead of silently changing the meaning of historical decisions.

Search is independently enabled with `MELODEE__SEARCH__ENABLED=true` and requires an explicit,
absolute `MELODEE__SEARCH__ROOT`. The root must not overlap inbound, work, or canonical storage.
The worker consumes the transactional catalog outbox in bounded batches, builds generations from
bounded PostgreSQL pages, and never opens media. A graceful stop releases a rebuild lease at a safe
page boundary; an in-flight Tantivy commit completes before PostgreSQL acknowledgement. Retryable
outbox failures stop retrying after `MELODEE__SEARCH__MAX_OUTBOX_ATTEMPTS` (default `5`, maximum
`100`) and remain visible as permanent failure evidence without blocking healthy sibling rows.

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
integration tests enforce the classification and policy load gate; run it directly and retain its JSON metrics with:

```bash
cargo test -p melodee-server --test policy_worker \
  two_hundred_releases_converge_across_bounded_worker_passes \
  -- --nocapture
```

These commands use the same exported hierarchical configuration as the server. They are operator
tools, not authenticated remote APIs. Local baseline capture compatibility captures remain beneath
`compatibility/captures/` and are ignored by Git.
