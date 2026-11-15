---
title: Importing user data and activity
description: Move likes, dislikes, ratings, pins, play history, playlists, bookmarks, groups, and playback preferences from a C# Melodee snapshot into a Rust deployment, as often as you like.
sidebar:
  order: 7
---

Melodee can carry a listener's **likes, dislikes, ratings, pins, play counts, play history,
playlists, smart playlists, bookmarks, access groups, and playback preferences** from a C# Melodee
database into a Rust deployment without touching the legacy server.

This is a two-step process:

1. `scripts/export_legacy_user_state.py` reads the C# database (read-only) and writes a versioned,
   checksummed export directory.
2. `melodee-cli users import-state` applies that directory to the Rust database.

The import is designed to be **re-run**: it never deletes rows, an older export cannot downgrade a
newer local change, play counts are monotonic maxima, and playlist items are only appended. Export
again when the source listener data changes. After adding music, re-run the **same export** to
apply state for newly available tracks; music that is still missing is skipped and reported.

:::note[When to use this instead of the full legacy migration]
The full [`melodee-cli legacy`](./legacy-migration/) path migrates catalog, media, and users in one
governed run. Use this portable path when the catalog is already in Melodee and you want to add or
refresh listener data — or when you need the same listener state on a second deployment.
:::

## Requirements

- The C# Melodee database must be reachable and **quiesced** (or a snapshot of it must be). The
  exporter sets `default_transaction_read_only = on` for every statement and never writes to the
  source.
- The `psql` client must be on `PATH`. Python's standard library cannot speak the PostgreSQL
  protocol, so the exporter uses the same PostgreSQL client the runtime audit tooling uses.
- The Rust target database must already be migrated (`melodee-cli migrate`) and must contain the
  catalog entries you want to match (see [Identity matching](#identity-matching)). Missing accounts
  are created by default; the catalog can be incomplete.
- `MELODEE__DATABASE__URL` must point at the Rust database when running the import.

## Step 1 — Export from C#

```bash
python3 scripts/export_legacy_user_state.py \
  --source-db postgres://user:pass@host/legacy_snapshot \
  --output ./legacy-user-state
```

| Option | Effect |
|---|---|
| `--source-db` | Legacy PostgreSQL URL. Defaults to `MELODEE_LEGACY_SOURCE_DATABASE_URL`. |
| `--output` | Export directory to create or refresh. |
| `--force` | Overwrite an existing export directory that already contains export files. |
| `--credentials` / `--no-credentials` | Include portable bcrypt password hashes so created accounts keep their passwords. **Included by default**; use `--no-credentials` to omit them. Treat the export directory as sensitive either way. |
| `--limit N` | Write at most N rows per stream. Rehearsal aid only. |
| `--json` | Print the manifest instead of the row-count table. |

The directory contains `manifest.json` (schema version, per-stream row counts, SHA-256 checksums,
and a `referenceOnly` section) plus one NDJSON stream per domain:

| Stream | Contents |
|---|---|
| `users.ndjson` | Account identity, profile fields, time zone, description, optional avatar |
| `entity-states.ndjson` | Per-user Artist/Release/Track star, dislike, rating, pin, play count, last played |
| `track-plays.ndjson` | Individual play-history events |
| `playlists.ndjson` | Static playlists with their public ids |
| `playlist-items.ndjson` | Static playlist ordering |
| `smart-playlists.ndjson` | Smart playlists and their MQL query |
| `bookmarks.ndjson` | OpenSubsonic bookmarks |
| `groups.ndjson` | Library access groups |
| `group-members.ndjson` | Group membership |
| `playback-settings.ndjson` | Per-user playback preferences |
| `equalizer-presets.ndjson` | Per-user equalizer presets |

Rows are written in a stable order with sorted keys and are addressed by **public id**, so the
export is diff-friendly and safe to keep for later refreshes.

### Tolerated legacy anomalies

Real C# snapshots are not perfectly uniform. The exporter repairs or reports these instead of
aborting a long export:

- A missing or earlier `LastUpdatedAt` falls back to the row's `CreatedAt`, so rows that predate
  the C# audit trigger still migrate.
- Rows without a public id (`ApiKey`), play-history rows without a played-at time, membership rows
  without a created-at time, and equalizer presets with unusable band JSON are skipped and counted
  under `referenceOnly` (`invalid.*`, `*.skipped`).
- Reference-only tables that are not present in the snapshot are summarized as a single
  `absentTables` entry with the table names, which usually means the source runs a different C#
  version.

### What is deliberately not exported

The manifest's `referenceOnly` section lists domains with their row counts and the reason they are
not portable, so nothing disappears silently:

- **Secrets and session material** — share bearer tokens, Last.fm session keys, social-login
  subjects/tokens, OpenSubsonic app passwords.
- **Deployment-shaped state** — per-user device profiles (Rust device profiles are global
  metadata), group library bindings (bind libraries in the Admin UI after migration).
- **Feed-dependent and community state** — podcast subscriptions/episodes and their history,
  user-authored radio stations, requests/comments/seen state.
- **Retired features** — Party/Jukebox sessions and play queues.

Use [`melodee-cli legacy import`](./legacy-migration/) for those.

## Step 2 — Import into Rust

Preview first. A dry run validates the manifest, checksums, row bounds, and identity resolution,
then prints the plan without writing anything. Accounts it would create are reported as
`usersPlannedCreated`; nothing is written.

```bash
melodee-cli users import-state --input ./legacy-user-state --dry-run
```

Apply it:

```bash
melodee-cli users import-state --input ./legacy-user-state
```

| Option | Effect |
|---|---|
| `--dry-run` | Validate, resolve identities, and report; write nothing. |
| `--no-create-users` | Do not create missing accounts; every exported user must already exist. **Missing accounts are created by default.** |
| `--grant-privileged-roles` | Keep the exported administrator/editor flags on **newly created** accounts. Off by default; existing accounts never change privilege. |
| `--allow-unmatched` | Skip missing users when combined with `--no-create-users`. Missing catalog entities are already skipped and reported by default. |
| `--require-all-matches` | Fail before writing if any referenced catalog entity is missing or not live. Cannot be combined with `--allow-unmatched`. |
| `--batch-size N` | Rows per committed transaction (default 1000). |
| `--json` | Emit the machine-readable summary. |

Accounts in the export that do not exist locally are **created** (unprivileged, using the exported
portable bcrypt hash when present, otherwise an unguessable placeholder credential that requires a
password reset). Existing accounts keep their credentials and privileges. Rows for missing or
inactive catalog entries are **skipped and counted**, while matched rows are imported. Use
`--require-all-matches` when you need to verify complete catalog coverage before writing.

## Re-running as the catalog grows

Keep the export directory and run the same command after more music has been added:

```bash
melodee-cli users import-state --input ./legacy-user-state --dry-run
melodee-cli users import-state --input ./legacy-user-state
```

Each run checks the current catalog again. Newly matched tracks receive their likes, dislikes,
play counts, history, playlist entries, and bookmarks. Rows still unmatched remain in the export
for a later run; no new export is needed unless the listener data in C# has changed.

Each row is merged by its natural key with these rules:

- **Account profile** — applied only when the export row is newer than the local row. Credentials
  and administrator/editor flags of existing accounts are never changed.
- **Likes, dislikes, ratings, pins** — applied only when the export row is newer. An identical
  re-run is a true no-op and reports `unchanged`. Equal timestamps preserve local flags.
- **Play counts and last played** — `max(local, export)`, so a re-run can add newly discovered
  plays but never lowers a count.
- **Play history** — additive. Events are deduplicated by a stable id derived from user, track,
  and played-at time, so replays of the same export insert nothing.
- **Playlists** — a playlist that already has items is never reordered or emptied. Tracks it does
  not contain yet are appended in export order; tracks it already contains are skipped. Local
  edits and locally added tracks survive.
- **Smart playlists, bookmarks, groups, playback settings, equalizer presets** — applied by
  natural key only when newer; memberships are additive.

The summary reports created, updated, unchanged, and duplicate counts, plus unmatched counts for
entity states, play history, playlist items, and bookmarks. JSON output includes `statesUnmatched`,
`playsUnmatched`, `playlistItemsUnmatched`, and `bookmarksUnmatched`. The `referenceOnly` domains
are listed separately. Dry-run row counts include only rows whose user and catalog entity resolve.

## Identity matching

Everything is matched by preserved public id:

- **Users** by their C# `ApiKey` (stored as `public_api_id`); missing accounts are created by default.
- **Artists, Releases, Tracks** by their preserved `ApiKey`, and only while they are live (`Active`
  / `Verified`; deleted and merged entries are skipped).
- **Playlists, smart playlists, equalizer presets, groups** by their own public ids.

If the Rust catalog was not produced by the full legacy migration, public ids will not line up and
rows are reported as unmatched. That is expected and safe: nothing is guessed from names, titles,
or file paths.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `psql is not on PATH` | Install the PostgreSQL client on the machine running the export. |
| `checksum mismatch` | The export directory was edited or partially copied. Re-run the export. |
| `user(s) in the export do not exist on this server` | You passed `--no-create-users`. Remove it to create the accounts, or add `--allow-unmatched` to skip their data. |
| Rows reported as `unmatched` | Their user or live catalog entry is missing. Add the music with preserved public ids and re-run the same export. Matched rows are still imported. |
| `reference catalog entities that are missing or not live` | You passed `--require-all-matches`. Remove it to import matched rows now, or complete the catalog before retrying. |
| `playlist ... different owner` | The playlist public id already exists locally under another account. The importer refuses to steal it; resolve the conflict manually. |
| Nothing was written but the command succeeded | The export is already applied, local state is newer, or no catalog ids match. Review the `unchanged`, `unmatched`, and `referenceOnly` counts. |
