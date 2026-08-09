#!/usr/bin/env python3
"""Generate deterministic synthetic benchmark datasets (FIRST-10-ISSUES.md
Issue 9) into a dedicated disposable PostgreSQL database.

Tiers:

- A (developer):        1,000 artists / 1,500 albums / 15,000 songs
- B (real-world scale): 35,000 artists / 50,000 albums / 560,000 songs
- C (stress):           100,000 artists / 500,000 albums / 5,000,000 songs

Rows are generated set-based in SQL with md5-derived deterministic UUIDs and
names (no per-row Python inserts; no media files — browse/search scale only).
The bench database is separate from the capture fixture DB; it is dropped and
recreated per invocation. Deterministic across runs (identical rows for a
given tier).
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LEGACY_ROOT = Path(os.environ.get("LEGACY_ROOT", str(ROOT.parent / "melodee")))

TIERS = {
    "A": {"artists": 1_000, "albums": 1_500, "songs": 15_000},
    "B": {"artists": 35_000, "albums": 50_000, "songs": 560_000},
    "C": {"artists": 100_000, "albums": 500_000, "songs": 5_000_000},
}

DB = {
    "host": os.environ.get("BENCH_DB_HOST", "127.0.0.1"),
    "port": os.environ.get("BENCH_DB_PORT", "5433"),
    "user": os.environ.get("BENCH_DB_USER", "melodee"),
    "password": os.environ.get("BENCH_DB_PASSWORD", "melodee-dev"),
}


def psql(sql: str, database: str = "postgres", check: bool = True) -> subprocess.CompletedProcess:
    env = {**os.environ, "PGPASSWORD": DB["password"]}
    return subprocess.run(
        ["psql", "-h", DB["host"], "-p", DB["port"], "-U", DB["user"],
         "-d", database, "-v", "ON_ERROR_STOP=1", "-c", sql],
        env=env, capture_output=True, text=True, check=check,
    )


def migrate(db_name: str) -> None:
    subprocess.run(
        [
            "dotnet", "ef", "database", "update",
            "--project", "src/Melodee.Common/Melodee.Common.csproj",
            "--startup-project", "src/Melodee.Blazor/Melodee.Blazor.csproj",
            "--context", "MelodeeDbContext",
            "--connection",
            f"Host={DB['host']};Port={DB['port']};Database={db_name};Username={DB['user']};Password={DB['password']};SSL Mode=Disable",
        ],
        cwd=LEGACY_ROOT,
        env={
            "HOME": os.environ.get("HOME", "/tmp"),
            "PATH": "/usr/local/bin:/usr/bin:/bin:" + str(Path.home() / ".dotnet" / "tools"),
            "DOTNET_CLI_TELEMETRY_OPTOUT": "1",
        },
        check=True,
    )


def seed_dataset(db_name: str, tier: str) -> dict[str, float]:
    sizes = TIERS[tier]
    n_artists, n_albums, n_songs = sizes["artists"], sizes["albums"], sizes["songs"]
    albums_per_artist = n_albums / n_artists
    songs_per_album = n_songs / n_albums
    timings: dict[str, float] = {}

    def timed(name: str, sql: str) -> None:
        start = time.monotonic()
        psql(sql, database=db_name)
        timings[name] = round(time.monotonic() - start, 2)

    timed("drop_and_recreate", "SELECT 1")

    timed(
        "settings_and_library",
        "INSERT INTO \"Libraries\" (\"Name\",\"Path\",\"Type\",\"IsLocked\",\"SortOrder\",\"ApiKey\",\"CreatedAt\")"
        f" VALUES ('Benchmark Storage {tier}','/tmp/media-baseline/bench-storage',3,false,0,"
        " '00000000-0000-4000-8000-0000000000b1', TIMESTAMPTZ '2026-08-08T00:00:00Z');"
        "INSERT INTO \"Settings\" (\"ApiKey\",\"Category\",\"CreatedAt\",\"Description\",\"IsLocked\",\"Key\",\"LastUpdatedAt\",\"SortOrder\",\"Value\")"
        " VALUES (gen_random_uuid(),0,now(),'bench',false,'system.baseUrl',now(),0,'http://127.0.0.1:5157')"
        " ON CONFLICT (\"Key\") DO UPDATE SET \"Value\" = EXCLUDED.\"Value\";",
    )

    # Deterministic set-based generation. UUIDs derive from md5 of a stable
    # key so every regeneration yields identical public IDs.
    timed(
        "artists",
        f"""
        SET session_replication_role = replica;
        INSERT INTO "Artists" ("Id","Name","NameNormalized","SortName","Directory","AlbumCount","SongCount",
                               "LibraryId","MetaDataStatus","IsLocked","SortOrder","ApiKey","CreatedAt",
                               "PlayedCount","CalculatedRating")
        SELECT
            1000000 + i,
            CASE WHEN i % 997 = 0 THEN 'アーティスト ' || i ELSE 'Artist ' || lpad(i::text, 8, '0') END,
            upper(CASE WHEN i % 997 = 0 THEN 'アーティスト ' || i ELSE 'Artist ' || lpad(i::text, 8, '0') END),
            'Artist ' || lpad(i::text, 8, '0'),
            'Artist ' || lpad(i::text, 8, '0'),
            {albums_per_artist}, {songs_per_album * albums_per_artist},
            (SELECT "Id" FROM "Libraries" LIMIT 1),
            2, false, 0,
            md5('bench-artist-' || i)::uuid,
            TIMESTAMPTZ '2026-01-01T00:00:00Z' + (i || ' minutes')::interval,
            0, 0
        FROM generate_series(1, {n_artists}) AS i;
        """,
    )

    timed(
        "albums",
        f"""
        INSERT INTO "Albums" ("Id","ArtistId","Name","NameNormalized","SortName","AlbumStatus","MetaDataStatus",
                              "AlbumType","OriginalReleaseDate","ReleaseDate","IsCompilation","SongCount","Duration",
                              "Directory","IsLocked","SortOrder","ApiKey","CreatedAt","PlayedCount","CalculatedRating")
        SELECT
            2000000 + j,
            1000000 + ((j - 1) % {n_artists}) + 1,
            'Album ' || lpad(j::text, 8, '0'),
            'ALBUM' || lpad(j::text, 8, '0'),
            'Album ' || lpad(j::text, 8, '0'),
            1, 2, 1,
            DATE '2000-01-01' + (j % 8000),
            DATE '2000-01-01' + (j % 8000),
            (j % 97 = 0),
            {songs_per_album}, {songs_per_album * 200000},
            'Album ' || lpad(j::text, 8, '0'),
            false, 0,
            md5('bench-album-' || j)::uuid,
            TIMESTAMPTZ '2026-01-01T00:00:00Z' + (j || ' minutes')::interval,
            0, 0
        FROM generate_series(1, {n_albums}) AS j;
        """,
    )

    timed(
        "songs",
        f"""
        INSERT INTO "Songs" ("Id","AlbumId","Title","TitleSort","TitleNormalized","SongNumber","FileName","FileSize",
                             "FileHash","Duration","SamplingRate","BitRate","BitDepth","BPM","ContentType","IsVbr",
                             "IsLocked","SortOrder","ApiKey","CreatedAt","PlayedCount","CalculatedRating")
        SELECT
            3000000 + k,
            2000000 + ((k - 1) % {n_albums}) + 1,
            CASE WHEN k % 9973 = 0 THEN '東京 Song ' || k ELSE 'Track ' || lpad(k::text, 9, '0') END,
            CASE WHEN k % 9973 = 0 THEN '東京 Song ' || k ELSE 'Track ' || lpad(k::text, 9, '0') END,
            upper(CASE WHEN k % 9973 = 0 THEN '東京 Song ' || k ELSE 'Track ' || lpad(k::text, 9, '0') END),
            ((k - 1) / {n_albums}) + 1,
            'track-' || k || '.mp3',
            3000000, md5('bench-song-' || k),
            200000, 44100, 320, 16, 120, 'audio/mpeg', false,
            false, 0,
            md5('bench-song-' || k)::uuid,
            TIMESTAMPTZ '2026-01-01T00:00:00Z' + (k || ' seconds')::interval,
            0, 0
        FROM generate_series(1, {n_songs}) AS k;
        SET session_replication_role = DEFAULT;
        """,
    )

    # Fixture user (synthetic, public fixture credential — same as Phase 0
    # captures) so authenticated API benchmarks can log in.
    cli_dir = LEGACY_ROOT / "src" / "Melodee.Cli" / "bin" / "Debug" / "net10.0"
    start = time.monotonic()
    subprocess.run(
        ["dotnet", "mcli.dll", "user", "create",
         "-u", "phase0-fixture", "-e", "phase0-fixture@example.test", "-p", "Mel0deeR0cks!", "-f"],
        cwd=cli_dir,
        env={
            "HOME": os.environ.get("HOME", "/tmp"),
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "security_secretKey": "phase0-fixture-only-security-secret-key-NOT-A-REAL-SECRET-0000",
            "ConnectionStrings__DefaultConnection": (
                f"Host={DB['host']};Port={DB['port']};Database={db_name};"
                f"Username={DB['user']};Password={DB['password']};SSL Mode=Disable"
            ),
            "ConnectionStrings__MusicBrainzConnection": "Data Source=/tmp/legacy-oracle/storage/_search-engines/musicbrainz/musicbrainz.ddb;",
            "ConnectionStrings__ArtistSearchEngineConnection": "Data Source=/tmp/legacy-oracle/storage/_search-engines/artistSearchEngine.db;Cache=Shared",
        },
        capture_output=True,
        text=True,
        check=True,
    )
    timings["fixture_user"] = round(time.monotonic() - start, 2)

    timed(
        "playlists",
        """
        INSERT INTO "Playlists" ("Name","Comment","UserId","IsPublic","SongCount","Duration","SongId",
                                 "IsLocked","SortOrder","ApiKey","CreatedAt")
        SELECT 'Bench Playlist ' || p, 'synthetic', (SELECT "Id" FROM "Users" ORDER BY "Id" LIMIT 1), true, 10, 2000000, 3000000 + p,
               false, 0, md5('bench-playlist-' || p)::uuid, TIMESTAMPTZ '2026-02-01T00:00:00Z'
        FROM generate_series(1, 5) AS p;
        INSERT INTO "PlaylistSong" ("SongId","PlaylistId","SongApiKey","PlaylistOrder")
        SELECT 3000000 + (s * 37), (SELECT "Id" FROM "Playlists" ORDER BY "Id" LIMIT 1),
               md5('bench-song-' || (s * 37))::uuid, s
        FROM generate_series(1, 10) AS s;
        """,
    )

    return timings


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tier", choices=["A", "B", "C"], required=True)
    parser.add_argument("--db-name", default=None)
    args = parser.parse_args()
    db_name = args.db_name or f"melodee_bench_{args.tier.lower()}"

    started = time.monotonic()
    psql(f"DROP DATABASE IF EXISTS {db_name} WITH (FORCE)")
    psql(f"CREATE DATABASE {db_name}")
    migrate(db_name)
    timings = seed_dataset(db_name, args.tier)
    counts = psql(
        "SELECT (SELECT COUNT(*) FROM \"Artists\") || '/' || (SELECT COUNT(*) FROM \"Albums\") || '/' || (SELECT COUNT(*) FROM \"Songs\")",
        database=db_name,
    ).stdout.strip().splitlines()[-2].strip()

    result = {
        "tier": args.tier,
        "database": db_name,
        "counts": counts,
        "targetCounts": TIERS[args.tier],
        "timingsSeconds": timings,
        "totalSeconds": round(time.monotonic() - started, 2),
        "uuidScheme": "md5('bench-<kind>-<i>')::uuid (deterministic per tier)",
    }
    out_dir = ROOT / "compatibility" / "benchmarks" / "datasets"
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / f"tier-{args.tier.lower()}.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=1))


if __name__ == "__main__":
    main()
