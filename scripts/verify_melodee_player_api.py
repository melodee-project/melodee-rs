#!/usr/bin/env python3
"""Verify the native-v1 API contract used by the Melodee Android / Android Auto player.

`melodee-player` is the reference Android client: a Kotlin/Compose app with a
Media3 playback service and a `MediaBrowserServiceCompat` for Android Auto. Its
whole server surface is the `MusicApi` + `ScrobbleApi` Retrofit interfaces, so
"the app works" reduces to "these requests succeed and deserialize into its
models". This script performs exactly those requests and checks the model
invariants the app relies on.

What is asserted
----------------
* every request the app makes (13 endpoints) answers 2xx for a listener account;
* the fields the app's Kotlin models declare **non-null** are present — a missing
  field becomes `null`/`0` in Gson and shows up as blank UI, not an error;
* the authentication lifecycle the app implements works: a stale access token is
  rejected, `POST /auth/refresh-token` returns a usable pair, and the new token
  authorizes the retried request;
* paging, ordering, and the artist filter the app sends are honoured;
* the issued `streamUrl` really streams, and honours an HTTP `Range` request
  (ExoPlayer seeking);
* favourites and scrobbles accept the app's exact payload shapes, including the
  integer `scrobbleTypeValue` enum member.

Usage
-----
    python3 scripts/verify_melodee_player_api.py --dry-run
    MELODEE_LIVE_USERNAME=... MELODEE_LIVE_PASSWORD=... \\
        python3 scripts/verify_melodee_player_api.py

Exit code is non-zero when any contract check fails, so it can gate a parity run.
Android Auto itself cannot be exercised without a device; this covers the
transport contract the app and its car surface share.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from typing import Any

DEFAULT_API = "http://127.0.0.1:8080"

# Non-null fields of the app's Kotlin models (`domain/model/Models.kt`).
SONG_FIELDS = (
    "id",
    "streamUrl",
    "title",
    "artist",
    "album",
    "thumbnailUrl",
    "imageUrl",
    "durationMs",
    "durationFormatted",
)
ARTIST_FIELDS = ("id", "name", "thumbnailUrl", "imageUrl")
ALBUM_FIELDS = ("id", "name", "thumbnailUrl", "imageUrl", "releaseYear")
PLAYLIST_FIELDS = (
    "id",
    "name",
    "imageUrl",
    "thumbnailUrl",
    "durationMs",
    "durationFormatted",
    "songCount",
)
USER_FIELDS = ("id", "email", "thumbnailUrl", "imageUrl", "username")
META_FIELDS = ("totalCount", "pageSize", "currentPage", "totalPages")


class Probe:
    """Runs the client's requests and records pass/fail per check."""

    def __init__(self, api: str) -> None:
        self.api = api.rstrip("/")
        self.results: list[tuple[str, bool, str]] = []

    def call(
        self,
        method: str,
        path: str,
        *,
        token: str | None = None,
        body: Any | None = None,
        headers: dict[str, str] | None = None,
        raw: bool = False,
    ) -> tuple[int, Any]:
        request = urllib.request.Request(f"{self.api}{path}", method=method)
        if token:
            request.add_header("Authorization", f"Bearer {token}")
        for name, value in (headers or {}).items():
            request.add_header(name, value)
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, data, timeout=60) as response:
                payload = response.read()
                if raw:
                    return response.status, payload
                return response.status, json.loads(payload) if payload else None
        except urllib.error.HTTPError as error:
            payload = error.read()
            if raw:
                return error.code, payload
            try:
                return error.code, json.loads(payload) if payload else None
            except json.JSONDecodeError:
                return error.code, payload[:200]

    def check(self, name: str, ok: bool, detail: str = "") -> bool:
        self.results.append((name, ok, detail))
        status = "PASS" if ok else "FAIL"
        print(f"{status}  {name}" + ("" if ok else f"  :: {detail}"))
        return ok

    def equals(self, name: str, actual: Any, expected: Any) -> bool:
        return self.check(name, actual == expected, f"expected {expected!r}, got {actual!r}")

    def has_fields(self, name: str, payload: dict[str, Any] | None, fields: tuple[str, ...]) -> bool:
        missing = [field for field in fields if not isinstance(payload, dict) or payload.get(field) is None]
        return self.check(name, not missing, f"missing/null: {missing}")

    def page(self, name: str, payload: dict[str, Any] | None) -> bool:
        ok = self.has_fields(f"{name}: envelope", payload, ("meta", "data"))
        if ok:
            ok = self.has_fields(f"{name}: pagination metadata", payload["meta"], META_FIELDS)
            ok = self.check(
                f"{name}: data is a list",
                isinstance(payload["data"], list),
                f"type={type(payload['data']).__name__}",
            ) and ok
        return ok


def first(payload: Any) -> dict[str, Any] | None:
    items = (payload or {}).get("data") or []
    return items[0] if items else None


def run(probe: Probe, username: str, password: str) -> int:
    # 1. Sign-in (app posts {userName, email, password}; either identifier works).
    status, auth = probe.call(
        "POST", "/api/v1/auth/authenticate", body={"userName": username, "password": password}
    )
    if not probe.check("POST /auth/authenticate", status == 200 and bool((auth or {}).get("token")), f"status={status} {auth}"):
        return 1
    probe.has_fields("authenticate: token/serverVersion/user", auth, ("token", "serverVersion", "user"))
    probe.has_fields("authenticate: user model", auth["user"], USER_FIELDS)
    token = auth["token"]

    # 2. Server info (the app gates login on the version numbers).
    status, info = probe.call("GET", "/api/v1/system/info")
    probe.check("GET /system/info", status == 200, f"status={status}")
    probe.has_fields("system/info: version fields", info or {}, ("majorVersion", "minorVersion"))

    # 3. Refresh lifecycle: stale access token -> refresh -> authorized retry.
    status, body = probe.call("GET", "/api/v1/user/me", token="stale-access-token")
    probe.check("stale access token is rejected (drives the app's refresh)",
                status == 401, f"status={status} body={body}")
    status, refreshed = probe.call(
        "POST", "/api/v1/auth/refresh-token",
        body={"refreshToken": auth.get("refreshToken"), "deviceId": "android-auto"},
    )
    usable = status == 200 and bool((refreshed or {}).get("token"))
    probe.check("POST /auth/refresh-token returns a usable pair", usable, f"status={status} {str(refreshed)[:160]}")
    if usable:
        token = refreshed["token"]
        status, me = probe.call("GET", "/api/v1/user/me", token=token)
        probe.check("refreshed token authorizes /user/me", status == 200, f"status={status}")
        probe.has_fields("user/me: user model", me or {}, USER_FIELDS)

    # 4. Playlists + playlist songs (Android Auto browse roots).
    status, playlists = probe.call("GET", "/api/v1/user/playlists?page=1&limit=50", token=token)
    probe.check("GET /user/playlists (page=1&limit=50)", status == 200, f"status={status}")
    probe.page("user/playlists", playlists)
    playlist = first(playlists)
    if playlist:
        probe.has_fields("playlist model", playlist, PLAYLIST_FIELDS)
        status, songs = probe.call(
            "GET", f"/api/v1/playlists/{playlist['id']}/songs?page=1&pageSize=50", token=token
        )
        probe.check("GET /playlists/{id}/songs", status == 200, f"status={status}")
        probe.page("playlist songs", songs)
    else:
        probe.check("playlist songs", False, "no playlist to test with")

    # 5. Catalogue browse: artists, artist albums/songs, album songs.
    status, artists = probe.call(
        "GET", "/api/v1/artists?page=1&pageSize=10&orderBy=name&orderDirection=asc", token=token
    )
    probe.check("GET /artists (q/orderBy/orderDirection)", status == 200, f"status={status}")
    probe.page("artists", artists)
    asc_ids = [artist["id"] for artist in (artists or {}).get("data", [])]
    status, artists_desc = probe.call(
        "GET", "/api/v1/artists?page=1&pageSize=10&orderBy=name&orderDirection=desc", token=token
    )
    desc_ids = [artist["id"] for artist in (artists_desc or {}).get("data", [])]
    probe.check("artist ordering is honoured", asc_ids and asc_ids != desc_ids, "asc == desc")

    status, albums = probe.call("GET", "/api/v1/albums?page=1&pageSize=5", token=token)
    probe.check("GET /albums", status == 200, f"status={status}")
    album = first(albums)
    artist = (album or {}).get("artist") or first(artists)
    if album:
        probe.has_fields("album model", album, ALBUM_FIELDS)
    if artist:
        probe.has_fields("artist model", artist, ARTIST_FIELDS)
        status, artist_songs = probe.call(
            "GET", f"/api/v1/artists/{artist['id']}/songs?page=1&pageSize=50", token=token
        )
        probe.check("GET /artists/{id}/songs", status == 200, f"status={status}")
        status, artist_albums = probe.call(
            "GET", f"/api/v1/artists/{artist['id']}/albums?page=1&pageSize=50", token=token
        )
        probe.check("GET /artists/{id}/albums", status == 200, f"status={status}")
    if album:
        # The app sends no paging parameters here.
        status, album_songs = probe.call("GET", f"/api/v1/albums/{album['id']}/songs", token=token)
        probe.check("GET /albums/{id}/songs (no paging params)", status == 200, f"status={status}")
        probe.page("album songs", album_songs)

    # 6. Search: the app's `q` search, an empty result, and the artist filter.
    status, hits = probe.call("GET", "/api/v1/search/songs?q=love&page=1&pageSize=50", token=token)
    probe.check("GET /search/songs?q=", status == 200, f"status={status}")
    probe.page("song search", hits)
    status, empty = probe.call(
        "GET", "/api/v1/search/songs?q=zzzznotarealsong&page=1&pageSize=50", token=token
    )
    probe.check(
        "empty search is an empty page, not an error",
        status == 200 and (empty or {}).get("data") == [] and (empty or {}).get("meta", {}).get("totalCount") == 0,
        f"status={status} {empty}",
    )
    if artist:
        status, filtered = probe.call(
            "GET",
            f"/api/v1/search/songs?q=&page=1&pageSize=25&filterByArtistApiKey={artist['id']}",
            token=token,
        )
        rows = (filtered or {}).get("data") or []
        probe.check(
            "search filterByArtistApiKey restricts results",
            status == 200 and all(row["artist"]["id"] == artist["id"] for row in rows),
            f"status={status} artists={[row['artist']['id'][:8] for row in rows][:4]}",
        )

    # 7. Playback: the issued streamUrl must stream, and honour Range (seeking).
    status, songs = probe.call("GET", "/api/v1/songs?page=1&pageSize=1", token=token)
    song = first(songs)
    if not probe.check("GET /songs", status == 200 and song is not None, f"status={status}"):
        return 1
    probe.has_fields("song model", song, SONG_FIELDS)
    status, audio = probe.call("GET", song["streamUrl"].replace(probe.api, ""), raw=True)
    probe.check(
        "streamUrl returns audio bytes",
        status == 200 and isinstance(audio, bytes) and len(audio) > 10_000,
        f"status={status} bytes={len(audio) if isinstance(audio, bytes) else audio}",
    )
    status, ranged = probe.call(
        "GET", song["streamUrl"].replace(probe.api, ""), headers={"Range": "bytes=0-1023"}, raw=True
    )
    probe.check(
        "stream honours HTTP Range (ExoPlayer seek)",
        status in (200, 206) and isinstance(ranged, bytes) and 0 < len(ranged) <= 1024 * 4,
        f"status={status} bytes={len(ranged) if isinstance(ranged, bytes) else ranged}",
    )

    # 8. User state: favourite toggle and the app's scrobble payload (int enum).
    status, _ = probe.call("POST", f"/api/v1/songs/starred/{song['id']}/true", token=token)
    probe.check("POST /songs/starred/{id}/true", status in (200, 204), f"status={status}")
    status, _ = probe.call("POST", f"/api/v1/songs/starred/{song['id']}/false", token=token)
    probe.check("POST /songs/starred/{id}/false", status in (200, 204), f"status={status}")
    now = time.time()
    status, body = probe.call(
        "POST", "/api/v1/scrobble",
        token=token,
        body={
            "songId": song["id"],
            "playerName": "MelodeePlayer",
            "scrobbleTypeValue": 2,
            "scrobbleType": "played",
            "timestamp": now,
            "playedDuration": 30.0,
        },
    )
    probe.check("POST /scrobble (played, int scrobbleTypeValue)", status in (200, 204), f"status={status} {body}")
    status, body = probe.call(
        "POST", "/api/v1/scrobble",
        token=token,
        body={
            "songId": song["id"],
            "playerName": "MelodeePlayer",
            "scrobbleTypeValue": 1,
            "scrobbleType": "nowPlaying",
            "timestamp": now,
            "playedDuration": 0.0,
        },
    )
    probe.check("POST /scrobble (nowPlaying)", status in (200, 204), f"status={status} {body}")

    passed = sum(1 for _, ok, _ in probe.results if ok)
    total = len(probe.results)
    gaps = [(name, detail) for name, ok, detail in probe.results if not ok]
    print(f"\n{passed}/{total} android-client contract checks passed")
    for name, detail in gaps:
        print(f"  gap: {name} :: {detail}")
    return 0 if passed == total else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Verify the native-v1 contract used by the Melodee Android / Android Auto client.",
        epilog="Requires a running instance with at least one Release and one playlist. "
        "Android Auto's car surface shares this transport contract; driving the car UI "
        "itself needs a device or emulator.",
    )
    parser.add_argument("--api", default=os.environ.get("MELODEE_LIVE_API_URL", DEFAULT_API))
    parser.add_argument("--username", default=os.environ.get("MELODEE_LIVE_USERNAME", ""))
    parser.add_argument("--password", default=os.environ.get("MELODEE_LIVE_PASSWORD", ""))
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="list the checks that would run without contacting the server",
    )
    parser.add_argument("--json", action="store_true", help="emit the results as JSON")
    args = parser.parse_args(argv)

    if args.dry_run:
        print("would verify against " + args.api)
        for line in (
            "POST /auth/authenticate (+ model fields)",
            "GET /system/info",
            "401 on a stale token, then POST /auth/refresh-token and an authorized retry",
            "GET /user/playlists and GET /playlists/{id}/songs (+ paging)",
            "GET /artists with ordering, /artists/{id}/songs, /artists/{id}/albums",
            "GET /albums/{id}/songs (no paging params)",
            "GET /search/songs (query, empty result, filterByArtistApiKey)",
            "streamUrl playback + HTTP Range",
            "POST /songs/starred/{id}/{value}",
            "POST /scrobble with the app's integer scrobbleTypeValue",
        ):
            print(f"  {line}")
        return 0

    if not args.username or not args.password:
        parser.error(
            "credentials required: pass --username/--password or set "
            "MELODEE_LIVE_USERNAME/MELODEE_LIVE_PASSWORD"
        )

    probe = Probe(args.api)
    exit_code = run(probe, args.username, args.password)
    if args.json:
        print(
            json.dumps(
                {
                    "api": args.api,
                    "checks": len(probe.results),
                    "failures": [
                        {"check": name, "detail": detail}
                        for name, ok, detail in probe.results
                        if not ok
                    ],
                },
                indent=2,
            )
        )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
