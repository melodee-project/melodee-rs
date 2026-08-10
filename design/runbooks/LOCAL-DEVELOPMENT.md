# Local Development

The Rust rewrite is currently in Phase 3. The server implements durable inbound reconciliation,
media inspection, and normalization; verification and canonical publication are the active Phase
3.3 boundary. It does not implement the Phase 4 catalog, compatibility APIs, playback, or
authentication.

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
trusted network controls: the current Phase 3 server has no application authentication layer.

## Check the running process

```bash
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
cargo run -p melodee-cli -- doctor
cargo run -p melodee-cli -- ingest status
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

Useful one-shot development commands are:

```bash
cargo run -p melodee-cli -- ingest reconcile
cargo run -p melodee-cli -- ingest inspect
cargo run -p melodee-cli -- ingest normalize
cargo run -p melodee-cli -- ingest validate
cargo run -p melodee-cli -- ingest publish
cargo run -p melodee-cli -- ingest manifest <work-item-uuid>
cargo run -p melodee-cli -- ingest publication <work-item-uuid>
cargo run -p melodee-cli -- work show <work-item-uuid>
```

These commands use the same exported hierarchical configuration as the server. They are operator
tools, not authenticated remote APIs. Local Phase 0 compatibility captures remain beneath
`compatibility/captures/` and are ignored by Git.
