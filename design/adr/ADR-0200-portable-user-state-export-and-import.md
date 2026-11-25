# ADR-0200: Portable user-state export and import

- **Date:** 2026-11-15
- **Status:** Accepted
- **Decision owners:** Migration tooling, Persistence, CLI
- **Supersedes:** None
- **Superseded by:** ADR-0201 (decision 5 catalog-gap default), ADR-0203 (explicitly approved public-ID links)
- **Related requirements:** PRD-MIG-002, PRD-MIG-003, PRD-USER-001, PRD-PLAYLIST-001

Decision 5's catalog-gap failure default is superseded by
[ADR-0201](ADR-0201-import-available-user-state-as-the-catalog-grows.md). The original decision
below records the prior behavior. [ADR-0203](ADR-0203-reconcile-listener-preferences-with-a-rebuilt-catalog.md)
also permits an operator to link two public IDs explicitly for a rebuilt installation. The merge
rules remain in force.

## Context

The governed migration path (`melodee-cli legacy import`, ADR-0148) reads the C# database directly
and imports catalog, media, and user state in one fenced run. It requires the legacy source and the
Rust target to be reachable from the same process for the whole run, and it re-derives every
catalog identity as part of the same operation.

Operators repeatedly need something narrower:

- the catalog and media are already in Melodee, but the listener data still lives in C#;
- the same listener state must be applied to a second deployment;
- the C# server stays live for a while after cutover, so the data must be refreshed periodically;
- a refresh must be safe to repeat as the catalog grows and more liked tracks are found.

The only existing option was to re-run the full migration, which is heavyweight and couples
listener data to catalog re-import. There was also no re-runnable contract: nothing defined what a
second import should do with newer local stars, locally edited playlists, or an older export.

## Decision

1. Add a **versioned portable export format**: a `manifest.json` (schema version, generator, RFC
   3339 generation time, non-sensitive source fingerprint, per-stream row counts, per-stream
   SHA-256, and a `referenceOnly` map of domains intentionally not exported with counts and
   reasons) plus one NDJSON stream per domain:

   `users`, `entity-states`, `track-plays`, `playlists`, `playlist-items`, `smart-playlists`,
   `bookmarks`, `groups`, `group-members`, `playback-settings`, `equalizer-presets`.

   Rows are addressed only by preserved public ids, written in stable order with sorted keys, and
   carry RFC 3339 timestamps.

2. Add `scripts/export_legacy_user_state.py`: a Python 3, read-only extractor that produces that
   directory from a C# Melodee snapshot. It follows the existing repository precedent for
   PostgreSQL access (the `psql` client, because the standard library has no driver), sets
   `default_transaction_read_only = on`, refuses a non-empty output directory without `--force`,
   includes portable bcrypt hashes by default (`--no-credentials` omits them, which would strand
   created accounts behind a password reset), and reports secret-bearing or
   feed-dependent domains in `referenceOnly` instead of dropping them silently.

3. Add `melodee-cli users import-state --input DIR [--dry-run] [--allow-unmatched]
   [--no-create-users] [--grant-privileged-roles] [--batch-size N] [--json]`. It validates the manifest version, every file
   checksum and row count, line/stream bounds, timestamp shape and ordering, rating bounds, and
   cross-stream references before writing anything.

4. **The import is additive and idempotent by contract.** It never deletes rows:

   - account profile fields apply only when the export row is newer; credentials and
     administrator/editor flags of existing accounts are never overwritten;
   - likes/dislikes/ratings/pins apply only when the export row is newer, and an identical re-run
     is a no-op (`unchanged`), not revision churn;
   - play counts and last-played are monotonic maxima;
   - play history inserts are deduplicated by a stable event id derived from user, track, and
     played-at time;
   - playlist items are appended only when the track is not already present, and existing order
     and local edits are preserved;
   - smart playlists, bookmarks, groups, playback settings, and equalizer presets merge by natural
     key with the same last-writer-wins rule; memberships are additive.

5. **Identity is the preserved public id**, never a name, title, path, or fuzzy match. Users,
   catalog entities, playlists, smart playlists, presets, and groups resolve by public id; catalog
   entities must also be live. Unresolved rows fail the run unless `--allow-unmatched` is given,
   and are always counted and reported.

6. **Missing accounts are created by default, and creation is unprivileged.** Importing user data
   exists to create or update user records, so an export that references unknown accounts creates
   them with the exported profile and either the exported portable bcrypt hash or an unguessable
   placeholder credential, all in the same run. `--no-create-users` makes every user have to exist
   already. Created accounts are never administrators or editors unless the operator explicitly
   passes `--grant-privileged-roles`; existing accounts never change credential or privilege.

7. Secret-bearing controls, device profiles, podcast/feed state, user-authored radio,
   requests/comments, and retired Party/Jukebox state remain the governed migration path's
   responsibility and are listed in `referenceOnly`.

## Alternatives considered

### Re-run the full legacy migration for listener refresh

Rejected. It re-imports the catalog, couples listener data to catalog identity derivation, and
requires the same operator ceremony as a cutover rehearsal. It also has no defined merge rule for
data that changed on both sides.

### CSV or SQL-dump interchange

Rejected for the default path. SQL dumps couple the importer to the C# schema and cannot express
the reference-only dispositions; CSV loses types, nullability, and nested values (equalizer bands,
avatars) and needs a separate manifest anyway. NDJSON with a checksummed manifest is
self-describing, streamable, and diffable.

### Upsert with delete-and-replace reconciliation

Rejected. It is destructive on re-run: removing a like locally would be undone by an older export,
and a locally curated playlist would be reset. The requirement is explicit that repeated imports
must not destroy local state.

### Match users/entities by normalized names when public ids do not line up

Rejected. PRD-MIG-002 and the catalog identity rules make public ids the identity contract; name
matching would silently attach one person's stars to another artist or account.

### Require an explicit flag before creating accounts

Rejected. The purpose of the import is to create or update user records; refusing unknown accounts
by default turned the intended operation into an error and forced an extra switch for every real
migration. `--no-create-users` documents and enforces the strict mode for deployments that must
match an existing account inventory.

### Import privileged flags without an explicit opt-in

Rejected. Privilege is authority, so it stays behind `--grant-privileged-roles` even though
credentials and profiles are copied by default.

## Rationale

Listener data is high-value, long-lived, and personal; catalog data is large and already migrated.
Separating them lets operators refresh the personal half cheaply and safely, while the
non-destructive merge contract makes repeated runs the expected workflow rather than a risk.

## Consequences

### Positive

- Listener data can be moved or refreshed without a second full migration.
- Re-running after new music arrives adds only what is new; local edits survive.
- Public ids remain the only identity mechanism, so a wrong or partial catalog fails loudly
  instead of attaching state to the wrong entity.
- Nothing is silently dropped: `referenceOnly` carries counts and reasons, and every domain prints
  created/updated/unchanged/duplicate/skipped counts.

### Negative / trade-offs

- The format must be versioned and kept in step with the schema; `SCHEMA_VERSION` gates
  incompatible changes.
- `psql` is required for extraction (documented; the same constraint the runtime audit tooling
  already has).
- Secret-bearing and feed-dependent domains still require the governed migration path.
- Identical-timestamp rows are treated as no-ops, so a same-second edit on both sides resolves in
  favor of local state and is reported as `unchanged` rather than applied.

### Follow-up work

- Extend the format to podcast subscriptions/episodes, user-authored radio, and community
  requests/comments once their target semantics are portable.
- Optional avatars in the Python exporter (requires image normalization that currently lives in
  the Rust pipeline).

## Validation

- `apps/melodee-cli/src/user_state_import.rs` unit tests: manifest version/checksum/count
  rejection, duplicate keys, timestamp and rating bounds, stable play event ids, orphan playlist
  item rejection, `referenceOnly` reporting.
- `apps/melodee-cli/tests/user_state_import.rs`: end-to-end binary run against PostgreSQL proving
  first import, identical re-run no-op, refreshed export merging (new state, new play, appended
  playlist item), monotonic play count, preserved playlist order, and a dry run that writes
  nothing.
- `tests/python/test_export_legacy_user_state.py`: timestamp/boolean/integer parsing, deterministic
  checksummed stream writing, output-directory safety, reference-only reasons.
- `scripts/check_command_help.py` matrix includes `users import-state`, so CLI help stays covered
  by the repository gate.

## References

- [ADR-0148](ADR-0148-legacy-migration-contract.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- Public documentation: [Importing user data and activity](../../docs/src/content/docs/installation/user-state-import.md)
