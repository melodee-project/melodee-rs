# Database Migrations

SQLx/PostgreSQL migrations live in `crates/melodee-persistence/migrations/` (numbered `0001_…`
onward) and are applied by the server's migration runner and `melodee-cli migrate`. This
top-level directory is retained as the conventional discovery location; do not add migration
files here.

Every schema change requires its own migration plus a rollback/forward plan and data-integrity
tests, and durable schema decisions require an accepted schema ADR (see `AGENTS.md`).
