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
| `catalog-evidence.ndjson` | Legacy artist, release, title, track number, and duration for referenced music. Used to review possible matches in a rebuilt library. |
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
| `--require-all-matches` | Fail before writing if a reference is missing, inactive, or still needs approval. Cannot be combined with `--allow-unmatched`. |
| `--identity-map FILE` | Use reviewed old and new public IDs for this Rust installation. Linked accounts keep their existing login, roles, and profile. |
| `--batch-size N` | Rows per committed transaction (default 1000). |
| `--json` | Emit the machine-readable summary. |

After checking public IDs, saved links, and unique email matches, missing accounts are **created** (unprivileged, using the exported
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

Read **Favorites (likes)** to see how many times listeners marked an artist, release, or track as a
favorite. The next three lines break that total down by type. Two listeners who like the same
track each contribute one favorite. Dislikes, ratings, and pins have their own counts. A listener
can both rate and like a track, so the same preference record can appear in both counts.

**In export** counts all preferences in the source files. **Matched** counts preferences for an
existing or planned account when the importer can identify one live catalog item. This includes
a unique match on artist, release, and track title. **Unmatched** counts preferences for missing
accounts, missing music, and tracks with conflicting candidates. If a listener rated a track one
star and also liked it, finding that track adds one to both **Ratings** and **Favorite tracks**.
Both are ready to import. An existing newer choice can still take precedence.

In JSON, `exportedPreferences` counts the source data and `matchedPreferences` counts matched
preferences. Each listener’s breakdown is in `preferencesByUser`. `supersededPreferences`
counts legacy choices superseded by existing ones. `tracksMatchedByMetadata` counts newly
identified exact track pairs. `tracksWithAmbiguousMatches` counts tracks requiring review.
`statesUnmatched`, `playsUnmatched`, `playlistItemsUnmatched`, and `bookmarksUnmatched` count
rows skipped on this run. Listening history counts individual plays. In the account summary,
`usersMatchedByEmail` counts new email matches, `usersLinked` counts saved links, and
`usersRequiringReview` counts account conflicts.

## Identity matching

The importer first looks up saved links and preserved public IDs.

- **Users** with different public IDs can match automatically by email. The address must be unique in both the export and the Rust database after trimming spaces and ignoring case. A renamed username is fine. The preview reports these matches separately. Missing accounts are created by default.
- **Artists, Releases, Tracks** by their preserved `ApiKey`, and only while they are live (`Active`
  / `Verified`; deleted and merged entries are skipped).
- **Rebuilt tracks** also match automatically when exactly one referenced legacy track and one live track share the release artist, release title, and track title.
- **Playlists, smart playlists, equalizer presets, groups** by their own public ids.

### When you are rebuilding from the old music files

Reprocessing the same files normally gives them new public IDs. Keep the export and one identity
map in a private directory that survives reboots. An older export without
`catalog-evidence.ndjson` can still import preserved IDs. To find matches by metadata, export once
more from the quiesced legacy database into a new private directory before retiring it.

When the import preview reports accounts to review or ambiguous tracks, open a terminal on your workstation in the
`melodee-rs` checkout and run the review command printed in the preview. Each **Legacy user** is
followed by numbered accounts from the target server. Confirm a pair only if both accounts belong
to the same person. You can also review suggested music matches here after each batch of releases.
Both commands use `MELODEE__DATABASE__URL` for the Rust target. These examples use a directory
named `legacy-user-state` and a map named `approved-links.json` in the current directory.

```bash
python3 scripts/reconcile_legacy_user_state.py --input ./legacy-user-state --identity-map ./approved-links.json --review
melodee-cli users import-state --input ./legacy-user-state --identity-map ./approved-links.json --dry-run
melodee-cli users import-state --input ./legacy-user-state --identity-map ./approved-links.json
```

The review tool only reads PostgreSQL. It shows both sides of each suggested link. Enter the
candidate number to confirm the pair, press Enter to skip it, or enter `q` to finish. Each approval
is saved immediately on your workstation and reused across runs. Omit `--review` for a preview
that also leaves the map untouched. Add `--show-pending` to list source items without suggestions.

- Accounts matched by email need no confirmation. On apply, the CLI saves their public IDs in
  `account-links-<installation UUID>.json` inside the export directory. Keep that file with the
  export so a future change of email still points to the same person. A dry run leaves it untouched.
  Linked users keep their Rust login, password, profile, avatar, and roles.
- Review accounts when identifiers disagree, for example when an old username and email point to
  different Rust users. Exporter-generated addresses such as `migrated-user-123@import.local` cannot establish an automatic link.
  If you cannot identify the person, press Enter to leave the account unresolved.
- Artist suggestions use the artist name. Release suggestions use the artist and release title.
  The import preview compares the release artist, release title, and track title together.
  It ignores differences in case and whitespace and retains edition and version text. A unique
  match requires one exported track and one live track with those names. On apply, the CLI saves
  the pair in `track-links-<installation UUID>.json` in the export directory. Keep this file
  with the export. A later edition or title change will still resolve to the originally matched
  track. When that track is unavailable, its listener data stays pending. The review tool shows
  track numbers and durations where available to help distinguish editions. If several tracks
  share those names, confirm the edition you recognize or leave it pending.
- If the names changed enough to prevent a suggestion, locate the exact item
  in the rebuilt library and verify it independently. With the review tool closed, you can add a
  confirmed pair to the map's `approved` array, for example
  `{"kind":"track","legacyId":"<old public UUID>","targetId":"<new public UUID>"}`.
  Each public ID can occur only once per side and kind. Keep a backup of the map. Editing a pair
  after applying it does not undo any data previously imported.
- For the final check, add `--require-all-matches --dry-run` to the import command. A successful
  check accounts for all referenced music. Apply the same export, then run it once more to confirm
  zero new preference records and plays. Review any counts for current listener choices that take
  precedence over a legacy favorite, dislike, rating, or pin. If a recording is still missing,
  retain the export and retry when it becomes available.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `psql is not on PATH` | Install the PostgreSQL client on the machine running the export. |
| `checksum mismatch` | The export directory was edited or partially copied. Re-run the export. |
| `user(s) in the export do not exist on this server` | You passed `--no-create-users`. Remove it to create the accounts, or add `--allow-unmatched` to skip their data. |
| Preferences under **Unmatched** | Check whether the account and release exist. If several tracks share the same artist, release, and title, review the candidates. Repeat the same export after adding music or approving a link. |
| `missing or not live` with `--require-all-matches` | Complete the catalog and approve outstanding matches, or remove the flag to import ready rows now. |
| `playlist ... different owner` | The playlist public id already exists locally under another account. The importer refuses to steal it; resolve the conflict manually. |
| Nothing was written but the command succeeded | The export is already applied, local state is newer, or no catalog ids match. Review the `unchanged`, `unmatched`, and `referenceOnly` counts. |
