#!/usr/bin/env python3
"""Reproducible lifecycle for the Phase 0 legacy (C#) compatibility oracle.

FIRST-10-ISSUES.md Issues 2 and 5. The documented recreation procedure in
``design/evidence/phase-0/legacy-baseline.md`` section 6 is authoritative; this
script automates it so no agent session depends on a previously running server.

Subcommands:

- ``status``    Report oracle checkout SHA vs the frozen baseline, DB migration
                head, fixture presence, and server liveness.
- ``build``     Incrementally build the legacy server and CLI (``dotnet build``).
- ``reset-db``  Drop/recreate the disposable capture database and apply EF
                migrations through the frozen head.
- ``seed``      Idempotently seed deterministic settings, the fixture user, the
                fixture catalog (``compatibility/fixtures/catalog-v1.json``),
                synthetic WAV media files, and an expired refresh token.
- ``start``     Launch ``server.dll`` with a scrubbed environment (``env -i``
                semantics) and deterministic fixture secrets.
- ``stop``      Stop a server started by this script.
- ``verify``    Probe system/info, OpenSubsonic ping, and fixture login.
- ``recreate``  check SHA -> build -> reset-db -> seed -> start -> verify.

All credentials here are synthetic, public, fixture-only values for a
disposable database. No production data is ever touched.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import json
import math
import os
import signal
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURE_CATALOG = ROOT / "compatibility" / "fixtures" / "catalog-v1.json"
FROZEN_COMMIT = "4028b58dbd3302ed431e2c5b32d9dde5634db218"
MIGRATION_HEAD = "20260714155936_AddDeleteSourceResidueAfterIngestSetting"

# Deterministic fixture UUID namespace. Every public API identifier in the
# fixture dataset derives from this namespace so reseeding always produces the
# same public IDs (PRD-API-M1-004 evidence).
FIXTURE_UUID_NAMESPACE = uuid.uuid5(uuid.NAMESPACE_URL, "https://melodee.dev/phase0-fixtures/v1")

# Synthetic, public, fixture-only credentials. These guard a disposable
# database full of generated WAV files; they are not secrets.
FIXTURE_USER = "phase0-fixture"
FIXTURE_EMAIL = "phase0-fixture@example.test"
FIXTURE_PASSWORD = "Mel0deeR0cks!"
FIXTURE_JWT_KEY = "phase0-fixture-only-jwt-signing-key-NOT-A-REAL-SECRET-0000000000000000"
FIXTURE_AUTH_TOKEN = "phase0-fixture-only-auth-token-NOT-A-REAL-SECRET-0000000000000000"
FIXTURE_SECURITY_KEY = "phase0-fixture-only-security-secret-key-NOT-A-REAL-SECRET-0000"
# Synthetic, public OpenSubsonic secret for the fixture user. Seeded through
# the same AES-GCM protector the server uses so OpenSubsonic token/salt auth
# can be exercised deterministically (the server generates a random secret at
# user creation; we overwrite it with this known fixture value).
FIXTURE_OPENSUBSONIC_SECRET = "phase0-opensubsonic-fixture-secret-not-a-real-secret"
# Fixed synthetic salt for OpenSubsonic token captures. The token
# md5(secret+salt) is computed at capture time and injected via environment —
# a usable credential is never written to disk or committed.
FIXTURE_SUBSONIC_SALT = "f1xture5a1t"
EXPIRED_REFRESH_TOKEN_RAW = "phase0-expired-refresh-token-fixture-not-a-real-secret"

WORK_DIR = Path(os.environ.get("PHASE0_WORK_DIR", "/tmp/legacy-oracle"))
LIBRARY_DIR = WORK_DIR / "library"
STORAGE_DIR = WORK_DIR / "storage"
PID_FILE = WORK_DIR / "server.pid"
LOG_FILE = WORK_DIR / "server.log"


def protect_secret(secret: str) -> str:
    """AES-GCM protector compatible with the legacy SecretProtector
    (v1:gcm:base64(nonce|tag|ciphertext), key = SHA256(security.secretKey)).
    Deterministic nonce: this protects a public fixture value, so reproducible
    output is a feature, not a weakness."""
    try:
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    except ImportError as error:
        raise SystemExit(
            "python package 'cryptography' is required for seeding the OpenSubsonic fixture secret"
        ) from error
    key = hashlib.sha256(FIXTURE_SECURITY_KEY.encode("utf-8")).digest()
    nonce = hashlib.sha256(b"phase0-fixture-nonce" + secret.encode("utf-8")).digest()[:12]
    aesgcm = AESGCM(key)
    ciphertext_and_tag = aesgcm.encrypt(nonce, secret.encode("utf-8"), None)
    combined = nonce + ciphertext_and_tag[-16:] + ciphertext_and_tag[:-16]
    return "v1:gcm:" + base64.b64encode(combined).decode("ascii")


def fixture_uuid(kind: str, slug: str) -> str:
    return str(uuid.uuid5(FIXTURE_UUID_NAMESPACE, f"{kind}:{slug}"))


def env_default(name: str, default: str) -> str:
    return os.environ.get(name, default)


class OracleConfig:
    def __init__(self, args: argparse.Namespace) -> None:
        self.legacy_root = Path(
            args.legacy_root or env_default("LEGACY_ROOT", str(ROOT.parent / "melodee"))
        ).resolve()
        self.db_host = env_default("PHASE0_DB_HOST", "127.0.0.1")
        self.db_port = env_default("PHASE0_DB_PORT", "5433")
        self.db_name = env_default("PHASE0_DB_NAME", "melodee_phase0")
        self.db_user = env_default("PHASE0_DB_USER", "melodee")
        self.db_password = env_default("PHASE0_DB_PASSWORD", "melodee-dev")
        self.base_url = env_default("LEGACY_MELODEE_BASE_URL", "http://127.0.0.1:5157").rstrip("/")

    @property
    def connection_string(self) -> str:
        return (
            f"Host={self.db_host};Port={self.db_port};Database={self.db_name};"
            f"Username={self.db_user};Password={self.db_password};SSL Mode=Disable"
        )

    @property
    def server_dir(self) -> Path:
        return self.legacy_root / "src" / "Melodee.Blazor" / "bin" / "Debug" / "net10.0"

    @property
    def cli_dir(self) -> Path:
        return self.legacy_root / "src" / "Melodee.Cli" / "bin" / "Debug" / "net10.0"


def run(cmd: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None,
        check: bool = True, capture: bool = False) -> subprocess.CompletedProcess:
    result = subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        check=False,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.STDOUT if capture else None,
    )
    if check and result.returncode != 0:
        output = f"\n{result.stdout}" if capture and result.stdout else ""
        raise SystemExit(f"command failed ({result.returncode}): {' '.join(cmd)}{output}")
    return result


def psql(config: OracleConfig, sql: str, *, database: str | None = None) -> subprocess.CompletedProcess:
    env = {**os.environ, "PGPASSWORD": config.db_password}
    return run(
        [
            "psql", "-h", config.db_host, "-p", config.db_port, "-U", config.db_user,
            "-d", database or config.db_name, "-v", "ON_ERROR_STOP=1", "-c", sql,
        ],
        env=env,
        capture=True,
    )


def sql_literal(value: object) -> str:
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return repr(value)
    if isinstance(value, list):
        return "ARRAY[" + ",".join(sql_literal(item) for item in value) + "]::text[]"
    return "'" + str(value).replace("'", "''") + "'"


# ---------------------------------------------------------------------------
# status
# ---------------------------------------------------------------------------

def oracle_sha(config: OracleConfig) -> str:
    result = run(
        ["git", "rev-parse", "HEAD"], cwd=config.legacy_root, capture=True, check=False
    )
    return (result.stdout or "").strip() if result.returncode == 0 else "<unknown>"


def server_alive(config: OracleConfig) -> bool:
    try:
        with urllib.request.urlopen(f"{config.base_url}/api/v1/system/info", timeout=3) as response:
            return response.status == 200
    except (urllib.error.URLError, urllib.error.HTTPError, OSError):
        return False


def cmd_status(config: OracleConfig) -> None:
    sha = oracle_sha(config)
    print(f"oracle checkout: {config.legacy_root}")
    print(f"oracle SHA:      {sha}")
    print(f"frozen SHA:      {FROZEN_COMMIT} ({'MATCH' if sha == FROZEN_COMMIT else 'MISMATCH — stop and reconcile before capturing'})")
    print(f"server.dll:      {'present' if (config.server_dir / 'server.dll').exists() else 'missing (run build)'}")
    print(f"server alive:    {server_alive(config)} ({config.base_url})")
    result = psql(
        config,
        "SELECT COUNT(*) FROM \"__EFMigrationsHistory\" WHERE \"MigrationId\" = "
        f"'{MIGRATION_HEAD}'",
    )
    applied = any(line.strip() == "1" for line in (result.stdout or "").splitlines())
    print(f"migration head:  {'applied' if applied else 'unknown/missing'}")
    if sha != FROZEN_COMMIT:
        raise SystemExit(2)


# ---------------------------------------------------------------------------
# build / reset-db
# ---------------------------------------------------------------------------

def scrubbed_env(extra: dict[str, str]) -> dict[str, str]:
    """env -i semantics: only the variables the oracle needs, never the
    developer shell's secrets (MelodeeConfigurationFactory imports *every*
    process environment variable)."""
    env = {
        "HOME": os.environ.get("HOME", "/tmp"),
        "PATH": "/usr/local/bin:/usr/bin:/bin:" + str(Path.home() / ".dotnet" / "tools"),
        "DOTNET_CLI_TELEMETRY_OPTOUT": "1",
    }
    env.update(extra)
    return env


def cmd_build(config: OracleConfig) -> None:
    run(
        ["dotnet", "build", "src/Melodee.Blazor/Melodee.Blazor.csproj", "--nologo", "-v", "q"],
        cwd=config.legacy_root,
        env=scrubbed_env({}),
    )
    run(
        ["dotnet", "build", "src/Melodee.Cli/Melodee.Cli.csproj", "--nologo", "-v", "q"],
        cwd=config.legacy_root,
        env=scrubbed_env({}),
    )
    wwwroot = config.server_dir / "wwwroot"
    if not wwwroot.exists():
        wwwroot.symlink_to(config.legacy_root / "src" / "Melodee.Blazor" / "wwwroot")
    print("build complete")


def cmd_reset_db(config: OracleConfig) -> None:
    env = {**os.environ, "PGPASSWORD": config.db_password}
    run(
        ["psql", "-h", config.db_host, "-p", config.db_port, "-U", config.db_user,
         "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-c",
         f"DROP DATABASE IF EXISTS {config.db_name} WITH (FORCE)"],
        env=env,
    )
    run(
        ["psql", "-h", config.db_host, "-p", config.db_port, "-U", config.db_user,
         "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-c", f"CREATE DATABASE {config.db_name}"],
        env=env,
    )
    run(
        [
            "dotnet", "ef", "database", "update",
            "--project", "src/Melodee.Common/Melodee.Common.csproj",
            "--startup-project", "src/Melodee.Blazor/Melodee.Blazor.csproj",
            "--context", "MelodeeDbContext",
            "--connection", config.connection_string,
        ],
        cwd=config.legacy_root,
        env=scrubbed_env({}),
    )
    print("database recreated and migrated")


# ---------------------------------------------------------------------------
# seed
# ---------------------------------------------------------------------------

def make_png(path: Path, *, size: int, rgb: tuple[int, int, int]) -> None:
    """Deterministic minimal RGB PNG (no external deps)."""
    import zlib

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = b""
    for _ in range(size):
        raw += b"\x00" + bytes(rgb) * size
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def make_wav(path: Path, *, seconds: float, frequency: float) -> None:
    """Deterministic PCM WAV (16-bit mono 44100 Hz sine)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    rate = 44100
    frames = int(rate * seconds)
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        samples = bytearray()
        for index in range(frames):
            value = int(12000 * math.sin(2 * math.pi * frequency * index / rate))
            samples += struct.pack("<h", value)
        wav.writeframes(bytes(samples))


FIXTURE_IMAGE_SPECS = [
    # (artist slug, album slug or None, file name, rgb) — one artist and one
    # album carry real artwork; every other entity exercises the no-image path.
    ("aurora-fields", None, "artist.png", (46, 82, 130)),
    ("aurora-fields", "northern-lights", "cover.png", (130, 46, 82)),
]


def seed_media_files(catalog: dict) -> dict[str, tuple[int, str]]:
    """Write WAV files; return {song_slug: (file_size, sha256_hex)}."""
    results: dict[str, tuple[int, str]] = {}
    for artist in catalog["artists"]:
        for album in artist["albums"]:
            for song in album["songs"]:
                relative = Path(artist["directory"]) / album["directory"] / song["fileName"]
                target = LIBRARY_DIR / relative
                make_wav(target, seconds=song["mediaSeconds"], frequency=song["mediaFrequency"])
                data = target.read_bytes()
                results[song["slug"]] = (len(data), hashlib.sha256(data).hexdigest())
    artists_by_slug = {a["slug"]: a for a in catalog["artists"]}
    for artist_slug, album_slug, file_name, rgb in FIXTURE_IMAGE_SPECS:
        artist = artists_by_slug[artist_slug]
        directory = Path(artist["directory"])
        if album_slug:
            album = next(a for a in artist["albums"] if a["slug"] == album_slug)
            directory = directory / album["directory"]
        make_png(LIBRARY_DIR / directory / file_name, size=64, rgb=rgb)
    # Synchronized-lyrics sidecar for the Jellyfin /Audio/{id}/Lyrics route.
    aurora = artists_by_slug["aurora-fields"]
    northern = aurora["albums"][0]
    lrc = (
        LIBRARY_DIR / aurora["directory"] / northern["directory"] / "02 - Polar Veil.lrc"
    )
    lrc.write_text(
        "[00:00.00] Synthetic lyric line one\n[00:01.00] Synthetic lyric line two\n",
        encoding="utf-8",
    )
    return results


def seed_sql(
    catalog: dict,
    media: dict[str, tuple[int, str]],
    *,
    include_native_share: bool = True,
) -> str:
    now = "TIMESTAMPTZ '2026-08-07T00:00:00Z'"
    statements: list[str] = ["BEGIN;"]

    # Wipe any prior fixture dataset (deterministic slugs -> deterministic UUIDs).
    fixture_library_key = fixture_uuid("library", catalog["library"]["slug"])
    fixture_keys = {
        "Contributors": [fixture_uuid("contributor", c["slug"]) for c in catalog.get("contributors", [])],
        "ArtistRelation": [fixture_uuid("artist-relation", r["slug"]) for r in catalog.get("artistRelations", [])],
        "UserSongs": [fixture_uuid("user-song", r["song"]) for r in catalog.get("userSongs", [])],
        "UserAlbums": [fixture_uuid("user-album", r["album"]) for r in catalog.get("userAlbums", [])],
        "UserArtists": [fixture_uuid("user-artist", r["artist"]) for r in catalog.get("userArtists", [])],
        "RefreshTokens": [fixture_uuid("refresh-token", "expired")],
    }
    for table, keys in fixture_keys.items():
        if keys:
            statements.append(
                f'DELETE FROM "{table}" WHERE "ApiKey" IN ('
                + ",".join(sql_literal(key) for key in keys)
                + ");"
            )
    # Playlists first: Playlists.SongId references Songs without ON DELETE CASCADE.
    # Wipe ALL playlists: scenario-created ones (native/OS/JF CRUD flows) must
    # not leak across capture runs; fixture playlists are reinserted below.
    statements.append('DELETE FROM "Playlists";')
    # Scenario-created entities must not leak across capture runs.
    statements.append('DELETE FROM "RadioStations";')
    statements.append('DELETE FROM "Shares";')
    statements.append('DELETE FROM "Bookmarks";')
    statements.append('DELETE FROM "PlayQues";')
    statements.append('DELETE FROM "RequestComments";')
    statements.append('DELETE FROM "RequestParticipants";')
    statements.append('DELETE FROM "RequestUserStates";')
    statements.append('DELETE FROM "Requests";')
    statements.append('DELETE FROM "UserEqualizerPresets";')
    statements.append('DELETE FROM "UserPlaybackSettings";')
    # Non-fixture users (scenario-created, e.g. OpenSubsonic createUser) must
    # not leak either; user-dependent rows above are already wiped.
    statements.append(
        f'DELETE FROM "RefreshTokens" WHERE "UserId" NOT IN (SELECT "Id" FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)});'
    )
    statements.append(
        f'DELETE FROM "Users" WHERE "UserName" <> {sql_literal(FIXTURE_USER)};'
    )
    statements.append(
        f'DELETE FROM "Libraries" WHERE "ApiKey" = {sql_literal(fixture_library_key)};'
    )

    statements.append(
        'INSERT INTO "Libraries" ("Name","Path","Type","IsLocked","SortOrder","ApiKey","CreatedAt") '
        f"VALUES ({sql_literal(catalog['library']['name'])},{sql_literal(str(LIBRARY_DIR))},3,"
        f"false,0,{sql_literal(fixture_library_key)},{now});"
    )
    artist_ids: dict[str, int] = {}
    album_ids: dict[str, int] = {}
    song_ids: dict[str, int] = {}
    id_counter = 100000
    for artist in catalog["artists"]:
        artist_ids[artist["slug"]] = id_counter
        id_counter += 1
        for album in artist["albums"]:
            album_ids[album["slug"]] = id_counter
            id_counter += 1
            for song in album["songs"]:
                song_ids[song["slug"]] = id_counter
                id_counter += 1

    statements.append("SET session_replication_role = replica;")  # explicit id inserts

    for artist in catalog["artists"]:
        albums = artist["albums"]
        song_total = sum(len(a["songs"]) for a in albums)
        statements.append(
            'INSERT INTO "Artists" ("Id","Name","NameNormalized","SortName","RealName","Directory",'
            '"AlbumCount","SongCount","LibraryId","MetaDataStatus","IsLocked","SortOrder","ApiKey",'
            '"CreatedAt","PlayedCount","CalculatedRating","Biography","Tags","AlternateNames","LastPlayedAt") VALUES ('
            + ",".join(
                [
                    str(artist_ids[artist["slug"]]),
                    sql_literal(artist["name"]),
                    sql_literal(artist["nameNormalized"]),
                    sql_literal(artist.get("sortName")),
                    sql_literal(artist.get("realName")),
                    sql_literal(artist["directory"]),
                    str(len(albums)),
                    str(song_total),
                    f'(SELECT "Id" FROM "Libraries" WHERE "ApiKey" = {sql_literal(fixture_library_key)})',
                    "2",  # MetaDataStatus: treated as resolved fixture metadata
                    "false",
                    "0",
                    sql_literal(fixture_uuid("artist", artist["slug"])),
                    sql_literal(artist.get("createdAt", "2026-08-07T00:00:00Z")),
                    str(artist.get("playedCount", 0)),
                    str(artist.get("calculatedRating", 0)),
                    sql_literal(artist.get("biography")),
                    sql_literal(artist.get("tags")),
                    sql_literal(artist.get("alternateNames")),
                    sql_literal(artist.get("lastPlayedAt")),
                ]
            )
            + ");"
        )

    for artist in catalog["artists"]:
        for album in artist["albums"]:
            duration = sum(s["durationMs"] for s in album["songs"])
            statements.append(
                'INSERT INTO "Albums" ("Id","ArtistId","Name","NameNormalized","SortName","AlbumStatus",'
                '"MetaDataStatus","AlbumType","OriginalReleaseDate","ReleaseDate","IsCompilation",'
                '"SongCount","Duration","Genres","Directory","IsLocked","SortOrder","ApiKey","CreatedAt",'
                '"PlayedCount","CalculatedRating","Tags","Comment","Description","LastPlayedAt") VALUES ('
                + ",".join(
                    [
                        str(album_ids[album["slug"]]),
                        str(artist_ids[artist["slug"]]),
                        sql_literal(album["name"]),
                        sql_literal(album["nameNormalized"]),
                        sql_literal(album.get("sortName")),
                        str(album.get("albumStatus", 1)),
                        "2",
                        str(album.get("albumType", 0)),
                        sql_literal(album.get("originalReleaseDate")),
                        sql_literal(album["releaseDate"]),
                        "true" if album.get("isCompilation") else "false",
                        str(len(album["songs"])),
                        repr(float(duration)),
                        sql_literal(album.get("genres")),
                        sql_literal(album["directory"]),
                        "false",
                        "0",
                        sql_literal(fixture_uuid("album", album["slug"])),
                        sql_literal(album.get("createdAt", "2026-08-07T00:00:00Z")),
                        str(album.get("playedCount", 0)),
                        str(album.get("calculatedRating", 0)),
                        sql_literal(album.get("tags")),
                        sql_literal(album.get("comment")),
                        sql_literal(album.get("description")),
                        sql_literal(album.get("lastPlayedAt")),
                    ]
                )
                + ");"
            )

    song_counter = 0
    for artist in catalog["artists"]:
        for album in artist["albums"]:
            for song in album["songs"]:
                song_counter += 1
                # Stagger CreatedAt: global song listings order by CreatedAt and
                # the legacy server has no tiebreak for equal timestamps.
                song_created = f"2026-01-01T00:{song_counter % 60:02d}:{song_counter % 60:02d}Z"
                size, sha = media[song["slug"]]
                statements.append(
                    'INSERT INTO "Songs" ("Id","AlbumId","Title","TitleSort","TitleNormalized","Genres",'
                    '"SongNumber","FileName","FileSize","FileHash","PartTitles","Duration","SamplingRate",'
                    '"BitRate","BitDepth","BPM","ContentType","IsVbr","IsLocked","SortOrder","ApiKey",'
                    '"CreatedAt","PlayedCount","CalculatedRating","Comment","Lyrics") VALUES ('
                    + ",".join(
                        [
                            str(song_ids[song["slug"]]),
                            str(album_ids[album["slug"]]),
                            sql_literal(song["title"]),
                            sql_literal(song.get("titleSort")),
                            sql_literal(song["titleNormalized"]),
                            sql_literal(song.get("genres", album.get("genres"))),
                            str(song["songNumber"]),
                            sql_literal(song["fileName"]),
                            str(size),
                            sql_literal(sha),
                            sql_literal(song.get("partTitles")),
                            repr(float(song["durationMs"])),
                            "44100",
                            str(song.get("bitRate", 1411)),
                            "16",
                            str(song.get("bpm", 0)),
                            sql_literal("audio/wav"),
                            "false",
                            "false",
                            "0",
                            sql_literal(fixture_uuid("song", song["slug"])),
                            sql_literal(song.get("createdAt", song_created)),
                            str(song.get("playedCount", 0)),
                            str(song.get("calculatedRating", 0)),
                            sql_literal(song.get("comment")),
                            sql_literal(song.get("lyrics")),
                        ]
                    )
                    + ");"
                )

    for relation in catalog.get("artistRelations", []):
        statements.append(
            'INSERT INTO "ArtistRelation" ("ArtistId","RelatedArtistId","ArtistRelationType","IsLocked",'
            '"SortOrder","ApiKey","CreatedAt") VALUES ('
            + ",".join(
                [
                    str(artist_ids[relation["artist"]]),
                    str(artist_ids[relation["relatedArtist"]]),
                    str(relation["type"]),
                    "false",
                    "0",
                    sql_literal(fixture_uuid("artist-relation", relation["slug"])),
                    now,
                ]
            )
            + ");"
        )

    for contributor in catalog.get("contributors", []):
        statements.append(
            'INSERT INTO "Contributors" ("Role","SubRole","ArtistId","ContributorName","MetaTagIdentifier",'
            '"SongId","AlbumId","ContributorType","IsLocked","SortOrder","ApiKey","CreatedAt") VALUES ('
            + ",".join(
                [
                    sql_literal(contributor["role"]),
                    sql_literal(contributor.get("subRole")),
                    str(artist_ids[contributor["artist"]]) if contributor.get("artist") else "NULL",
                    sql_literal(contributor.get("contributorName")),
                    str(contributor["metaTagIdentifier"]),
                    str(song_ids[contributor["song"]]) if contributor.get("song") else "NULL",
                    str(album_ids[contributor["album"]]),
                    str(contributor.get("contributorType", 1)),
                    "false",
                    "0",
                    sql_literal(fixture_uuid("contributor", contributor["slug"])),
                    now,
                ]
            )
            + ");"
        )

    # User library state (stars / hates / ratings / play counts).
    for row in catalog.get("userSongs", []):
        statements.append(
            'INSERT INTO "UserSongs" ("UserId","SongId","PlayedCount","IsStarred","IsHated","StarredAt",'
            '"Rating","IsLocked","SortOrder","ApiKey","CreatedAt","LastPlayedAt") VALUES ('
            + ",".join(
                [
                    f'(SELECT "Id" FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)})',
                    str(song_ids[row["song"]]),
                    str(row.get("playedCount", 0)),
                    "true" if row.get("isStarred") else "false",
                    "true" if row.get("isHated") else "false",
                    now if row.get("isStarred") else "NULL",
                    str(row.get("rating", 0)),
                    "false",
                    "0",
                    sql_literal(fixture_uuid("user-song", row["song"])),
                    now,
                    now if row.get("playedCount") else "NULL",
                ]
            )
            + ");"
        )
    for row in catalog.get("userAlbums", []):
        statements.append(
            'INSERT INTO "UserAlbums" ("UserId","AlbumId","PlayedCount","IsStarred","IsHated","StarredAt",'
            '"Rating","IsLocked","SortOrder","ApiKey","CreatedAt") VALUES ('
            + ",".join(
                [
                    f'(SELECT "Id" FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)})',
                    str(album_ids[row["album"]]),
                    str(row.get("playedCount", 0)),
                    "true" if row.get("isStarred") else "false",
                    "true" if row.get("isHated") else "false",
                    now if row.get("isStarred") else "NULL",
                    str(row.get("rating", 0)),
                    "false",
                    "0",
                    sql_literal(fixture_uuid("user-album", row["album"])),
                    now,
                ]
            )
            + ");"
        )
    for row in catalog.get("userArtists", []):
        statements.append(
            'INSERT INTO "UserArtists" ("UserId","ArtistId","IsStarred","IsHated","StarredAt",'
            '"Rating","IsLocked","SortOrder","ApiKey","CreatedAt") VALUES ('
            + ",".join(
                [
                    f'(SELECT "Id" FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)})',
                    str(artist_ids[row["artist"]]),
                    "true" if row.get("isStarred") else "false",
                    "true" if row.get("isHated") else "false",
                    now if row.get("isStarred") else "NULL",
                    str(row.get("rating", 0)),
                    "false",
                    "0",
                    sql_literal(fixture_uuid("user-artist", row["artist"])),
                    now,
                ]
            )
            + ");"
        )

    # Playlists spanning multiple Releases. Stagger CreatedAt so list ordering
    # is deterministic (the legacy server has no tiebreak for equal timestamps).
    playlist_day = 1
    for playlist in catalog["playlists"]:
        songs = playlist["songs"]
        duration = 0.0
        for slug in songs:
            for artist in catalog["artists"]:
                for album in artist["albums"]:
                    for song in album["songs"]:
                        if song["slug"] == slug:
                            duration += song["durationMs"]
        statements.append(
            'INSERT INTO "Playlists" ("Name","Comment","UserId","IsPublic","SongCount","Duration","SongId",'
            '"IsLocked","SortOrder","ApiKey","CreatedAt","Description") VALUES ('
            + ",".join(
                [
                    sql_literal(playlist["name"]),
                    sql_literal(playlist.get("comment")),
                    f'(SELECT "Id" FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)})',
                    "true" if playlist.get("isPublic") else "false",
                    str(len(songs)),
                    repr(duration),
                    str(song_ids[songs[0]]) if songs else "NULL",
                    "false",
                    "0",
                    sql_literal(fixture_uuid("playlist", playlist["slug"])),
                    sql_literal(playlist.get("createdAt", f"2026-03-{playlist_day:02d}T00:00:00Z")),
                    sql_literal(playlist.get("description")),
                ]
            )
            + ");"
        )
        playlist_day += 1
        for order, slug in enumerate(songs, start=1):
            statements.append(
                'INSERT INTO "PlaylistSong" ("SongId","PlaylistId","SongApiKey","PlaylistOrder") VALUES ('
                + ",".join(
                    [
                        str(song_ids[slug]),
                        f'(SELECT "Id" FROM "Playlists" WHERE "ApiKey" = '
                        f'{sql_literal(fixture_uuid("playlist", playlist["slug"]))})',
                        sql_literal(fixture_uuid("song", slug)),
                        str(order),
                    ]
                )
                + ");"
            )

    if include_native_share:
        # Stable anonymous native-v1 share fixture. The public short ID is
        # part of the external contract and intentionally remains
        # human-readable here so the unauthenticated route can be captured
        # without leaking a generated ID from a prior request step. The
        # OpenSubsonic and Jellyfin capture flows do not use this native-only
        # setup; omitting it keeps their clean-target share state aligned with
        # their independently captured goldens.
        statements.append(
            'INSERT INTO "Shares" ("UserId","ShareId","ShareType","ShareUniqueId","IsDownloadable",'
            '"VisitCount","IsLocked","SortOrder","ApiKey","CreatedAt","Description") VALUES ('
            + ",".join(
                [
                    f'(SELECT "Id" FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)})',
                    str(song_ids["nl-02-polarveil"]),
                    "1",
                    sql_literal("phase7-public-song"),
                    "false",
                    "0",
                    "false",
                    "0",
                    sql_literal(fixture_uuid("share", "public-song")),
                    now,
                    sql_literal("Phase 7 public share fixture"),
                ]
            )
            + ");"
        )

    statements.append("SET session_replication_role = DEFAULT;")

    # An expired refresh token for negative-path capture. HashedToken is
    # base64(SHA256(utf8(raw))) per RefreshTokenService.HashToken.
    expired_hash = base64.b64encode(
        hashlib.sha256(EXPIRED_REFRESH_TOKEN_RAW.encode("utf-8")).digest()
    ).decode("ascii")
    statements.append(
        'INSERT INTO "RefreshTokens" ("UserId","HashedToken","TokenFamily","IssuedAt","ExpiresAt",'
        '"SessionStartedAt","DeviceId","IsLocked","SortOrder","ApiKey","CreatedAt") VALUES ('
        + ",".join(
            [
                f'(SELECT "Id" FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)})',
                sql_literal(expired_hash),
                sql_literal("phase0expiredfamily00000000000000"),
                "TIMESTAMPTZ '2026-07-01T00:00:00Z'",
                "TIMESTAMPTZ '2026-07-02T00:00:00Z'",
                "TIMESTAMPTZ '2026-07-01T00:00:00Z'",
                sql_literal("phase0-fixture-device"),
                "false",
                "0",
                sql_literal(fixture_uuid("refresh-token", "expired")),
                now,
            ]
        )
        + ");"
    )

    statements.append("COMMIT;")
    return "\n".join(statements)


def cmd_seed(config: OracleConfig, *, include_native_share: bool = True) -> None:
    catalog = json.loads(FIXTURE_CATALOG.read_text(encoding="utf-8"))

    # Deterministic canonical baseUrl (never the legacy placeholder) and
    # completed onboarding so unauthenticated API requests are not redirected.
    psql(
        config,
        "INSERT INTO \"Settings\" (\"ApiKey\",\"Category\",\"CreatedAt\",\"Description\",\"IsLocked\",\"Key\",\"LastUpdatedAt\",\"SortOrder\",\"Value\")"
        " VALUES (gen_random_uuid(), 0, now(), 'Phase 0 canonical capture base URL', false,"
        f" 'system.baseUrl', now(), 0, {sql_literal(config.base_url)})"
        " ON CONFLICT (\"Key\") DO UPDATE SET \"Value\" = EXCLUDED.\"Value\";",
    )
    psql(
        config,
        "INSERT INTO \"Settings\" (\"ApiKey\",\"Category\",\"CreatedAt\",\"Description\",\"IsLocked\",\"Key\",\"LastUpdatedAt\",\"SortOrder\",\"Value\")"
        " VALUES (gen_random_uuid(), 0, now(), 'Phase 0 capture fixture', false,"
        " 'system.onboardingCompletedAt', now(), 0, '2026-08-07T00:00:00.0000000Z')"
        " ON CONFLICT (\"Key\") DO UPDATE SET \"Value\" = EXCLUDED.\"Value\";",
    )

    # Fixture user via the legacy CLI (correct password hashing), then pin the
    # public ApiKey to a deterministic fixture UUID so golden fixtures show
    # stable public identifiers.
    existing = psql(
        config, f'SELECT COUNT(*) FROM "Users" WHERE "UserName" = {sql_literal(FIXTURE_USER)}'
    )
    if "1" not in (existing.stdout or "").split():
        run(
            [
                "dotnet", "mcli.dll", "user", "create",
                "-u", FIXTURE_USER, "-e", FIXTURE_EMAIL, "-p", FIXTURE_PASSWORD, "-f",
            ],
            cwd=config.cli_dir,
            env=scrubbed_env(
                {
                    "ConnectionStrings__DefaultConnection": config.connection_string,
                    "ConnectionStrings__MusicBrainzConnection": f"Data Source={STORAGE_DIR}/_search-engines/musicbrainz/musicbrainz.ddb;",
                    "ConnectionStrings__ArtistSearchEngineConnection": f"Data Source={STORAGE_DIR}/_search-engines/artistSearchEngine.db;Cache=Shared",
                    "security_secretKey": FIXTURE_SECURITY_KEY,
                }
            ),
        )
    psql(
        config,
        f'UPDATE "Users" SET "ApiKey" = {sql_literal(fixture_uuid("user", FIXTURE_USER))}, '
        f'"IsScrobblingEnabled" = true, '
        f'"OpenSubsonicSecretProtected" = {sql_literal(protect_secret(FIXTURE_OPENSUBSONIC_SECRET))} '
        f'WHERE "UserName" = {sql_literal(FIXTURE_USER)};',
    )

    LIBRARY_DIR.mkdir(parents=True, exist_ok=True)
    media = seed_media_files(catalog)
    sql = seed_sql(catalog, media, include_native_share=include_native_share)
    sql_file = WORK_DIR / "seed.sql"
    sql_file.parent.mkdir(parents=True, exist_ok=True)
    sql_file.write_text(sql, encoding="utf-8")
    env = {**os.environ, "PGPASSWORD": config.db_password}
    run(
        ["psql", "-h", config.db_host, "-p", config.db_port, "-U", config.db_user,
         "-d", config.db_name, "-v", "ON_ERROR_STOP=1", "-q", "-f", str(sql_file)],
        env=env,
    )
    counts = psql(
        config,
        "SELECT (SELECT COUNT(*) FROM \"Artists\") || ' artists / ' || "
        "(SELECT COUNT(*) FROM \"Albums\") || ' albums / ' || "
        "(SELECT COUNT(*) FROM \"Songs\") || ' songs / ' || "
        "(SELECT COUNT(*) FROM \"Playlists\") || ' playlists'",
    )
    line = next(
        (l.strip() for l in (counts.stdout or "").splitlines() if "artists" in l), "counts unavailable"
    )
    print(f"seed complete: {line}")


# ---------------------------------------------------------------------------
# start / stop / verify
# ---------------------------------------------------------------------------

def cmd_start(config: OracleConfig) -> None:
    if PID_FILE.exists():
        pid = int(PID_FILE.read_text().strip())
        try:
            os.kill(pid, 0)
            print(f"server already running (pid {pid})")
            return
        except OSError:
            PID_FILE.unlink()

    WORK_DIR.mkdir(parents=True, exist_ok=True)
    env = scrubbed_env(
        {
            "ASPNETCORE_ENVIRONMENT": "Production",
            "ASPNETCORE_URLS": config.base_url,
            "QuartzDisabled": "true",
            # Allows capture tooling to present non-local client IPs via
            # X-Forwarded-For so auth-enforced (non-bypassed) OpenSubsonic and
            # IP-bound behavior can be captured from the same machine.
            "UseForwardedHeaders": "true",
            "ConnectionStrings__DefaultConnection": config.connection_string,
            "ConnectionStrings__MusicBrainzConnection": f"Data Source={STORAGE_DIR}/_search-engines/musicbrainz/musicbrainz.ddb;",
            "ConnectionStrings__ArtistSearchEngineConnection": f"Data Source={STORAGE_DIR}/_search-engines/artistSearchEngine.db;Cache=Shared",
            "MelodeeAuthSettings__Token": FIXTURE_AUTH_TOKEN,
            "Jwt__Key": FIXTURE_JWT_KEY,
            "Jwt__Issuer": "MelodeeApi",
            "Jwt__Audience": "MelodeeClient",
            "security_secretKey": FIXTURE_SECURITY_KEY,
        }
    )
    log_handle = LOG_FILE.open("ab")
    process = subprocess.Popen(
        ["dotnet", "server.dll"],
        cwd=config.server_dir,
        env=env,
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    PID_FILE.write_text(str(process.pid))
    deadline = time.time() + 90
    while time.time() < deadline:
        if process.poll() is not None:
            tail = LOG_FILE.read_bytes()[-4000:].decode("utf-8", "replace")
            raise SystemExit(f"server exited during startup; log tail:\n{tail}")
        if server_alive(config):
            print(f"server running (pid {process.pid}) at {config.base_url}")
            return
        time.sleep(1)
    raise SystemExit("server did not become ready within 90s; see " + str(LOG_FILE))


def cmd_stop(config: OracleConfig) -> None:
    if not PID_FILE.exists():
        print("no pid file; nothing to stop")
        return
    pid = int(PID_FILE.read_text().strip())
    try:
        os.killpg(pid, signal.SIGTERM)
    except (OSError, ProcessLookupError):
        print(f"process {pid} not running")
        PID_FILE.unlink(missing_ok=True)
        return
    # Wait for the process group to actually exit — otherwise a subsequent
    # start's readiness probe can pass against the dying instance and the
    # capture then races its shutdown (Issue 6 pipeline discovery).
    # The legacy server routinely takes >10s to stop (or ignores SIGTERM in
    # this configuration); escalate quickly to keep capture cycles fast.
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
        except OSError:
            PID_FILE.unlink(missing_ok=True)
            print(f"stopped server process group {pid}")
            return
        time.sleep(0.5)
    try:
        os.killpg(pid, signal.SIGKILL)
        print(f"force-killed server process group {pid}")
    except (OSError, ProcessLookupError):
        pass
    PID_FILE.unlink(missing_ok=True)


def cmd_verify(config: OracleConfig) -> None:
    failures: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        print(f"{'PASS' if ok else 'FAIL'} {name} {detail}")
        if not ok:
            failures.append(name)

    try:
        with urllib.request.urlopen(f"{config.base_url}/api/v1/system/info", timeout=10) as response:
            info = json.loads(response.read())
        check(
            "system/info",
            info.get("majorVersion") == 2 and info.get("minorVersion") == 2,
            f"version={info.get('version')}",
        )
    except Exception as error:  # noqa: BLE001
        check("system/info", False, str(error))

    try:
        with urllib.request.urlopen(
            f"{config.base_url}/rest/ping.view?f=json&v=1.16.1&c=phase0-verify", timeout=10
        ) as response:
            ping = json.loads(response.read())
        check("opensubsonic-ping", "subsonic-response" in ping)
    except Exception as error:  # noqa: BLE001
        check("opensubsonic-ping", False, str(error))

    try:
        request = urllib.request.Request(
            f"{config.base_url}/api/v1/auth/authenticate",
            data=json.dumps({"userName": FIXTURE_USER, "password": FIXTURE_PASSWORD}).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            auth = json.loads(response.read())
        token = auth.get("token", "")
        check(
            "fixture-login",
            response.status == 200 and token.count(".") == 2,
            "jwt structure ok" if token.count(".") == 2 else "unexpected token shape",
        )
        base_url_seen = auth.get("user", {}).get("imageUrl", "")
        check(
            "baseUrl-not-placeholder",
            "REQUIRED: THIS MUST BE EDITED" not in json.dumps(auth),
            f"imageUrl={base_url_seen}",
        )
    except Exception as error:  # noqa: BLE001
        check("fixture-login", False, str(error))

    try:
        with urllib.request.urlopen(
            f"{config.base_url}/api/v1/artists?page=1&pageSize=5",
            timeout=10,
        ) as response:
            pass
    except urllib.error.HTTPError as error:
        # 401 here means routing works and auth is enforced; a redirect or 404
        # means onboarding/routing regressed.
        check("artists-requires-auth", error.code == 401, f"status={error.code}")
    except Exception as error:  # noqa: BLE001
        check("artists-requires-auth", False, str(error))

    if failures:
        raise SystemExit(f"{len(failures)} verification checks failed")


SCENARIO_FILES = [
    "public.json",
    "native-auth.json",
    "native-auth-additional.json",
    "native-catalog.json",
    "native-discovery.json",
    "native-excluded.json",
    "native-stream.json",
    "native-scrobble-extra.json",
    "native-user-extra.json",
    "native-user-providers.json",
    "native-community.json",
    "native-podcasts.json",
    "opensubsonic-system.json",
    "opensubsonic-auth.json",
    "opensubsonic-browse.json",
    "opensubsonic-lists.json",
    "opensubsonic-media.json",
    "opensubsonic-annotation.json",
    "opensubsonic-playlists.json",
    "opensubsonic-community.json",
    "opensubsonic-negative.json",
    "opensubsonic-conditional.json",
    "jellyfin-discovery.json",
    "jellyfin-auth.json",
    "jellyfin-browse.json",
    "jellyfin-playback.json",
    "jellyfin-userstate.json",
    "jellyfin-playlists.json",
    "jellyfin-images.json",
    "jellyfin-extra.json",
]


def cmd_capture(config: OracleConfig) -> None:
    """Reseed, then capture each native-v1 scenario file into a timestamped
    capture directory. Reseeding between files keeps state-mutating flows
    (star/rating/scrobble, refresh-token rotation, logout) deterministic."""
    if not server_alive(config):
        raise SystemExit("oracle server is not running; run `legacy_oracle.py start` first")

    # Inject the expired fixture JWT without writing it anywhere persistent.
    spec = importlib.util.spec_from_file_location(
        "gen_fixture_tokens", ROOT / "scripts" / "gen_fixture_tokens.py"
    )
    if spec is None or spec.loader is None:
        raise SystemExit("unable to load gen_fixture_tokens.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    expired_jwt = module.make_jwt(
        FIXTURE_JWT_KEY, expired=True, user_api_key=fixture_uuid("user", FIXTURE_USER)
    )

    subsonic_token = hashlib.md5(
        (FIXTURE_OPENSUBSONIC_SECRET + FIXTURE_SUBSONIC_SALT).encode("utf-8")
    ).hexdigest()
    # Forged unsigned JWT used to prove the OpenSubsonic jwt branch performs
    # no signature verification (payload-only claims mapping).
    forged_jwt = module.make_jwt(
        "phase0-unsigned-fixture-key",
        expired=False,
        user_api_key=fixture_uuid("user", FIXTURE_USER),
    )
    capture_env = {
        **os.environ,
        "LEGACY_MELODEE_EXPIRED_JWT": expired_jwt,
        "PHASE0_SUBSONIC_SALT": FIXTURE_SUBSONIC_SALT,
        "PHASE0_SUBSONIC_TOKEN": subsonic_token,
        "PHASE0_SUBSONIC_TOKEN_UPPER": subsonic_token.upper(),
        "PHASE0_SUBSONIC_JWT": forged_jwt,
    }

    timestamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    output_parent = ROOT / "compatibility" / "captures" / f"native-v1-{timestamp}"
    output_parent.mkdir(parents=True, exist_ok=False)
    for scenario_file in SCENARIO_FILES:
        scenario_path = ROOT / "compatibility" / "scenarios" / scenario_file
        if not scenario_path.exists():
            print(f"skipping {scenario_file} (not present)")
            continue
        # The native community flow needs the deterministic public share;
        # all other surface captures start without native-only state.
        cmd_seed(config, include_native_share=scenario_file == "native-community.json")
        # The legacy server caches user/song detail rows; restart so reseeded
        # credential and catalog changes are visible (Issue 6 discovery).
        cmd_stop(config)
        cmd_start(config)
        run(
            [
                sys.executable,
                str(ROOT / "scripts" / "capture_baseline.py"),
                "--scenarios",
                str(scenario_path),
                "--output",
                str(output_parent / scenario_file.replace(".json", "")),
            ],
            env=capture_env,
        )

    # Conditional-feature phase: podcast.enabled=false (the seeded default is
    # true). Toggle, restart (config cache), capture, restore.
    off_scenario = ROOT / "compatibility" / "scenarios" / "opensubsonic-conditional-off.json"
    if off_scenario.exists():
        set_setting(config, "podcast.enabled", "false")
        cmd_stop(config)
        cmd_start(config)
        try:
            run(
                [
                    sys.executable,
                    str(ROOT / "scripts" / "capture_baseline.py"),
                    "--scenarios",
                    str(off_scenario),
                    "--output",
                    str(output_parent / "opensubsonic-conditional-off"),
                ],
                env=capture_env,
            )
        finally:
            set_setting(config, "podcast.enabled", "true")
            cmd_stop(config)
            cmd_start(config)
    print(f"captures written to {output_parent.relative_to(ROOT)}")


def set_setting(config: OracleConfig, key: str, value: str) -> None:
    psql(
        config,
        "INSERT INTO \"Settings\" (\"ApiKey\",\"Category\",\"CreatedAt\",\"Description\",\"IsLocked\",\"Key\",\"LastUpdatedAt\",\"SortOrder\",\"Value\")"
        f" VALUES (gen_random_uuid(), 0, now(), 'Phase 0 conditional capture toggle', false, {sql_literal(key)}, now(), 0, {sql_literal(value)})"
        " ON CONFLICT (\"Key\") DO UPDATE SET \"Value\" = EXCLUDED.\"Value\";",
    )


def cmd_recreate(config: OracleConfig) -> None:
    sha = oracle_sha(config)
    if sha != FROZEN_COMMIT:
        raise SystemExit(
            f"oracle checkout is at {sha}, not frozen baseline {FROZEN_COMMIT}.\n"
            "Stop: reconcile the oracle checkout before generating captures."
        )
    cmd_build(config)
    cmd_stop(config)
    cmd_reset_db(config)
    cmd_seed(config)
    cmd_start(config)
    cmd_verify(config)
    print("recreate complete: oracle is live, seeded, and verified")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--legacy-root", help="path to the legacy melodee checkout")
    parser.add_argument(
        "command",
        choices=["status", "build", "reset-db", "seed", "start", "stop", "verify", "recreate", "capture"],
    )
    parser.add_argument(
        "--without-native-share",
        action="store_true",
        help="omit the native-v1 public-share fixture when running the seed command",
    )
    args = parser.parse_args()
    config = OracleConfig(args)
    if args.command == "seed":
        cmd_seed(config, include_native_share=not args.without_native_share)
    else:
        {
            "status": cmd_status,
            "build": cmd_build,
            "reset-db": cmd_reset_db,
            "start": cmd_start,
            "stop": cmd_stop,
            "verify": cmd_verify,
            "recreate": cmd_recreate,
            "capture": cmd_capture,
        }[args.command](config)


if __name__ == "__main__":
    main()
