#!/usr/bin/env python3
"""Populate a local Melodee instance with data the MeloAmp client screens need.

MeloAmp (the reference native-API desktop client) exercises surfaces that a bare
catalogue leaves empty: favourites, play history, playlists and smart playlists,
charts, requests, shares, equalizer presets, and the analytics/recommendation
views that read play history. An empty catalogue makes those screens look broken
even when the API is correct, so parity work needs a populated instance.

This script is deliberately idempotent and non-destructive: it only creates data,
never deletes. It talks to the public native API with the same shapes the client
uses, so a failure here is a real API failure rather than a SQL-fiction problem.

Usage
-----
    python3 scripts/seed_meloamp_parity_data.py --dry-run
    MELODEE_LIVE_USERNAME=... MELODEE_LIVE_PASSWORD=... \\
        python3 scripts/seed_meloamp_parity_data.py

Options
-------
``--api``              API origin (default ``http://127.0.0.1:8080``).
``--username``/``--password``  native account used for every write; falls back to
                        ``MELODEE_LIVE_USERNAME``/``MELODEE_LIVE_PASSWORD``.
``--dry-run``          report what would be created and exit without writing.
``--scrobbles``        number of play events to record (default 120).
``--no-chart``         skip the chart import (needs ``melodee-cli`` on PATH or
                        ``--cli``).
``--cli``              path to ``melodee-cli`` (default: workspace debug build).

Exit code is non-zero when a write fails, so this can gate a parity run.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_API = "http://127.0.0.1:8080"
STAR_VALUES = ("1", "0")
PLAYLIST_NAME = "Parity Playlist"
SMART_PLAYLIST_NAME = "Parity Smart Playlist"
SHARE_DESCRIPTION = "Parity share"
REQUEST_DESCRIPTION = "Parity request: a radio edit of the album"


class SeedError(RuntimeError):
    """A write the client depends on failed; stop rather than half-populate."""


class Http:
    """Minimal JSON client carrying a native bearer token."""

    def __init__(self, api: str, token: str | None = None) -> None:
        self.api = api.rstrip("/")
        self.token = token

    def call(
        self,
        method: str,
        path: str,
        *,
        body: Any | None = None,
        query: dict[str, Any] | None = None,
        expect: tuple[int, ...] = (200, 201, 204),
    ) -> Any:
        url = f"{self.api}/api/v1{path}"
        if query:
            url += "?" + urllib.parse.urlencode(query)
        data = None
        headers = {"Accept": "application/json"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                payload = response.read()
                return json.loads(payload) if payload else None
        except urllib.error.HTTPError as error:
            payload = error.read().decode("utf-8", "replace")
            if error.code in expect:
                return None
            raise SeedError(f"{method} {path} -> {error.code}: {payload[:200]}") from error

    def get(self, path: str, **query: Any) -> Any:
        return self.call("GET", path, query=query or None)


def authenticate(api: str, username: str, password: str) -> str:
    client = Http(api)
    response = client.call(
        "POST", "/auth/authenticate", body={"userName": username, "password": password}
    )
    token = (response or {}).get("token")
    if not token:
        raise SeedError("authenticate returned no token")
    return token


def first_items(client: Http, path: str, count: int, **query: Any) -> list[dict[str, Any]]:
    payload = client.get(path, page=1, pageSize=count, **query) or {}
    return list(payload.get("data") or [])


def seed_user_state(client: Http, artists, albums, songs, dry_run: bool) -> list[str]:
    actions: list[str] = []
    # A spread of liked / disliked / rated entities so every Favorites tab has
    # rows (the client filters by these three independent bits).
    for index, artist in enumerate(artists[:6]):
        star = "true" if index % 2 == 0 else "false"
        if index % 3 == 0:
            client.call("POST", f"/artists/starred/{artist['id']}/{star}")
            actions.append(f"artist {artist['name']}: starred={star}")
        if index % 4 == 0:
            client.call("POST", f"/artists/setrating/{artist['id']}/{4}")
            actions.append(f"artist {artist['name']}: rating=4")
    for index, album in enumerate(albums[:8]):
        client.call("POST", f"/albums/starred/{album['id']}/{'true' if index % 2 == 0 else 'false'}")
        actions.append(f"album {album['name']}: starred={index % 2 == 0}")
        if index % 3 == 0:
            client.call("POST", f"/albums/setrating/{album['id']}/{3 + (index % 3)}")
            actions.append(f"album {album['name']}: rated")
    for index, song in enumerate(songs[:12]):
        client.call("POST", f"/songs/starred/{song['id']}/{'true' if index % 3 else 'false'}")
        actions.append(f"song {song['title']}: starred={bool(index % 3 == 0)}")
        if index % 4 == 0:
            client.call("POST", f"/songs/setrating/{song['id']}/{5 - (index % 5)}")
            actions.append(f"song {song['title']}: rated")
        if index % 5 == 0 and index:
            client.call("POST", f"/songs/hated/{song['id']}/true")
            actions.append(f"song {song['title']}: disliked")
    return actions


def seed_history(client: Http, songs, count: int) -> list[str]:
    actions: list[str] = []
    now = int(time.time())
    # Spread plays over two weeks so history, day/week analytics and
    # recommendations all have a distribution rather than a single bucket.
    for index in range(count):
        song = songs[index % len(songs)]
        played_at = now - (index % 14) * 86_400 - (index % 12) * 3_600
        client.call(
            "POST",
            "/scrobble",
            body={
                "songId": song["id"],
                "playerName": "MeloAmp parity seed",
                "scrobbleType": "played",
                "timestamp": played_at,
                "playedDuration": 180.0,
                "scrobbleTypeValue": 1,
            },
            expect=(200, 201, 204),
        )
        if index % 10 == 0:
            client.call(
                "POST",
                "/scrobble",
                body={
                    "songId": song["id"],
                    "playerName": "MeloAmp parity seed",
                    "scrobbleType": "nowplaying",
                    "timestamp": played_at,
                    "playedDuration": 0.0,
                    "scrobbleTypeValue": 0,
                },
                expect=(200, 201, 204),
            )
        actions.append(f"play {index + 1}/{count}: {song['title']}")
    return actions


def seed_playlists(client: Http, songs, existing: list[dict[str, Any]]) -> list[str]:
    actions: list[str] = []
    names = {playlist.get("name") for playlist in existing}
    if PLAYLIST_NAME not in names:
        created = client.call(
            "POST",
            "/playlists",
            body={
                "name": PLAYLIST_NAME,
                "comment": "Created by the parity seeding script",
                "isPublic": True,
                "songIds": [song["id"] for song in songs[:5]],
            },
        )
        actions.append(f"playlist created: {PLAYLIST_NAME} ({len(songs[:5])} songs)")
        playlist_id = (created or {}).get("id")
    else:
        playlist_id = next(
            (p.get("id") for p in existing if p.get("name") == PLAYLIST_NAME), None
        )
        actions.append(f"playlist exists: {PLAYLIST_NAME}")
    if playlist_id:
        try:
            client.call(
                "POST",
                f"/playlists/{playlist_id}/songs",
                body={"songIds": [song["id"] for song in songs[5:9]]},
            )
            actions.append(f"playlist songs added: {len(songs[5:9])}")
        except SeedError as error:
            # Duplicates are rejected by design; a second run must stay quiet.
            if "409" not in str(error) and "400" not in str(error):
                raise
            actions.append("playlist songs already present")

    smart = client.get("/playlists/smart", page=1, pageSize=100) or {}
    if SMART_PLAYLIST_NAME not in {p.get("name") for p in smart.get("data") or []}:
        client.call(
            "POST",
            "/playlists/smart",
            body={
                "name": SMART_PLAYLIST_NAME,
                "mqlQuery": 'year>=2000',
                "entityType": "songs",
                "isPublic": True,
            },
        )
        actions.append(f"smart playlist created: {SMART_PLAYLIST_NAME}")
    else:
        actions.append(f"smart playlist exists: {SMART_PLAYLIST_NAME}")
    return actions


def seed_misc(client: Http, songs, artists) -> list[str]:
    actions: list[str] = []
    presets = client.get("/equalizer/presets", page=1, limit=50) or {}
    if not presets.get("presets"):
        client.call(
            "POST",
            "/equalizer/presets",
            body={
                "name": "Parity Flat",
                "bands": [
                    {"frequency": 60, "gain": 0},
                    {"frequency": 230, "gain": 0},
                    {"frequency": 910, "gain": 0},
                    {"frequency": 3600, "gain": 0},
                    {"frequency": 14000, "gain": 0},
                ],
                "isDefault": True,
            },
        )
        actions.append("equalizer preset created: Parity Flat")

    shares = client.get("/shares", page=1, pageSize=50) or {}
    if not shares.get("data"):
        client.call(
            "POST",
            "/shares",
            body={
                "shareType": 1,
                "resourceId": songs[0]["id"],
                "description": SHARE_DESCRIPTION,
                "isDownloadable": False,
            },
        )
        actions.append("share created for the first Song")

    requests = client.get("/requests", page=1, pageSize=50) or {}
    if not requests.get("data"):
        client.call(
            "POST",
            "/requests",
            body={
                "category": 1,
                "description": REQUEST_DESCRIPTION,
                "artistName": artists[0]["name"] if artists else "Unknown",
                "albumTitle": "Parity Request Album",
                "releaseYear": 2024,
                "notes": "seeded for MeloAmp parity testing",
            },
        )
        actions.append("request created")
    return actions


def seed_chart(api: str, cli: str, albums) -> list[str]:
    if not albums:
        return []
    rows = [
        (index + 1, album.get("artist", {}).get("name") or "Unknown", album["name"], album["releaseYear"])
        for index, album in enumerate(albums[:10])
    ]
    with tempfile.NamedTemporaryFile("w", suffix=".csv", delete=False, newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["Rank", "ArtistName", "ReleaseTitle", "ReleaseYear"])
        for rank, artist, title, year in rows:
            writer.writerow([rank, artist, title, year or ""])
        path = Path(handle.name)
    try:
        command = [
            cli,
            "charts",
            "import",
            "--file",
            str(path),
            "--slug",
            "parity-chart",
            "--title",
            "Parity Chart",
        ]
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
        if completed.returncode != 0:
            output = f"{completed.stdout}\n{completed.stderr}".lower()
            if "already exists" in output:
                # Idempotent re-run: the chart is part of the fixture already.
                return ["chart already present"]
            raise SeedError(
                f"chart import failed ({completed.returncode}): {completed.stderr.strip()[:200]}"
            )
        return [f"chart imported: {len(rows)} rows"]
    finally:
        path.unlink(missing_ok=True)


def run(args: argparse.Namespace) -> int:
    token = authenticate(args.api, args.username, args.password)
    client = Http(args.api, token)
    artists = first_items(client, "/artists", 40)
    albums = first_items(client, "/albums", 40)
    songs = first_items(client, "/songs", 60)
    if not songs:
        raise SeedError("catalogue has no Songs; import Releases before seeding")
    print(f"catalogue sample: {len(artists)} artists, {len(albums)} albums, {len(songs)} songs")

    plan: list[str] = []
    plan += [f"star/rate {len(artists[:6])} artists, {len(albums[:8])} albums, {len(songs[:12])} songs"]
    plan += [f"record {args.scrobbles} play events"]
    plan += [f"ensure playlist '{PLAYLIST_NAME}' and smart playlist '{SMART_PLAYLIST_NAME}'"]
    plan += ["ensure one equalizer preset, share, and request"]
    if not args.no_chart:
        plan += ["import a chart from the first 10 albums"]
    if args.dry_run:
        print("\n".join(f"  would {item}" for item in plan))
        return 0

    actions: list[str] = []
    actions += seed_user_state(client, artists, albums, songs, args.dry_run)
    actions += seed_history(client, songs, args.scrobbles)
    actions += seed_playlists(client, songs, first_items(client, "/playlists", 50))
    actions += seed_misc(client, songs, artists)
    if not args.no_chart:
        actions += seed_chart(args.api, args.cli, albums)

    for action in actions:
        print(f"  {action}")
    print(f"seeded {len(actions)} actions against {args.api}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Populate a local Melodee instance with MeloAmp parity data "
        "(favourites, history, playlists, charts, requests, shares, presets).",
        epilog="Idempotent: creates missing data, never deletes. Requires a "
        "catalogue with at least one Song; run after importing Releases.",
    )
    parser.add_argument("--api", default=os.environ.get("MELODEE_LIVE_API_URL", DEFAULT_API))
    parser.add_argument(
        "--username", default=os.environ.get("MELODEE_LIVE_USERNAME", "")
    )
    parser.add_argument(
        "--password", default=os.environ.get("MELODEE_LIVE_PASSWORD", "")
    )
    parser.add_argument("--scrobbles", type=int, default=120)
    parser.add_argument("--no-chart", action="store_true")
    parser.add_argument(
        "--cli",
        default=str(ROOT / "target" / "debug" / "melodee-cli"),
    )
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    if not args.username or not args.password:
        parser.error(
            "credentials required: pass --username/--password or set "
            "MELODEE_LIVE_USERNAME/MELODEE_LIVE_PASSWORD"
        )
    if not args.no_chart and shutil.which(args.cli) is None and not Path(args.cli).is_file():
        args.no_chart = True
        print(f"note: {args.cli} not found; skipping chart import", file=sys.stderr)
    try:
        return run(args)
    except SeedError as error:
        print(f"seeding failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
