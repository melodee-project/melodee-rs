#!/usr/bin/env python3
"""Phase 8 live differential harness: replay the frozen Phase 0 scenario
suites against the real Rust server seeded with the Phase 0 fixture catalog.

The harness:

1. Creates a disposable PostgreSQL database (migrations run at server boot).
2. Generates the deterministic fixture media tree (the same WAV/PNG/LRC
   bytes ``legacy_oracle.py`` produced for the legacy baseline) under a
   temporary canonical storage root.
3. Seeds the Rust catalog schema with ``compatibility/fixtures/catalog-v1.json``
   using the frozen uuid5 public identifiers, so every captured request path
   resolves to the same entity it did on the legacy server.
4. Boots the real ``melodee-server`` binary (full middleware stack) on an
   ephemeral port with the pinned fixture ingestion source.
5. Replays every Jellyfin and OpenSubsonic scenario file through
   ``capture_baseline.py`` and diffs the result against the frozen goldens
   with ``compare_captures.py``.

Differences are classified against ``KNOWN_DIFFERENCES`` — the enumerated,
justified intentional divergences (deterministic-id family ADR-0131 D10, the
artwork-storage evidence gap, the absent legacy Blazor shell). Any difference
outside that registry fails the run.

Usage:

    python3 scripts/run_phase8_differential.py [--keep] [--only jellyfin|opensubsonic]
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import random
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import compare_captures  # noqa: E402
import capture_baseline  # noqa: E402


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"unable to load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


oracle = load_module("legacy_oracle", ROOT / "scripts" / "legacy_oracle.py")
tokens = load_module("gen_fixture_tokens", ROOT / "scripts" / "gen_fixture_tokens.py")

CATALOG_PATH = ROOT / "compatibility" / "fixtures" / "catalog-v1.json"
FIXTURE_TS = "2026-08-07T00:00:00Z"
FIXTURE_LIBRARY_ID = oracle.fixture_uuid("library", "phase0-fixture-library")
FIXTURE_USER_PUBLIC_ID = oracle.fixture_uuid("user", oracle.FIXTURE_USER)
DB_ADMIN_URL = "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres"

SCENARIO_GROUPS = {
    "jellyfin": (
        ROOT / "compatibility" / "jellyfin" / "golden",
        [
            "jellyfin-discovery.json",
            "jellyfin-auth.json",
            "jellyfin-browse.json",
            "jellyfin-playback.json",
            "jellyfin-userstate.json",
            "jellyfin-playlists.json",
            "jellyfin-images.json",
            "jellyfin-extra.json",
        ],
    ),
    "opensubsonic": (
        ROOT / "compatibility" / "opensubsonic" / "golden",
        [
            "opensubsonic-system.json",
            "opensubsonic-auth.json",
            "opensubsonic-browse.json",
            "opensubsonic-media.json",
            "opensubsonic-playlists.json",
            "opensubsonic-annotation.json",
            "opensubsonic-lists.json",
            "opensubsonic-community.json",
            "opensubsonic-conditional.json",
            "opensubsonic-conditional-off.json",
            "opensubsonic-negative.json",
        ],
    ),
}

# Enumerated intentional divergences from the frozen baseline. Each entry is
# (scenario name prefix, difference field substring, justification). Anything
# not matching an entry fails the run.
KNOWN_DIFFERENCES: list[tuple[str, str, str]] = [
    # The Rust catalog has no durable artwork storage yet; catalog entities
    # serve the captured no-artwork 404 classes. The 200/304 image paths the
    # legacy fixture produced are the documented Phase 8 evidence gap
    # (PHASE-8-REPORT.md); images.rs::resolve_image is the wiring point.
    ("jf-images/item-image-primary", "", "artwork storage evidence gap"),
    ("jf-images/item-image-head", "", "artwork storage evidence gap"),
    ("jf-images/item-image-304", "", "artwork storage evidence gap"),
    ("jf-images/artist-image", "", "artwork storage evidence gap"),
    ("jf-images/image-maxwidth-bucket", "", "artwork storage evidence gap"),
    ("jf-images/item-image-indexed", "", "artwork storage evidence gap"),
    # The legacy Blazor shell at `/` does not exist in the Rust server; the
    # adapter's documented discovery contract is /System/Info/Public.
    ("jf-root-no-markers", "", "legacy Blazor shell absent by design"),
    ("jf-root-head-mediabrowser", "", "legacy Blazor shell absent by design"),
    ("jf-system-info-fallthrough", "", "legacy Blazor shell absent by design"),
    # Configuration-derived UserViews: the legacy random per-install "Storage"
    # library id/etag is replaced by a deterministic id (ADR-0131 D10 family),
    # and configured source creation time is boot time, not the fixture date.
    ("jf-browse/userviews", "Items[0].DateCreated", "configured-source creation time is boot time"),
    ("jf-browse/userviews", "Items[0].Etag", "etag derives from creation time (ADR-0131 D10)"),
    ("jf-browse/userviews", "Items[1]", "deterministic Storage library id replaces legacy random (ADR-0131 D10)"),
    ("jf-browse/user-views-by-id", "Items[0].DateCreated", "configured-source creation time is boot time"),
    ("jf-browse/user-views-by-id", "Items[0].Etag", "etag derives from creation time (ADR-0131 D10)"),
    ("jf-browse/user-views-by-id", "Items[1]", "deterministic Storage library id replaces legacy random (ADR-0131 D10)"),
    # D4 do-not-preserve: the legacy second playlist add corrupted state and
    # the frozen capture records its 500; the Rust adapter performs the clean
    # add (204) per the documented D4 decision.
    ("jf-playlists/add-second-nonempty", "", "D4: legacy 500 on second add intentionally not preserved"),
    # Same D4 corruption family: legacy remove-by-entryIds also rewrote the
    # surviving PlaylistOrder values (frozen `[Falling, Neon, Coda]`); the
    # Rust adapter removes exactly the addressed entry and preserves order
    # (`[Coda, Falling, Neon]`).
    ("jf-playlists/items-after-remove", "", "D4 family: legacy remove corrupted surviving PlaylistOrder"),
    # Legacy's synthetic "Storage" music folder id was random per install; the
    # Rust adapter derives a deterministic id (ADR-0131 D10 family).
    ("os-getmusicfolders-json", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-getmusicfolders-xml", "response.body", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-no-version-param", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-jwt-forged-unsigned", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-bypass-localhost", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-valid-token", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-valid-token-uppercase", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-valid-password-plain", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-valid-password-enc-hex", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    ("os-auth-form-post", "musicFolder[1].id", "deterministic Storage folder id replaces legacy random (ADR-0131 D10)"),
    # R1 security decision (Phase 0 report): the legacy jwt branch verified NO
    # signature — knowing a username authenticated as that user. The Rust
    # adapter rejects unsigned/forged JWTs; the frozen 200 is not preserved.
    ("os-auth-jwt-forged-unsigned", "", "R1: legacy unsigned-JWT auth bypass intentionally not preserved"),
    # The legacy serializer embedded the request origin in the image URL
    # fields (artistImageUrl, albumInfo/artistInfo small/medium/large URLs)
    # and the Phase 0 capture did not mask it (the goldens literally encode
    # the capture port 5157); the value is per-run origin data, not contract.
    ("", "ImageUrl", "image URL embeds per-run origin; unmasked capture volatility"),
    # Legacy Album.OriginalReleaseDate/PlayedCount/LastPlayedAt are
    # album-level columns with no Rust schema counterpart (the domain models
    # play stats per user; original release date storage needs a schema ADR).
    # `played` and song-summed play counts are reproduced; the standalone
    # original-release-date fields are not.
    ("", "originalReleaseDate", "legacy Album.OriginalReleaseDate column absent from Rust schema (deferred schema ADR)"),
    ("", "originalAlbumDate", "legacy Album.OriginalReleaseDate column absent from Rust schema (deferred schema ADR)"),
    # Legacy NULL SortName artifacts: entities without a catalog sortName
    # (Sæglópur 日本, Cassette Ghost, all albums but Northern Lights) omitted
    # `sortName`; the Rust schema requires non-empty sort values (migration
    # 0007 CHECK), so the adapter always emits the resolved sort value.
    ("os-getalbum-minimal", "sortName", "legacy null-SortName omission artifact; Rust requires non-empty sort values"),
    ("os-getsongsbygenre-unicode", "sortName", "legacy null-SortName omission artifact; Rust requires non-empty sort values"),
    # The XML artist index serializes the legacy null-SortName '' group,
    # which the Rust domain cannot reproduce (non-empty sort_name CHECK).
    ("os-getartists-xml", "", "legacy null-SortName empty-index artifact in the XML serializer"),
    # Sized default cover art: the legacy ETag derivation for processed
    # images is not reproducible (id-dependent hash); bytes/status/headers
    # otherwise match the frozen goldens.
    ("os-getcoverart-song", "response.headers", "legacy processed-image etag derivation not reproducible"),
    ("os-getcoverart-80", "response.headers", "legacy processed-image etag derivation not reproducible"),
    # Unknown-id 302 redirects: the legacy Location embeds the per-run
    # X-Forwarded-Host (unmasked capture volatility) and the full credential
    # query — the credential leak is intentionally stripped (ADR-0130). The
    # 302 + /Account/Login target class is preserved.
    ("os-stream-unknown", "", "redirect Location embeds per-run forwarded host; credential strip per ADR-0130"),
    ("os-download-unknown", "", "redirect Location embeds per-run forwarded host; credential strip per ADR-0130"),
    ("os-playlists/getplaylist-unknown", "", "redirect Location embeds per-run forwarded host; credential strip per ADR-0130"),
    ("os-shares/create-share-bad-id", "", "redirect Location embeds per-run forwarded host; credential strip per ADR-0130"),
    ("os-searchforartist", "", "redirect Location embeds per-run forwarded host; credential strip per ADR-0130"),
    ("os-searchforartistimage", "", "redirect Location embeds per-run forwarded host; credential strip per ADR-0130"),
    ("os-searchforalbumimage", "", "redirect Location embeds per-run forwarded host; credential strip per ADR-0130"),
    # ADR-0130 decision 4 / Phase 0 risk R2: the legacy double-range bug
    # (416 for in-file middle ranges, suffix ranges serving the FIRST N bytes,
    # Content-Range totals equal to the requested length) is intentionally NOT
    # preserved; the Rust streamer implements correct single-pass ranges.
    ("os-stream-range-middle", "", "ADR-0130 decision 4 (risk D2/R2): legacy double-range 416 bug intentionally not preserved"),
    ("os-stream-range-middle-mangled", "", "ADR-0130 decision 4 (risk D2/R2): legacy double-range clamp bug intentionally not preserved"),
    ("os-stream-range-end", "", "ADR-0130 decision 4 (risk D2/R2): legacy suffix-range first-N bug intentionally not preserved"),
    # averageRating = legacy CalculatedRating columns, which the Phase 0
    # oracle seeded directly (bypassing the rating service) — so fixture
    # ratings never reach the legacy aggregate while runtime setRating calls
    # do. The Rust adapter computes the aggregate from user ratings
    # uniformly, matching the runtime-observed rule; the fixture-seeded rows
    # diverge as a capture-seeding artifact.
    ("os-getstarred", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-getstarred2", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-annotation-star/verify-starred2-after-star", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-annotation-star/verify-starred2-after-unstar", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-annotation-star/verify-starred2-album", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-getalbumlist-newest", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-shares/create-share", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-shares/verify-shares", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    ("os-shares/verify-shares-empty", ".averageRating", "fixture-seeded ratings bypassed legacy CalculatedRating (seeding artifact)"),
    # Legacy grouped artists with NULL SortName under an empty-name index
    # (Sæglópur 日本, Cassette Ghost); the Rust domain requires a non-empty
    # sort_name, so those artists group under S/C. The legacy artifact is not
    # reproducible without weakening the domain invariant.
    ("os-getartists", "artists.index", "legacy null-SortName empty-index artifact; Rust requires sort_name"),
    ("os-getindexes", "indexes.index", "legacy null-SortName empty-index artifact; Rust requires sort_name"),
    ("os-getmusicdirectory", "indexes.index", "legacy null-SortName empty-index artifact; Rust requires sort_name"),
]


def uuid7() -> str:
    """Minimal UUIDv7 (48-bit ms timestamp + random), matching the schema's
    id_v7 CHECK constraints."""
    # 48 bits ts | 4 ver | 12 rnd_a | 2 var | 62 rnd_b
    timestamp_ms = int(time.time() * 1000) & 0xFFFFFFFFFFFF
    random_bits = secrets.randbits(74)
    rnd_a = (random_bits >> 62) & 0xFFF
    rnd_b = random_bits & 0x3FFFFFFFFFFFFFFF
    value = (timestamp_ms << 80) | (0x7 << 76) | (rnd_a << 64) | (0x2 << 62) | rnd_b
    hexed = f"{value:032x}"
    return f"{hexed[0:8]}-{hexed[8:12]}-{hexed[12:16]}-{hexed[16:20]}-{hexed[20:32]}"


def psql(sql: str, *, database: str = "postgres") -> None:
    subprocess.run(
        [
            "psql",
            "-h",
            "127.0.0.1",
            "-p",
            "5433",
            "-U",
            "melodee",
            "-v",
            "ON_ERROR_STOP=1",
            "-At",
            "-d",
            database,
            "-c",
            sql,
        ],
        env={**os.environ, "PGPASSWORD": "melodee-dev"},
        check=True,
        capture_output=True,
        text=True,
    )


def generate_media(catalog: dict, storage_root: Path) -> dict[str, tuple[int, str]]:
    """Write the fixture media tree under the canonical storage root; returns
    {song_slug: (byte_size, sha256)} exactly as the legacy seeder recorded."""
    results: dict[str, tuple[int, str]] = {}
    for artist in catalog["artists"]:
        for album in artist["albums"]:
            for song in album["songs"]:
                relative = Path(artist["directory"]) / album["directory"] / song["fileName"]
                target = storage_root / relative
                oracle.make_wav(target, seconds=song["mediaSeconds"], frequency=song["mediaFrequency"])
                data = target.read_bytes()
                results[song["slug"]] = (len(data), hashlib.sha256(data).hexdigest())
    # Synchronized-lyrics sidecar for /Audio/{id}/Lyrics.
    aurora = next(a for a in catalog["artists"] if a["slug"] == "aurora-fields")
    northern = aurora["albums"][0]
    lrc = storage_root / aurora["directory"] / northern["directory"] / "02 - Polar Veil.lrc"
    lrc.write_text(
        "[00:00.00] Synthetic lyric line one\n[00:01.00] Synthetic lyric line two\n",
        encoding="utf-8",
    )
    return results


def build_seed_sql(catalog: dict, media: dict[str, tuple[int, str]], password_hash: str) -> str:
    """Map catalog-v1.json into the Rust catalog schema with frozen uuid5
    public identifiers. Internal ids are uuidv7 per the schema CHECKs."""
    lit = oracle.sql_literal
    fxid = oracle.fixture_uuid
    s: list[str] = ["BEGIN;"]

    # --- fixture user -------------------------------------------------------
    user_id = uuid7()
    s.append(
        "INSERT INTO native_users (id, public_api_id, username, normalized_username, email,"
        " normalized_email, password_hash, password_hash_algorithm, stream_signing_key,"
        " is_admin, is_editor, enabled, locked, blacklisted, preferred_language,"
        " preferred_theme, auth_revision, created_at, updated_at) VALUES ("
        + ",".join(
            [
                lit(user_id),
                lit(FIXTURE_USER_PUBLIC_ID),
                lit(oracle.FIXTURE_USER),
                lit(oracle.FIXTURE_USER.upper()),
                lit(oracle.FIXTURE_EMAIL),
                lit(oracle.FIXTURE_EMAIL.upper()),
                lit(password_hash),
                lit("bcrypt"),
                lit("phase8-differential-stream-signing-key"),
                "true",
                "false",
                "true",
                "false",
                "false",
                lit("en-US"),
                lit("dark"),
                "1",
                lit(FIXTURE_TS),
                lit(FIXTURE_TS),
            ]
        )
        + ");"
    )
    # Legacy User booleans HasShareRole/HasJukeboxRole default to true; the
    # frozen getUser golden shows them set for the fixture user.
    for role in (
        "Administrator",
        "HasStreamRole",
        "HasDownloadRole",
        "HasJukeboxRole",
        "HasShareRole",
    ):
        s.append(
            "INSERT INTO native_user_roles (user_id, role, created_at) VALUES ("
            f"{lit(user_id)},{lit(role)},{lit(FIXTURE_TS)});"
        )
    s.append(
        "INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) VALUES ("
        f"{lit(user_id)},{lit(oracle.FIXTURE_OPENSUBSONIC_SECRET)},{lit(FIXTURE_TS)},{lit(FIXTURE_TS)});"
    )

    # --- catalog ------------------------------------------------------------
    artist_ids: dict[str, str] = {}
    release_ids: dict[str, str] = {}
    track_ids: dict[str, str] = {}
    release_genres: dict[str, list[str]] = {}

    # Frozen entity CreatedAt stagger: artists and their albums share one
    # global counter in catalog order (artist first, then its albums), one day
    # per entity from 2026-01-01. The frozen etags encode these values.
    entity_dates: dict[str, str] = {}
    counter = 0
    for artist in catalog["artists"]:
        counter += 1
        entity_dates[artist["slug"]] = f"2026-01-{counter:02d}T00:00:00Z"
        for album in artist["albums"]:
            counter += 1
            entity_dates[album["slug"]] = f"2026-01-{counter:02d}T00:00:00Z"

    for artist in catalog["artists"]:
        artist_ids[artist["slug"]] = uuid7()
        s.append(
            "INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name,"
            " artist_type, status, revision, created_at, updated_at) VALUES ("
            + ",".join(
                [
                    lit(artist_ids[artist["slug"]]),
                    lit(fxid("artist", artist["slug"])),
                    lit(artist["name"]),
                    lit(artist["nameNormalized"]),
                    lit(artist.get("sortName") or artist["name"]),
                    lit("unknown"),
                    lit("verified"),
                    "1",
                    lit(artist.get("createdAt", entity_dates[artist["slug"]])),
                    lit(artist.get("createdAt", entity_dates[artist["slug"]])),
                ]
            )
            + ");"
        )

    song_counter = 0
    entity_day = 0
    for artist in catalog["artists"]:
        for album in artist["albums"]:
            release_ids[album["slug"]] = uuid7()
            genres = album.get("genres") or []
            release_genres[album["slug"]] = genres
            s.append(
                "INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title,"
                " display_artist, release_date, status, revision, created_at, updated_at) VALUES ("
                + ",".join(
                    [
                        lit(release_ids[album["slug"]]),
                        lit(fxid("album", album["slug"])),
                        lit(album["name"]),
                        lit(album["nameNormalized"]),
                        lit(album.get("sortName") or album["name"]),
                        lit(artist["name"]),
                        lit(album["releaseDate"]),
                        lit("active"),
                        "1",
                        lit(album.get("createdAt", entity_dates[album["slug"]])),
                        lit(album.get("createdAt", entity_dates[album["slug"]])),
                    ]
                )
                + ");"
            )
            s.append(
                "INSERT INTO release_artists (id, release_id, artist_id, role, presentation_order,"
                " display_value, join_phrase, created_at) VALUES ("
                + ",".join(
                    [
                        lit(uuid7()),
                        lit(release_ids[album["slug"]]),
                        lit(artist_ids[artist["slug"]]),
                        lit("primary"),
                        "1",
                        lit(artist["name"]),
                        lit(""),
                        lit(FIXTURE_TS),
                    ]
                )
                + ");"
            )
            duration_total = sum(song["durationMs"] for song in album["songs"])
            bytes_total = sum(media[song["slug"]][0] for song in album["songs"])
            s.append(
                "INSERT INTO release_catalog_aggregates (release_id, track_count, duration_ms,"
                " media_bytes, revision, updated_at) VALUES ("
                f"{lit(release_ids[album['slug']])},{len(album['songs'])},{duration_total},"
                f"{bytes_total},1,{lit(FIXTURE_TS)});"
            )

            for song in album["songs"]:
                song_counter += 1
                # Legacy staggered song CreatedAt so global listings order
                # deterministically; the frozen etags encode these values.
                song_created = f"2026-01-01T00:{song_counter % 60:02d}:{song_counter % 60:02d}Z"
                size, sha = media[song["slug"]]
                track_ids[song["slug"]] = uuid7()
                identity_id = uuid7()
                asset_id = uuid7()
                relative = str(
                    Path(artist["directory"]) / album["directory"] / song["fileName"]
                )
                s.append(
                    "INSERT INTO audio_identities (id, status, revision, created_at, updated_at)"
                    f" VALUES ({lit(identity_id)},'verified',1,{lit(FIXTURE_TS)},{lit(FIXTURE_TS)});"
                )
                s.append(
                    "INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size,"
                    " mime_type, container, codec, sample_rate_hz, channels, bitrate_bps,"
                    " duration_ms, created_at) VALUES ("
                    + ",".join(
                        [
                            lit(asset_id),
                            lit(relative),
                            lit(sha),
                            str(size),
                            lit("audio/wav"),
                            lit("wav"),
                            lit("pcm_s16le"),
                            "44100",
                            "1",
                            # Legacy fixture stored BitRate=1411 (kbps) for
                            # every song regardless of the actual WAV rate;
                            # the frozen MediaSources Bitrate values encode it.
                            "1411000",
                            str(song["durationMs"]),
                            lit(FIXTURE_TS),
                        ]
                    )
                    + ");"
                )
                s.append(
                    "INSERT INTO tracks (id, public_api_id, release_id, audio_identity_id,"
                    " playable_media_asset_id, ordinal, title, normalized_title, sort_title,"
                    " display_artist, disc_number, track_number, duration_ms, source_path,"
                    " revision, created_at, updated_at) VALUES ("
                    + ",".join(
                        [
                            lit(track_ids[song["slug"]]),
                            lit(fxid("song", song["slug"])),
                            lit(release_ids[album["slug"]]),
                            lit(identity_id),
                            lit(asset_id),
                            str(song["songNumber"]),
                            lit(song["title"]),
                            lit(song["titleNormalized"]),
                            lit(song.get("titleSort") or song["title"]),
                            lit(None),
                            lit(None),
                            str(song["songNumber"]),
                            str(song["durationMs"]),
                            # Legacy Song path = album directory + file name
                            # (frozen `path` field, e.g. "Northern Lights/01 -
                            # Solar Wind.wav"); also the manifest join key.
                            lit(f"{album['directory']}/{song['fileName']}"),
                            "1",
                            lit(song_created),
                            lit(song_created),
                        ]
                    )
                    + ");"
                )
                s.append(
                    "INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal,"
                    " created_at) VALUES ("
                    f"{lit(track_ids[song['slug']])},{lit(asset_id)},'playable',1,{lit(FIXTURE_TS)});"
                )
                s.append(
                    "INSERT INTO native_track_audio_features (track_id, tempo, musical_key,"
                    " mode, time_signature, acousticness, danceability, energy,"
                    " instrumentalness, liveness, loudness, speechiness, valence, updated_at)"
                    " VALUES ("
                    + ",".join(
                        [
                            lit(track_ids[song["slug"]]),
                            # Legacy Song.BPM (0 when analysis had not run).
                            str(song.get("bpm", 0)),
                            lit(None),
                            lit(None),
                            "4",
                            "0.0",
                            "0.0",
                            "0.0",
                            "0.0",
                            "0.0",
                            "0.0",
                            "0.0",
                            "0.0",
                            lit(FIXTURE_TS),
                        ]
                    )
                    + ");"
                )
                # Global play counts (legacy Song.PlayedCount): the OpenSubsonic
                # playCount fields count native_track_plays rows.
                for play_index in range(song.get("playedCount") or 0):
                    s.append(
                        "INSERT INTO native_track_plays (id, user_id, track_id,"
                        " client_event_id, played_at, submitted_at, duration_seconds,"
                        " completed) VALUES ("
                        + ",".join(
                            [
                                lit(uuid7()),
                                lit(user_id),
                                lit(track_ids[song["slug"]]),
                                lit(f"phase8-fixture-{song['slug']}-{play_index}"),
                                # Date the global play rows so max(played_at)
                                # per album matches the legacy
                                # Album.LastPlayedAt fixture values.
                                lit(album.get("lastPlayedAt", FIXTURE_TS)),
                                lit(album.get("lastPlayedAt", FIXTURE_TS)),
                                str(song["durationMs"] // 1000),
                                "true",
                            ]
                        )
                        + ");"
                    )
                s.append(
                    "INSERT INTO track_artists (id, track_id, artist_id, role, presentation_order,"
                    " display_value, join_phrase, created_at) VALUES ("
                    + ",".join(
                        [
                            lit(uuid7()),
                            lit(track_ids[song["slug"]]),
                            lit(artist_ids[artist["slug"]]),
                            lit("primary"),
                            "1",
                            lit(artist["name"]),
                            lit(""),
                            lit(FIXTURE_TS),
                        ]
                    )
                    + ");"
                )

    for artist in catalog["artists"]:
        release_count = len(artist["albums"])
        track_count = sum(len(a["songs"]) for a in artist["albums"])
        s.append(
            "INSERT INTO artist_catalog_aggregates (artist_id, release_count, billed_track_count,"
            " performance_track_count, all_credit_track_count, revision, updated_at) VALUES ("
            f"{lit(artist_ids[artist['slug']])},{release_count},{track_count},{track_count},"
            f"{track_count},1,{lit(FIXTURE_TS)});"
        )

    # --- genre evidence: one work item + manifest per release ---------------
    for artist in catalog["artists"]:
        for album in artist["albums"]:
            work_item_id = uuid7()
            manifest_id = uuid7()
            genre_value = ";".join(release_genres[album["slug"]])
            files = [
                {
                    "relative_path": f"{album['directory']}/{song['fileName']}",
                    "metadata": ({"genre": genre_value} if genre_value else {}),
                    "audio": {
                        "bit_depth": 16,
                        "sample_rate": 44100,
                        "channels": 1,
                    },
                }
                for song in album["songs"]
            ]
            manifest_payload = json.dumps({"files": files})
            s.append(
                "INSERT INTO release_work_items (id, api_key, source_path, current_path, state,"
                " attempt_count, correlation_id, discovered_at, updated_at, created_at) VALUES ("
                + ",".join(
                    [
                        lit(work_item_id),
                        lit(uuid7()),
                        lit("/phase8-fixture/" + album["slug"]),
                        lit("/phase8-fixture/" + album["slug"]),
                        lit("published"),
                        "0",
                        lit(uuid7()),
                        lit(FIXTURE_TS),
                        lit(FIXTURE_TS),
                        lit(FIXTURE_TS),
                    ]
                )
                + ");"
            )
            # id is GENERATED ALWAYS AS IDENTITY; catalog_imports references
            # it via the unique work_item_id below.
            s.append(
                "INSERT INTO release_work_item_manifests (work_item_id, schema_version,"
                " manifest_hash, manifest_payload, source_stable, generation_duration_ms,"
                " generated_at) VALUES ("
                + ",".join(
                    [
                        lit(work_item_id),
                        "3",
                        lit(hashlib.sha256(manifest_payload.encode()).hexdigest()),
                        lit(manifest_payload) + "::jsonb",
                        "true",
                        "0",
                        lit(FIXTURE_TS),
                    ]
                )
                + ");"
            )
            authorization_id = uuid7()
            release_hex = release_ids[album["slug"]].replace("-", "")
            target_path = f"releases/{release_hex[0:2]}/{release_hex[2:4]}/rel_{release_hex}"
            s.append(
                "INSERT INTO release_publication_authorizations (authorization_id, work_item_id,"
                " release_id, authorization_name, authorization_version, validation_profile_name,"
                " validation_profile_version, validation_outcome, validation_payload,"
                " source_disposition, authorized_at) VALUES ("
                + ",".join(
                    [
                        lit(authorization_id),
                        lit(work_item_id),
                        lit(release_ids[album["slug"]]),
                        lit("phase3.structural-publication"),
                        "1",
                        lit("phase8-fixture"),
                        "1",
                        lit("passed"),
                        "'{}'::jsonb",
                        lit("retained"),
                        lit(FIXTURE_TS),
                    ]
                )
                + ");"
            )
            s.append(
                "INSERT INTO release_publication_plans (work_item_id, authorization_id, release_id,"
                " layout_version, input_manifest_hash, target_relative_path, manifest_relative_path,"
                " plan_hash, plan_payload, created_at) VALUES ("
                + ",".join(
                    [
                        lit(work_item_id),
                        lit(authorization_id),
                        lit(release_ids[album["slug"]]),
                        "1",
                        lit(hashlib.sha256(manifest_payload.encode()).hexdigest()),
                        lit(target_path),
                        lit(target_path + "/manifest.json"),
                        lit(hashlib.sha256(f"phase8-fixture-plan:{album['slug']}".encode()).hexdigest()),
                        "'{}'::jsonb",
                        lit(FIXTURE_TS),
                    ]
                )
                + ");"
            )
            s.append(
                "INSERT INTO release_publication_receipts (work_item_id, release_id, plan_hash,"
                " sealed_manifest_hash, source_disposition, published_at) VALUES ("
                + ",".join(
                    [
                        lit(work_item_id),
                        lit(release_ids[album["slug"]]),
                        lit(hashlib.sha256(f"phase8-fixture-plan:{album['slug']}".encode()).hexdigest()),
                        lit(hashlib.sha256(f"phase8-fixture-sealed:{album['slug']}".encode()).hexdigest()),
                        lit("retained"),
                        lit(FIXTURE_TS),
                    ]
                )
                + ");"
            )
            s.append(
                "INSERT INTO catalog_imports (id, work_item_id, release_id, manifest_row_id,"
                " manifest_id, manifest_schema_version, semantic_manifest_hash,"
                " publication_plan_hash, sealed_manifest_hash, resolver_version, imported_at)"
                " VALUES ("
                + ",".join(
                    [
                        lit(uuid7()),
                        lit(work_item_id),
                        lit(release_ids[album["slug"]]),
                        "(SELECT id FROM release_work_item_manifests WHERE work_item_id = "
                        + lit(work_item_id)
                        + ")",
                        lit(manifest_id),
                        "3",
                        lit(hashlib.sha256(manifest_payload.encode()).hexdigest()),
                        lit(hashlib.sha256(b"phase8-fixture-plan").hexdigest()),
                        lit(hashlib.sha256(b"phase8-fixture-sealed").hexdigest()),
                        lit("phase8-fixture"),
                        lit(FIXTURE_TS),
                    ]
                )
                + ");"
            )

    # --- artist relationships -------------------------------------------------
    for relation in catalog.get("artistRelations", []):
        s.append(
            "INSERT INTO artist_relationships (id, source_artist_id, target_artist_id,"
            " relationship_type, confidence, evidence_source, is_locked, created_at) VALUES ("
            + ",".join(
                [
                    lit(uuid7()),
                    lit(artist_ids[relation["artist"]]),
                    lit(artist_ids[relation["relatedArtist"]]),
                    lit("member"),
                    "100",
                    lit("phase8-fixture"),
                    "false",
                    lit(FIXTURE_TS),
                ]
            )
            + ");"
        )

    # --- user state -------------------------------------------------------------
    for entry in catalog.get("userSongs", []):
        s.append(
            "INSERT INTO native_track_user_states (user_id, track_id, starred, hated, rating,"
            " pinned, play_count, created_at, updated_at) VALUES ("
            + ",".join(
                [
                    lit(user_id),
                    lit(track_ids[entry["song"]]),
                    "true" if entry.get("isStarred") else "false",
                    "true" if entry.get("isHated") else "false",
                    str(entry.get("rating", 0)),
                    "false",
                    str(entry.get("playedCount", 0)),
                    lit(FIXTURE_TS),
                    lit(FIXTURE_TS),
                ]
            )
            + ");"
        )
    for entry in catalog.get("userAlbums", []):
        s.append(
            "INSERT INTO native_release_user_states (user_id, release_id, starred, hated, rating,"
            " pinned, play_count, created_at, updated_at) VALUES ("
            + ",".join(
                [
                    lit(user_id),
                    lit(release_ids[entry["album"]]),
                    "true" if entry.get("isStarred") else "false",
                    "true" if entry.get("isHated") else "false",
                    str(entry.get("rating", 0)),
                    "false",
                    str(entry.get("playedCount", 0)),
                    lit(FIXTURE_TS),
                    lit(FIXTURE_TS),
                ]
            )
            + ");"
        )
    for entry in catalog.get("userArtists", []):
        s.append(
            "INSERT INTO native_artist_user_states (user_id, artist_id, starred, hated, rating,"
            " pinned, play_count, created_at, updated_at) VALUES ("
            + ",".join(
                [
                    lit(user_id),
                    lit(artist_ids[entry["artist"]]),
                    "true" if entry.get("isStarred") else "false",
                    "true" if entry.get("isHated") else "false",
                    str(entry.get("rating", 0)),
                    "false",
                    str(entry.get("playedCount", 0)),
                    lit(FIXTURE_TS),
                    lit(FIXTURE_TS),
                ]
            )
            + ");"
        )

    # --- playlists --------------------------------------------------------------
    for playlist_index, playlist in enumerate(catalog.get("playlists", []), start=1):
        playlist_id = uuid7()
        playlist_date = f"2026-03-{playlist_index:02d}T00:00:00Z"
        s.append(
            "INSERT INTO native_playlists (id, public_api_id, owner_user_id, name, description,"
            " is_public, revision, created_at, updated_at) VALUES ("
            + ",".join(
                [
                    lit(playlist_id),
                    lit(fxid("playlist", playlist["slug"])),
                    lit(user_id),
                    lit(playlist["name"]),
                    # Frozen Overview surfaces the legacy Comment column.
                    lit(playlist.get("comment") or playlist.get("description")),
                    "true" if playlist.get("isPublic") else "false",
                    "1",
                    lit(playlist_date),
                    lit(playlist_date),
                ]
            )
            + ");"
        )
        for position, song_slug in enumerate(playlist["songs"], start=1):
            s.append(
                "INSERT INTO native_playlist_items (id, playlist_id, track_id, position,"
                " added_by_user_id, created_at) VALUES ("
                + ",".join(
                        [
                            lit(uuid7()),
                            lit(playlist_id),
                            lit(track_ids[song_slug]),
                            str(position),
                            lit(user_id),
                            lit(FIXTURE_TS),
                        ]
                    )
                + ");"
            )

    s.append("COMMIT;")
    return "\n".join(s) + "\n"


def capture_env() -> dict[str, str]:
    subsonic_token = hashlib.md5(
        (oracle.FIXTURE_OPENSUBSONIC_SECRET + oracle.FIXTURE_SUBSONIC_SALT).encode("utf-8")
    ).hexdigest()
    forged_jwt = tokens.make_jwt(
        "phase0-unsigned-fixture-key",
        expired=False,
        user_api_key=FIXTURE_USER_PUBLIC_ID,
    )
    return {
        **os.environ,
        "PHASE0_SUBSONIC_SALT": oracle.FIXTURE_SUBSONIC_SALT,
        "PHASE0_SUBSONIC_TOKEN": subsonic_token,
        "PHASE0_SUBSONIC_TOKEN_UPPER": subsonic_token.upper(),
        "PHASE0_SUBSONIC_JWT": forged_jwt,
    }


def wait_ready(base_url: str, process: subprocess.Popen, timeout: float = 120.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise SystemExit(f"server exited during startup with {process.returncode}")
        try:
            with urllib.request.urlopen(base_url + "/System/Info/Public", timeout=2) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.5)
    raise SystemExit("server did not become ready in time")


def is_known_difference(
    name: str, field: str, expected: object, actual: object
) -> str | None:
    # The Phase 0 capture did not record content-length on JSON responses
    # (Kestrel chunked encoding); the Rust server sets it. A headers diff whose
    # only delta is an extra actual content-length is transport-level and
    # non-contractual.
    if field == "response.headers" and isinstance(expected, dict) and isinstance(actual, dict):
        extra = {k: v for k, v in actual.items() if k not in expected}
        missing = {k: v for k, v in expected.items() if k not in actual}
        changed = {
            k for k in expected.keys() & actual.keys() if expected[k] != actual[k]
        }
        if not missing and not changed and set(extra) == {"content-length"}:
            return "content-length not recorded in Phase 0 captures (chunked JSON)"
    for prefix, field_fragment, reason in KNOWN_DIFFERENCES:
        if name.startswith(prefix) and field_fragment in field:
            return reason
    return None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--keep", action="store_true", help="keep the work directory, database, and captures")
    parser.add_argument("--only", choices=sorted(SCENARIO_GROUPS), help="run only one surface")
    parser.add_argument("--db-name", default="melodee_phase8_diff")
    parser.add_argument("--port", type=int, default=0, help="bind port (0 = pick a free one)")
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()

    if not args.skip_build:
        subprocess.run(["cargo", "build", "-p", "melodee-server"], cwd=ROOT, check=True)

    work_dir = args.work_dir or Path(tempfile.mkdtemp(prefix="phase8-diff-"))
    work_dir.mkdir(parents=True, exist_ok=True)
    storage_root = work_dir / "storage"
    inbound_root = work_dir / "inbound"
    storage_root.mkdir(exist_ok=True)
    inbound_root.mkdir(exist_ok=True)
    captures_dir = work_dir / "captures"
    captures_dir.mkdir(exist_ok=True)

    port = args.port or random.randint(20000, 45000)
    base_url = f"http://127.0.0.1:{port}"

    catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
    media = generate_media(catalog, storage_root)

    password_hash = subprocess.run(
        [
            "python3",
            "-c",
            "import bcrypt,sys; sys.stdout.write(bcrypt.hashpw(sys.argv[1].encode(), bcrypt.gensalt(4)).decode())",
            oracle.FIXTURE_PASSWORD,
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout

    seed_sql = build_seed_sql(catalog, media, password_hash)
    (work_dir / "seed.sql").write_text(seed_sql, encoding="utf-8")

    psql(f'DROP DATABASE IF EXISTS "{args.db_name}" WITH (FORCE);')
    psql(f'CREATE DATABASE "{args.db_name}";')

    def server_env(podcasts_enabled: bool) -> dict[str, str]:
        return {
            **os.environ,
            "MELODEE__DATABASE__URL": f"postgres://melodee:melodee-dev@127.0.0.1:5433/{args.db_name}",
            "MELODEE__SERVER__BIND": f"127.0.0.1:{port}",
            "MELODEE__STORAGE__ROOT": str(storage_root),
            "MELODEE__INGEST__ENABLED": "false",
            "MELODEE__INGEST__SOURCES_JSON": json.dumps(
                [
                    {
                        "id": FIXTURE_LIBRARY_ID,
                        "root": str(inbound_root),
                        "name": catalog["library"]["name"],
                    }
                ]
            ),
            "MELODEE__NATIVE__AUTH__JWT_KEY": oracle.FIXTURE_JWT_KEY,
            # The Phase 0 capture defeated the localhost auth bypass with
            # X-Forwarded-For: 203.0.113.7; honor it by trusting the loopback
            # proxy for this fixture only.
            "MELODEE__NATIVE__TRUSTED_PROXIES": "127.0.0.1",
            "MELODEE__PODCAST__ENABLED": "true" if podcasts_enabled else "false",
            "MELODEE__JELLYFIN__TOKEN_PEPPER": "phase8-differential-fixture-pepper-NOT-A-SECRET",
            "RUST_LOG": "warn",
        }

    def start_server(podcasts_enabled: bool, log_name: str) -> subprocess.Popen:
        log_path = work_dir / log_name
        process = subprocess.Popen(
            [str(ROOT / "target" / "debug" / "melodee-server")],
            cwd=ROOT,
            env=server_env(podcasts_enabled),
            stdout=log_path.open("w"),
            stderr=subprocess.STDOUT,
        )
        wait_ready(base_url, process)
        return process

    def stop_server(process: subprocess.Popen) -> None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

    def reseed() -> None:
        """Mirror the legacy capture pipeline: every scenario file sees a
        pristine fixture (the oracle reseeded between files)."""
        wipe = subprocess.run(
            [
                "psql", "-h", "127.0.0.1", "-p", "5433", "-U", "melodee", "-At",
                "-d", args.db_name,
                "-c",
                "SELECT 'TRUNCATE ' || string_agg(format('%I', tablename), ', ')"
                " || ' CASCADE' FROM pg_tables WHERE schemaname = 'public'"
                " AND tablename <> '_sqlx_migrations'",
            ],
            env={**os.environ, "PGPASSWORD": "melodee-dev"},
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        if wipe:
            psql(wipe, database=args.db_name)
        subprocess.run(
            [
                "psql", "-h", "127.0.0.1", "-p", "5433", "-U", "melodee",
                "-v", "ON_ERROR_STOP=1", "-d", args.db_name,
                "-f", str(work_dir / "seed.sql"),
            ],
            env={**os.environ, "PGPASSWORD": "melodee-dev"},
            check=True,
            capture_output=True,
            text=True,
        )

    unexpected: list[tuple[str, str, object, object]] = []
    known: list[tuple[str, str, str]] = []

    def run_group(surface: str, golden_root: Path, scenario_file: str) -> None:
        scenario_path = ROOT / "compatibility" / "scenarios" / scenario_file
        group = scenario_file.removeprefix(f"{surface}-").removesuffix(".json")
        golden_dir = golden_root / group
        output_dir = captures_dir / surface / group
        if not golden_dir.is_dir():
            print(f"skip {scenario_file}: no golden dir {golden_dir}")
            return
        reseed()
        # Drive the capturer in-process so intentional-failure classes
        # (e.g. the absent legacy Blazor shell returning 404 where the
        # scenario expects the 200 HTML) still produce comparable captures
        # instead of aborting the run.
        output_dir.mkdir(parents=True, exist_ok=True)
        scenarios = json.loads(scenario_path.read_text(encoding="utf-8"))
        for key, value in capture_env().items():
            if key.startswith(("PHASE0_", "LEGACY_")):
                os.environ[key] = value
        for scenario in scenarios:
            # Per-step capture (capture_scenario inlined): a step whose
            # expectStatus assertion fails — e.g. the intentional D4
            # playlist-add divergence where legacy's frozen 500 is
            # deliberately not preserved — must not abort the rest of the
            # flow; the golden diff classifies the step instead.
            variables: dict[str, str] = {"baseUrl": base_url.rstrip("/")}
            step_secrets: dict[str, str] = {}
            for step_name, step in capture_baseline.iter_steps(scenario):
                try:
                    result = capture_baseline.capture_request(
                        base_url,
                        step_name,
                        step,
                        output_dir,
                        variables,
                        step_secrets,
                        None,
                    )
                except SystemExit as error:
                    print(f"  {step_name}: capture assertion failed ({error})")
                    continue
                except KeyError as missing:
                    result = {
                        "name": step_name,
                        "skipped": True,
                        "reason": str(missing),
                    }
                status = (
                    "skipped"
                    if result.get("skipped")
                    else result.get("response", {}).get("status")
                )
                print(f"  {result['name']}: {status}")
        differences = compare_captures.compare(golden_dir, output_dir)
        for difference in differences:
            reason = is_known_difference(
                difference.scenario,
                difference.field,
                difference.expected,
                difference.actual,
            )
            if reason is None:
                unexpected.append(
                    (difference.scenario, difference.field, difference.expected, difference.actual)
                )
            else:
                known.append((difference.scenario, difference.field, reason))
        print(f"{scenario_file}: {len(differences)} difference(s)")

    groups = {args.only: SCENARIO_GROUPS[args.only]} if args.only else SCENARIO_GROUPS
    servers: list[subprocess.Popen] = []
    try:
        # Migrations run at server boot. Main phase: podcasts enabled.
        servers.append(start_server(True, "server-enabled.log"))
        for surface, (golden_root, scenario_files) in groups.items():
            for scenario_file in scenario_files:
                if scenario_file == "opensubsonic-conditional-off.json":
                    continue
                run_group(surface, golden_root, scenario_file)
        stop_server(servers.pop())
        # Conditional-off phase: podcasts disabled (captured in the
        # podcast-disabled phase of the legacy oracle).
        if any(
            "opensubsonic-conditional-off.json" in files
            for _, files in groups.values()
        ):
            servers.append(start_server(False, "server-disabled.log"))
            run_group(
                "opensubsonic",
                SCENARIO_GROUPS["opensubsonic"][0],
                "opensubsonic-conditional-off.json",
            )
            stop_server(servers.pop())
    finally:
        for process in servers:
            stop_server(process)
        if not args.keep:
            psql(f'DROP DATABASE IF EXISTS "{args.db_name}" WITH (FORCE);')
            shutil.rmtree(work_dir, ignore_errors=True)

    print(f"\nknown intentional differences: {len(known)}")
    for scenario, field, reason in known:
        print(f"  [known] {scenario}: {field} — {reason}")
    # Reports land in the gitignored captures tree so a non---keep run still
    # leaves inspectable evidence without polluting the repository.
    report_dir = ROOT / "compatibility" / "captures" / "phase8-differential"
    report_dir.mkdir(parents=True, exist_ok=True)
    report_path = report_dir / "differential-report.json"
    report_path.write_text(
        json.dumps(
            {
                "baseUrl": base_url,
                "unexpected": [
                    {"scenario": s, "field": f, "expected": e, "actual": a}
                    for s, f, e, a in unexpected
                ],
                "known": [
                    {"scenario": s, "field": f, "reason": r} for s, f, r in known
                ],
            },
            indent=2,
            default=str,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"full report: {report_path}")
    if unexpected:
        print(f"\nUNEXPECTED differences: {len(unexpected)}")
        for scenario, field, expected, actual in unexpected[:50]:
            print(f"  {scenario}: {field}\n    expected: {expected}\n    actual:   {actual}")
        raise SystemExit(1)
    print("\nAll captured scenarios match the frozen goldens after normalization and the enumerated intentional differences.")


if __name__ == "__main__":
    main()
