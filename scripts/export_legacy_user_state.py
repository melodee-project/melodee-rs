#!/usr/bin/env python3
"""Export C# Melodee user data and activity into a portable import directory.

Why this exists
---------------
The `melodee-cli legacy import` path reads the C# database directly and imports
every domain at once, including catalog rows and media adoption. Operators who
already migrated their catalog (or who are adding a second deployment) still
need the *user* half: likes, dislikes, ratings, pins, play history, playlists,
smart playlists, bookmarks, groups, and playback preferences.

This script reads a C# Melodee snapshot strictly read-only and writes a
versioned, checksummed export directory that `melodee-cli users import-state`
consumes. The import is additive and last-writer-wins by timestamp, so the
export can be regenerated and re-applied as more music and more likes are found
without deleting or downgrading local state.

Output directory
----------------
    manifest.json              schema version, per-stream row counts + SHA-256
    users.ndjson               account/profile/avatar rows
    entity-states.ndjson       artist/release/track star/hate/rating/pin/plays
    catalog-evidence.ndjson    names, editions, positions and durations for referenced music
    track-plays.ndjson         play history events
    playlists.ndjson           static playlists
    playlist-items.ndjson      static playlist ordering
    smart-playlists.ndjson     smart playlists (MQL)
    bookmarks.ndjson           OpenSubsonic bookmarks
    groups.ndjson              library access groups
    group-members.ndjson       group membership
    playback-settings.ndjson   per-user playback preferences
    equalizer-presets.ndjson   per-user equalizer presets

Usage
-----
    python3 scripts/export_legacy_user_state.py \
        --source-db postgres://user:pass@host/legacy_snapshot \
        --output ./legacy-user-state

    melodee-cli users import-state --input ./legacy-user-state --dry-run

Requirements
------------
The `psql` client must be on PATH: Python's standard library cannot speak the
PostgreSQL protocol, and this repository does not vendor a database driver (the
existing runtime audit script follows the same rule). Every statement runs with
`default_transaction_read_only = on`; the source is never modified.

Portable bcrypt password hashes are included by default so a created account
keeps its password; pass `--no-credentials` to omit them. Treat the export
directory as sensitive in either case, and never print rows or addresses. Domains that carry secrets or feed-dependent state (shares, Last.fm
sessions, social logins, OpenSubsonic app passwords, device profiles, podcast
subscriptions/episodes, user-authored radio, requests/comments) are reported in
the manifest's `referenceOnly` section with row counts and reasons instead of
being silently dropped. Use `melodee-cli legacy import` for those.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from uuid import UUID

SCHEMA_VERSION = 1
GENERATOR = {"name": "melodee-legacy-user-state-export", "version": "1.0.0"}
DEFAULT_SOURCE_ENV = "MELODEE_LEGACY_SOURCE_DATABASE_URL"

STREAMS = (
    "users.ndjson",
    "entity-states.ndjson",
    "catalog-evidence.ndjson",
    "track-plays.ndjson",
    "playlists.ndjson",
    "playlist-items.ndjson",
    "smart-playlists.ndjson",
    "bookmarks.ndjson",
    "groups.ndjson",
    "group-members.ndjson",
    "playback-settings.ndjson",
    "equalizer-presets.ndjson",
)

# Legacy tables intentionally not exported, with the reason reported in the
# manifest. The operator sees counts, so nothing disappears silently.
REFERENCE_ONLY_TABLES = {
    "Shares": "share bearer secrets are not portable; recreate or rotate after migration",
    "ShareActivities": "telemetry with no user-visible meaning",
    "SocialLogins": "provider subject/token material is secret",
    "UserDeviceProfiles": "target device profiles are global metadata, not per-user rows",
    "PodcastChannels": "feed-dependent state; use the legacy import path",
    "PodcastEpisodes": "feed-dependent state; use the legacy import path",
    "PodcastEpisodeBookmarks": "feed-dependent state; use the legacy import path",
    "PodcastEpisodePlayHistories": "feed-dependent state; use the legacy import path",
    "RadioStations": "user-authored radio is not portable in v1; recreate or use the legacy import",
    "Requests": "community requests are not portable in v1; use the legacy import path",
    "RequestComments": "community comments are not portable in v1; use the legacy import path",
    "RequestSeen": "community seen state is not portable in v1; use the legacy import path",
    "Notifications": "user notifications are server-local delivery history",
    "PartySessions": "retired legacy feature (ADR-0133)",
    "PartyQueueItems": "retired legacy feature (ADR-0133)",
    "PlayQues": "retired legacy feature (ADR-0133)",
}


# Rows that cannot be represented (missing public id, unusable timestamp). They
# are counted and reported in the manifest instead of aborting a long export.
SKIPS: dict[str, int] = {}


def skip(domain: str, reason: str) -> None:
    key = f"{domain}: {reason}"
    SKIPS[key] = SKIPS.get(key, 0) + 1


def public_id(value: str | None) -> str | None:
    return text(value)


class ExportError(RuntimeError):
    """Raised when the export cannot be produced safely."""


@dataclass
class Row:
    """One exported row plus its stream."""

    stream: str
    payload: dict


def psql_binary() -> str:
    found = shutil.which("psql")
    if not found:
        raise ExportError("psql is not on PATH; install the PostgreSQL client to export")
    return found


def run_query(psql: str, url: str, sql: str) -> list[list[str]]:
    """Runs one read-only query and returns unaligned rows as text."""
    completed = subprocess.run(
        [
            psql,
            url,
            "-X",
            "-q",
            "--no-align",
            "--field-separator",
            "\x1f",
            "--tuples-only",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            "SET default_transaction_read_only = on",
            "-c",
            sql,
        ],
        shell=False,
        capture_output=True,
        text=True,
        check=False,
        env={**os.environ, "PGCONNECT_TIMEOUT": "10"},
    )
    if completed.returncode != 0:
        message = completed.stderr.strip().splitlines()
        raise ExportError(
            "source query failed: " + (message[0] if message else "unknown psql error")
        )
    rows = []
    for line in completed.stdout.splitlines():
        if not line:
            continue
        rows.append(line.split("\x1f"))
    return rows


def timestamp(value: str | None, field: str) -> str | None:
    """Normalizes a PostgreSQL timestamptz string to RFC 3339 UTC."""
    if value is None or not value.strip():
        return None
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ExportError(f"{field} value {value!r} has no time zone")
    return parsed.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def required_timestamp(value: str | None, field: str) -> str:
    normalized = timestamp(value, field)
    if normalized is None:
        raise ExportError(f"{field} is required but the source row has no value")
    return normalized


def clamped_timestamps(
    created: str | None, updated: str | None, label: str
) -> tuple[str, str]:
    """Returns (createdAt, updatedAt), tolerating a NULL or earlier update time.

    The C# schema allows `LastUpdatedAt` to be NULL on rows that predate its
    audit trigger, and legacy data occasionally has it earlier than
    `CreatedAt`. The Rust importer requires RFC 3339 timestamps with
    `createdAt <= updatedAt`, so a missing or inverted update time falls back
    to the creation time instead of failing the whole export.
    """
    created_at = timestamp(created, f"{label}.CreatedAt")
    updated_at = timestamp(updated, f"{label}.LastUpdatedAt")
    if created_at is None and updated_at is None:
        raise ExportError(f"{label} row has neither CreatedAt nor LastUpdatedAt")
    if created_at is None:
        created_at = updated_at
    if updated_at is None or updated_at < created_at:
        updated_at = created_at
    return created_at, updated_at


def text(value: str | None) -> str | None:
    if value is None:
        return None
    stripped = value.strip()
    return stripped or None


def boolean(value: str | None) -> bool:
    return value is not None and value.strip().lower() in {"t", "true", "1"}


def integer(value: str | None, default: int = 0) -> int:
    if value is None or not value.strip():
        return default
    return int(value)


def export_users(psql: str, url: str, include_credentials: bool) -> list[Row]:
    sql = (
        'SELECT "Id", "ApiKey", "UserName", "Email", "IsAdmin", "IsEditor", "IsLocked",'
        ' "PreferredLanguage", "PreferredTheme", "TimeZoneId", "Description",'
        ' "CreatedAt", "LastUpdatedAt", "PasswordHash", "PasswordHashAlgorithm"'
        ' FROM "Users" ORDER BY "Id"'
    )
    rows = []
    for line in run_query(psql, url, sql):
        if len(line) < 15:
            raise ExportError("Users row had an unexpected column count")
        (
            source_id,
            api_key,
            username,
            email,
            is_admin,
            is_editor,
            is_locked,
            language,
            theme,
            time_zone,
            description,
            created,
            updated,
            password_hash,
            password_algorithm,
        ) = line
        if not public_id(api_key):
            skip("Users", "row has no ApiKey")
            continue
        created_at, updated_at = clamped_timestamps(created, updated, "Users")
        payload = {
            "publicId": api_key,
            "username": text(username) or f"user-{source_id}",
            "email": text(email) or f"migrated-user-{source_id}@import.local",
            "isAdmin": boolean(is_admin),
            "isEditor": boolean(is_editor),
            "enabled": True,
            "locked": boolean(is_locked),
            "blacklisted": False,
            "preferredLanguage": text(language),
            "preferredTheme": text(theme),
            "timeZoneId": text(time_zone) or "UTC",
            "description": text(description),
            "createdAt": created_at,
            "updatedAt": updated_at,
        }
        if include_credentials:
            if (password_algorithm or "").strip().lower() == "bcrypt" and (
                password_hash or ""
            ).startswith("$2"):
                payload["credential"] = {
                    "algorithm": "bcrypt",
                    "hash": password_hash,
                }
        rows.append(Row("users.ndjson", payload))
    return rows


def export_entity_states(psql: str, url: str) -> tuple[list[Row], int]:
    # (source table, entity key, entity column, has play stats)
    sources = (
        ("UserSongs", "SongId", "track", True),
        ("UserAlbums", "AlbumId", "release", True),
        ("UserArtists", "ArtistId", "artist", False),
    )
    states: dict[tuple[str, str, str], dict] = {}
    for table, entity_column, entity, has_play_stats in sources:
        sql = entity_state_query(table, entity_column, entity, has_play_stats)
        for line in run_query(psql, url, sql):
            if len(line) < 10:
                raise ExportError(f"{table} row had an unexpected column count")
            (
                user_api_key,
                _entity_id,
                entity_api_key,
                starred,
                hated,
                rating,
                played_count,
                last_played_at,
                created,
                updated,
            ) = line
            if not public_id(user_api_key) or not public_id(entity_api_key):
                skip(f"{table}", "row has no user or entity ApiKey")
                continue
            key = (user_api_key, entity, entity_api_key)
            created_at, updated_at = clamped_timestamps(
                created, updated, table
            )
            states[key] = {
                "userPublicId": user_api_key,
                "entity": entity,
                "entityPublicId": entity_api_key,
                "starred": boolean(starred),
                "hated": boolean(hated),
                "rating": integer(rating),
                "pinned": states.get(key, {}).get("pinned", False),
                "playCount": integer(played_count),
                "lastPlayedAt": timestamp(last_played_at, f"{table}.LastPlayedAt"),
                "createdAt": created_at,
                "updatedAt": updated_at,
            }

    # Pins set the `pinned` flag on the same state row; pin-only entities get a
    # row with defaults so the flag is never lost. Each pin type targets a
    # different legacy table, so the join is per type.
    pin_sources = (
        (1, "artist", "Artists"),
        (2, "release", "Albums"),
        (3, "track", "Songs"),
    )
    unpublished = 0
    for pin_type, entity, target_table in pin_sources:
        sql = (
            f'SELECT u."ApiKey", a."ApiKey", p."CreatedAt"'
            f' FROM "UserPins" p'
            f' JOIN "Users" u ON u."Id" = p."UserId"'
            f' JOIN "{target_table}" a ON a."Id" = p."PinId"'
            f' WHERE p."PinType" = {pin_type}'
            f' ORDER BY u."ApiKey", p."PinId"'
        )
        for line in run_query(psql, url, sql):
            if len(line) < 3:
                raise ExportError("UserPins row had an unexpected column count")
            user_api_key, entity_api_key, created = line
            if not public_id(entity_api_key) or not public_id(user_api_key):
                unpublished += 1
                continue
            key = (user_api_key, entity, entity_api_key)
            if key in states:
                states[key]["pinned"] = True
                continue
            created_at, updated_at = clamped_timestamps(
                created, created, "UserPins"
            )
            states[key] = {
                "userPublicId": user_api_key,
                "entity": entity,
                "entityPublicId": entity_api_key,
                "starred": False,
                "hated": False,
                "rating": 0,
                "pinned": True,
                "playCount": 0,
                "lastPlayedAt": None,
                "createdAt": created_at,
                "updatedAt": updated_at,
            }
    return [Row("entity-states.ndjson", payload) for payload in states.values()], unpublished


def entity_table(entity: str) -> str:
    return {"artist": "Artists", "release": "Albums", "track": "Songs"}[entity]


def entity_state_query(
    table: str, entity_column: str, entity: str, has_play_stats: bool
) -> str:
    """Builds the per-user state query for one legacy state table.

    Every column is qualified with its table alias. The legacy catalog tables
    (`Songs`, `Albums`) also carry aggregate `PlayedCount`/`LastPlayedAt`
    columns, so an unqualified reference is ambiguous in a real C# database.
    """
    stats = (
        's."PlayedCount", s."LastPlayedAt"'
        if has_play_stats
        else 'NULL::integer, NULL::timestamptz'
    )
    return (
        f'SELECT u."ApiKey", s."{entity_column}", a."ApiKey", s."IsStarred",'
        f' s."IsHated", s."Rating", {stats}, s."CreatedAt", s."LastUpdatedAt"'
        f' FROM "{table}" s'
        f' JOIN "Users" u ON u."Id" = s."UserId"'
        f' JOIN "{entity_table(entity)}" a ON a."Id" = s."{entity_column}"'
        f' ORDER BY u."ApiKey", s."{entity_column}"'
    )


def export_track_plays(psql: str, url: str) -> tuple[list[Row], int]:
    sql = (
        'SELECT u."ApiKey", t."ApiKey", h."PlayedAt", h."SecondsPlayed", h."IsNowPlaying"'
        ' FROM "UserSongPlayHistories" h'
        ' JOIN "Users" u ON u."Id" = h."UserId"'
        ' JOIN "Songs" t ON t."Id" = h."SongId"'
        ' ORDER BY h."PlayedAt", u."ApiKey", t."ApiKey"'
    )
    rows = []
    skipped = 0
    for line in run_query(psql, url, sql):
        if len(line) < 5:
            raise ExportError("UserSongPlayHistories row had an unexpected column count")
        user_api_key, track_api_key, played_at, seconds, now_playing = line
        if not public_id(user_api_key) or not public_id(track_api_key):
            skip("UserSongPlayHistories", "row has no user or track ApiKey")
            skipped += 1
            continue
        if timestamp(played_at, "PlayedAt") is None:
            skipped += 1
            continue
        rows.append(
            Row(
                "track-plays.ndjson",
                {
                    "userPublicId": user_api_key,
                    "trackPublicId": track_api_key,
                    "playedAt": required_timestamp(played_at, "PlayedAt"),
                    "durationSeconds": integer(seconds) if text(seconds) else None,
                    "completed": not boolean(now_playing),
                },
            )
        )
    return rows, skipped


def export_playlists(psql: str, url: str) -> tuple[list[Row], list[Row]]:
    sql = (
        'SELECT p."ApiKey", u."ApiKey", p."Name", p."Description", p."IsPublic",'
        ' p."CreatedAt", p."LastUpdatedAt"'
        ' FROM "Playlists" p'
        ' JOIN "Users" u ON u."Id" = p."UserId"'
        ' ORDER BY p."Id"'
    )
    playlists = []
    for line in run_query(psql, url, sql):
        if len(line) < 7:
            raise ExportError("Playlists row had an unexpected column count")
        api_key, owner, name, description, is_public, created, updated = line
        if not public_id(api_key) or not public_id(owner):
            skip("Playlists", "row has no playlist or owner ApiKey")
            continue
        created_at, updated_at = clamped_timestamps(created, updated, "Playlists")
        playlists.append(
            Row(
                "playlists.ndjson",
                {
                    "publicId": api_key,
                    "ownerUserPublicId": owner,
                    "name": text(name) or "Migrated playlist",
                    "description": text(description),
                    "isPublic": boolean(is_public),
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                },
            )
        )
    sql = (
        'SELECT p."ApiKey", s."PlaylistOrder", t."ApiKey"'
        ' FROM "PlaylistSong" s'
        ' JOIN "Playlists" p ON p."Id" = s."PlaylistId"'
        ' JOIN "Songs" t ON t."Id" = s."SongId"'
        ' ORDER BY p."ApiKey", s."PlaylistOrder"'
    )
    items = []
    for line in run_query(psql, url, sql):
        if len(line) < 3:
            raise ExportError("PlaylistSong row had an unexpected column count")
        playlist_api_key, position, track_api_key = line
        if not public_id(playlist_api_key) or not public_id(track_api_key):
            skip("PlaylistSong", "row has no playlist or track ApiKey")
            continue
        items.append(
            Row(
                "playlist-items.ndjson",
                {
                    "playlistPublicId": playlist_api_key,
                    "position": integer(position),
                    "trackPublicId": track_api_key,
                },
            )
        )
    return playlists, items


def export_smart_playlists(psql: str, url: str) -> tuple[list[Row], int]:
    sql = (
        'SELECT sp."ApiKey", u."ApiKey", sp."Name", sp."MqlQuery", sp."EntityType",'
        ' sp."IsPublic", sp."CreatedAt", sp."LastUpdatedAt"'
        ' FROM "SmartPlaylists" sp'
        ' JOIN "Users" u ON u."Id" = sp."UserId"'
        ' ORDER BY sp."Id"'
    )
    rows = []
    skipped = 0
    for line in run_query(psql, url, sql):
        if len(line) < 8:
            raise ExportError("SmartPlaylists row had an unexpected column count")
        api_key, owner, name, query, entity_type, is_public, created, updated = line
        normalized_entity = (entity_type or "").strip().lower()
        mapped = {
            "song": "songs",
            "songs": "songs",
            "album": "albums",
            "albums": "albums",
            "artist": "artists",
            "artists": "artists",
        }.get(normalized_entity)
        if not public_id(api_key) or not public_id(owner) or not text(query) or not mapped:
            skipped += 1
            continue
        created_at, updated_at = clamped_timestamps(
            created, updated, "SmartPlaylists"
        )
        rows.append(
            Row(
                "smart-playlists.ndjson",
                {
                    "publicId": api_key,
                    "ownerUserPublicId": owner,
                    "name": text(name) or "Migrated smart playlist",
                    "mqlQuery": query.strip(),
                    "entityType": mapped,
                    "isPublic": boolean(is_public),
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                },
            )
        )
    return rows, skipped


def export_bookmarks(psql: str, url: str) -> list[Row]:
    sql = (
        'SELECT u."ApiKey", t."ApiKey", b."Position", b."Comment", b."CreatedAt",'
        ' b."LastUpdatedAt"'
        ' FROM "Bookmarks" b'
        ' JOIN "Users" u ON u."Id" = b."UserId"'
        ' JOIN "Songs" t ON t."Id" = b."SongId"'
        ' ORDER BY u."ApiKey", t."ApiKey"'
    )
    rows = []
    for line in run_query(psql, url, sql):
        if len(line) < 6:
            raise ExportError("Bookmarks row had an unexpected column count")
        user_api_key, track_api_key, position, comment, created, updated = line
        if not public_id(user_api_key) or not public_id(track_api_key):
            skip("Bookmarks", "row has no user or track ApiKey")
            continue
        created_at, updated_at = clamped_timestamps(created, updated, "Bookmarks")
        rows.append(
            Row(
                "bookmarks.ndjson",
                {
                    "userPublicId": user_api_key,
                    "trackPublicId": track_api_key,
                    "position": integer(position),
                    "comment": text(comment),
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                },
            )
        )
    return rows


def export_groups(psql: str, url: str) -> tuple[list[Row], list[Row]]:
    sql = (
        'SELECT "ApiKey", "Name", "CreatedAt", "LastUpdatedAt"'
        ' FROM "UserGroups" ORDER BY "Id"'
    )
    groups = []
    for line in run_query(psql, url, sql):
        if len(line) < 4:
            raise ExportError("UserGroups row had an unexpected column count")
        api_key, name, created, updated = line
        if not public_id(api_key):
            skip("UserGroups", "row has no ApiKey")
            continue
        created_at, updated_at = clamped_timestamps(created, updated, "UserGroups")
        groups.append(
            Row(
                "groups.ndjson",
                {
                    "publicId": api_key,
                    "name": text(name) or "Migrated group",
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                },
            )
        )
    sql = (
        'SELECT g."ApiKey", u."ApiKey", m."CreatedAt"'
        ' FROM "UserGroupMembers" m'
        ' JOIN "UserGroups" g ON g."Id" = m."UserGroupId"'
        ' JOIN "Users" u ON u."Id" = m."UserId"'
        ' ORDER BY g."ApiKey", u."ApiKey"'
    )
    members = []
    skipped_members = 0
    for line in run_query(psql, url, sql):
        if len(line) < 3:
            raise ExportError("UserGroupMembers row had an unexpected column count")
        group_api_key, user_api_key, created = line
        if not public_id(group_api_key) or not public_id(user_api_key):
            skipped_members += 1
            continue
        created_at = timestamp(created, "UserGroupMembers.CreatedAt")
        if created_at is None:
            skipped_members += 1
            continue
        members.append(
            Row(
                "group-members.ndjson",
                {
                    "groupPublicId": group_api_key,
                    "userPublicId": user_api_key,
                    "createdAt": created_at,
                },
            )
        )
    return groups, members, skipped_members


def export_playback_settings(psql: str, url: str) -> list[Row]:
    sql = (
        'SELECT u."ApiKey", s."CrossfadeDuration", s."GaplessPlayback",'
        ' s."VolumeNormalization", s."ReplayGain", s."AudioQuality",'
        ' s."EqualizerPreset", s."LastUsedDevice", s."CreatedAt", s."LastUpdatedAt"'
        ' FROM "UserPlaybackSettings" s'
        ' JOIN "Users" u ON u."Id" = s."UserId"'
        ' ORDER BY u."ApiKey"'
    )
    rows = []
    for line in run_query(psql, url, sql):
        if len(line) < 10:
            raise ExportError("UserPlaybackSettings row had an unexpected column count")
        (
            user_api_key,
            crossfade,
            gapless,
            volume,
            replay_gain,
            quality,
            preset,
            device,
            created,
            updated,
        ) = line
        if not public_id(user_api_key):
            skip("UserPlaybackSettings", "row has no user ApiKey")
            continue
        created_at, updated_at = clamped_timestamps(
            created, updated, "UserPlaybackSettings"
        )
        rows.append(
            Row(
                "playback-settings.ndjson",
                {
                    "userPublicId": user_api_key,
                    "crossfadeDuration": float(crossfade or 0),
                    "gaplessPlayback": boolean(gapless),
                    "volumeNormalization": boolean(volume),
                    "replayGain": (replay_gain or "none").strip().lower(),
                    "audioQuality": (quality or "high").strip().lower(),
                    "equalizerPreset": text(preset),
                    "lastUsedDevice": text(device),
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                },
            )
        )
    return rows


def export_equalizer_presets(psql: str, url: str) -> tuple[list[Row], int]:
    sql = (
        'SELECT p."ApiKey", u."ApiKey", p."Name", p."BandsJson", p."IsDefault",'
        ' p."CreatedAt", p."LastUpdatedAt"'
        ' FROM "UserEqualizerPresets" p'
        ' JOIN "Users" u ON u."Id" = p."UserId"'
        ' ORDER BY u."ApiKey", p."Name"'
    )
    rows = []
    skipped = 0
    for line in run_query(psql, url, sql):
        if len(line) < 7:
            raise ExportError("UserEqualizerPresets row had an unexpected column count")
        api_key, user_api_key, name, bands_json, is_default, created, updated = line
        if not public_id(api_key) or not public_id(user_api_key):
            skipped += 1
            continue
        try:
            bands = json.loads(bands_json or "[]")
        except json.JSONDecodeError:
            skipped += 1
            continue
        if not isinstance(bands, list) or not bands:
            skipped += 1
            continue
        created_at, updated_at = clamped_timestamps(
            created, updated, "UserEqualizerPresets"
        )
        rows.append(
            Row(
                "equalizer-presets.ndjson",
                {
                    "publicId": api_key,
                    "userPublicId": user_api_key,
                    "name": text(name) or "Migrated preset",
                    "bands": bands,
                    "isDefault": boolean(is_default),
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                },
            )
        )
    return rows, skipped


def reference_only_counts(psql: str, url: str) -> dict[str, dict]:
    """Counts rows in domains this format deliberately does not carry."""
    counts: dict[str, dict] = {}
    absent: list[str] = []
    for table, reason in REFERENCE_ONLY_TABLES.items():
        exists = run_query(psql, url, f"SELECT to_regclass('public.\"{table}\"')")
        if not exists or not exists[0] or exists[0][0].strip().lower() in {"", "null"}:
            absent.append(table)
            continue
        rows = run_query(psql, url, f'SELECT COUNT(*) FROM "{table}"')
        try:
            count = int(rows[0][0]) if rows else 0
        except (IndexError, ValueError):
            count = 0
        if count > 0:
            counts[table] = {"count": count, "reason": reason}
    if absent:
        counts["absentTables"] = {
            "count": len(absent),
            "reason": (
                "legacy tables not present in this snapshot (different C# version?): "
                + ", ".join(sorted(absent))
            ),
        }
    return counts


def source_fingerprint(psql: str, url: str) -> dict:
    """Non-sensitive source facts, hashed so no host or database name leaks."""
    rows = run_query(
        psql,
        url,
        "SELECT current_setting('server_version'), current_setting('server_version_num')",
    )
    facts = rows[0] if rows else ["unknown", "unknown"]
    digest = hashlib.sha256("\x1f".join(facts).encode("utf-8")).hexdigest()
    return {"fingerprint": digest, "facts": "redacted"}


def export_catalog_evidence(
    psql: str, url: str, streams: dict[str, list[Row]]
) -> list[Row]:
    """Keep the legacy identities and descriptive evidence needed after retiring C#."""
    ids: dict[str, set[UUID]] = {"artist": set(), "release": set(), "track": set()}
    for row in streams["entity-states.ndjson"]:
        ids[row.payload["entity"]].add(UUID(row.payload["entityPublicId"]))
    for stream in ("track-plays.ndjson", "playlist-items.ndjson", "bookmarks.ndjson"):
        ids["track"].update(UUID(row.payload["trackPublicId"]) for row in streams[stream])

    artist = """SELECT json_build_object('kind','artist','publicId',a."ApiKey",
        'artist',a."Name",'artistMbid',a."MusicBrainzId") FROM "Artists" a WHERE a."ApiKey" """
    release = """SELECT json_build_object('kind','release','publicId',r."ApiKey",
        'artist',a."Name",'release',r."Name",'releaseDate',r."ReleaseDate",
        'releaseMbid',r."MusicBrainzId",'artistMbid',a."MusicBrainzId")
        FROM "Albums" r JOIN "Artists" a ON a."Id"=r."ArtistId" WHERE r."ApiKey" """
    track = """SELECT json_build_object('kind','track','publicId',t."ApiKey",
        'artist',a."Name",'release',r."Name",'releaseDate',r."ReleaseDate",
        'title',t."Title",'trackNumber',t."SongNumber",'durationMs',round(t."Duration")::bigint,
        'fileSize',t."FileSize",'legacyFileHash',t."FileHash",'trackMbid',t."MusicBrainzId",
        'releaseMbid',r."MusicBrainzId",'artistMbid',a."MusicBrainzId")
        FROM "Songs" t JOIN "Albums" r ON r."Id"=t."AlbumId"
        JOIN "Artists" a ON a."Id"=r."ArtistId" WHERE t."ApiKey" """
    result = []
    for kind, query in (("artist", artist), ("release", release), ("track", track)):
        ordered = sorted(ids[kind])
        for start in range(0, len(ordered), 500):
            # UUID objects are validated above. Batching bounds both psql argv and the lookup.
            values = ",".join(f"'{key}'::uuid" for key in ordered[start : start + 500])
            rows = run_query(psql, url, f"{query} IN ({values})")
            records = (json.loads(raw) for (raw,) in rows)
            found = {record["publicId"]: record for record in records}
            # Keep references visible in the review backlog even if the source row is gone.
            for source_id in ordered[start : start + 500]:
                record = found.get(str(source_id), {"kind": kind, "publicId": str(source_id)})
                result.append(Row("catalog-evidence.ndjson", record))
    return sorted(result, key=lambda row: (row.payload["kind"], row.payload["publicId"]))


def write_stream(path: Path, rows: list[Row]) -> tuple[int, str]:
    digest = hashlib.sha256()
    count = 0
    with path.open("wb") as handle:
        for row in rows:
            line = (
                json.dumps(row.payload, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
                + "\n"
            ).encode("utf-8")
            handle.write(line)
            digest.update(line)
            count += 1
    return count, digest.hexdigest()


def progress(label: str, count: int) -> None:
    """Reports domain progress on stderr so stdout stays clean for --json."""
    print(f"  {label}: {count} rows", file=sys.stderr, flush=True)


def prepare_output(directory: Path, force: bool) -> None:
    if directory.exists():
        if not directory.is_dir():
            raise ExportError(f"{directory} exists and is not a directory")
        if any(directory.iterdir()):
            if not force:
                raise ExportError(
                    f"{directory} is not empty; pass --force to overwrite the export files"
                )
            for entry in directory.iterdir():
                if entry.is_file() and (
                    entry.name == "manifest.json" or entry.name in STREAMS
                ):
                    entry.unlink()
    else:
        directory.mkdir(parents=True)


def build_export(args: argparse.Namespace) -> dict:
    source_db = args.source_db or os.environ.get(DEFAULT_SOURCE_ENV)
    if not source_db:
        raise ExportError(f"pass --source-db or set {DEFAULT_SOURCE_ENV}")
    psql = psql_binary()
    SKIPS.clear()
    output = Path(args.output)
    prepare_output(output, args.force)

    users = export_users(psql, source_db, args.credentials)
    progress("users", len(users))
    states, unpinned = export_entity_states(psql, source_db)
    progress("entity states", len(states))
    plays, skipped_plays = export_track_plays(psql, source_db)
    progress("track plays", len(plays))
    playlists, playlist_items = export_playlists(psql, source_db)
    progress("playlists", len(playlists))
    progress("playlist items", len(playlist_items))
    smart_playlists, skipped_smart = export_smart_playlists(psql, source_db)
    progress("smart playlists", len(smart_playlists))
    bookmarks = export_bookmarks(psql, source_db)
    progress("bookmarks", len(bookmarks))
    groups, group_members, skipped_members = export_groups(psql, source_db)
    progress("groups", len(groups))
    progress("group members", len(group_members))
    playback = export_playback_settings(psql, source_db)
    progress("playback settings", len(playback))
    equalizer, skipped_equalizer = export_equalizer_presets(psql, source_db)
    progress("equalizer presets", len(equalizer))

    if args.limit:
        users = users[: args.limit]
        states = states[: args.limit]
        plays = plays[: args.limit]
        playlists = playlists[: args.limit]
        playlist_items = playlist_items[: args.limit]
        smart_playlists = smart_playlists[: args.limit]
        bookmarks = bookmarks[: args.limit]
        groups = groups[: args.limit]
        group_members = group_members[: args.limit]
        playback = playback[: args.limit]
        equalizer = equalizer[: args.limit]

    rows_by_stream: dict[str, list[Row]] = {name: [] for name in STREAMS}
    for collection in (
        users,
        states,
        plays,
        playlists,
        playlist_items,
        smart_playlists,
        bookmarks,
        groups,
        group_members,
        playback,
        equalizer,
    ):
        for row in collection:
            rows_by_stream[row.stream].append(row)

    evidence = export_catalog_evidence(psql, source_db, rows_by_stream)
    rows_by_stream["catalog-evidence.ndjson"] = evidence
    progress("referenced music", len(evidence))

    counts = {}
    checksums = {}
    for name in STREAMS:
        count, digest = write_stream(output / name, rows_by_stream[name])
        counts[name] = count
        checksums[name] = digest

    reference_only = reference_only_counts(psql, source_db)
    if unpinned:
        reference_only.setdefault(
            "UserPins.unresolved", {"count": unpinned, "reason": "pin target was not mapped"}
        )
    if skipped_smart:
        reference_only.setdefault(
            "SmartPlaylists.skipped",
            {"count": skipped_smart, "reason": "empty query or unsupported entity type"},
        )
    if skipped_equalizer:
        reference_only.setdefault(
            "UserEqualizerPresets.skipped",
            {"count": skipped_equalizer, "reason": "bands JSON was empty or invalid"},
        )
    if skipped_plays:
        reference_only.setdefault(
            "UserSongPlayHistories.skipped",
            {"count": skipped_plays, "reason": "play history row had no played-at time"},
        )
    if skipped_members:
        reference_only.setdefault(
            "UserGroupMembers.skipped",
            {"count": skipped_members, "reason": "membership row had no created-at time"},
        )

    for key, count in sorted(SKIPS.items()):
        reference_only.setdefault(
            f"invalid.{key}", {"count": count, "reason": "row cannot be represented"}
        )

    manifest = {
        "schemaVersion": SCHEMA_VERSION,
        "generator": GENERATOR,
        "generatedAt": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "source": source_fingerprint(psql, source_db),
        "counts": counts,
        "checksums": checksums,
        "referenceOnly": reference_only,
    }
    (output / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.splitlines()[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Reads the C# Melodee database strictly read-only and writes a "
            "versioned export directory for `melodee-cli users import-state`. "
            "The import is additive: it never deletes rows, older exports cannot "
            "downgrade newer local state, and playlist items are only appended, "
            "so the export can be regenerated and re-applied as the catalog grows."
        ),
    )
    parser.add_argument(
        "--source-db",
        default=None,
        help=f"legacy PostgreSQL URL (default: ${DEFAULT_SOURCE_ENV})",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="export directory to create or refresh",
    )
    parser.add_argument(
        "--credentials",
        action=argparse.BooleanOptionalAction,
        default=True,
        help=(
            "include portable bcrypt password hashes so created accounts keep "
            "their password (default: included; use --no-credentials to omit)"
        ),
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="overwrite existing export files in a non-empty output directory",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=0,
        help="write at most N rows per stream (rehearsal only; 0 = all rows)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the manifest summary as JSON instead of a table",
    )
    args = parser.parse_args(argv)
    if args.limit < 0:
        parser.error("--limit must be zero or positive")
    try:
        manifest = build_export(args)
    except ExportError as error:
        print(f"export failed: {error}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(manifest, indent=2, sort_keys=True))
    else:
        print(f"exported user state to {args.output}")
        for name in STREAMS:
            print(f"  {name}: {manifest['counts'][name]} rows")
        if manifest["referenceOnly"]:
            print("  reference-only domains (not exported):")
            for domain, entry in sorted(manifest["referenceOnly"].items()):
                print(f"    {domain}: {entry['count']} rows — {entry['reason']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
