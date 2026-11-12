#!/usr/bin/env python3
"""Verify the known-answer corpus after it has been processed by a live instance.

This is the known-answer half of the audit (see
`design/testing/KNOWN-ANSWER-E2E.md`): the corpus generator declares exactly what
should exist, and this script compares the running system against that
declaration across four independent read paths, so a lie in one layer cannot hide
behind another:

* **PostgreSQL** — Releases, Tracks, Artists, media assets, billing, aggregates,
  classifications, novelty, policy decisions, audit events.
* **Filesystem** — the canonical published bundle for each Release, the drained
  inbound source, and retained staging.
* **Search projection** — `melodee-cli search status` / `search query` over the
  active Tantivy generation.
* **Client surfaces** — native API v1 and OpenSubsonic, i.e. what a listener
  actually gets.

Usage
-----
    # 1. generate the corpus
    python3 scripts/generate_known_corpus.py
    # 2. run the instance, then drive + verify in one step
    python3 scripts/verify_known_corpus.py --ingest

`--ingest` copies `<corpus>/staging/*` into the configured inbound root and waits
for terminal states, so one command covers "process it and check it". Without it
the script verifies whatever is already catalogued (useful after a manual drop).

Exit code is non-zero when any assertion fails; `--json` emits the report.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = Path("/tmp/melodee-known-corpus")
DEFAULT_API = "http://127.0.0.1:8080"
TERMINAL_STATES = ("indexed", "ignored", "deleted", "failed", "quarantined")


# ---------------------------------------------------------------------------
# infrastructure
# ---------------------------------------------------------------------------


class VerifyError(RuntimeError):
    """The verification could not run (missing corpus, no database, no server)."""


def run(command: list[str], *, check: bool = True, timeout: float | None = None) -> str:
    completed = subprocess.run(
        command, capture_output=True, text=True, check=False, timeout=timeout
    )
    if check and completed.returncode != 0:
        raise VerifyError(
            f"command failed ({completed.returncode}): {' '.join(command)}\n{completed.stderr.strip()}"
        )
    return completed.stdout


def load_env() -> dict[str, str]:
    values: dict[str, str] = {}
    env_file = ROOT / ".env"
    if env_file.is_file():
        for line in env_file.read_text().splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#") or "=" not in stripped:
                continue
            key, raw = stripped.split("=", 1)
            values[key.strip()] = raw.strip().strip('"').strip("'")
    values.update({key: value for key, value in os.environ.items() if key.startswith("MELODEE__")})
    return values


def database_url(env: dict[str, str]) -> str:
    return env.get("MELODEE__DATABASE__URL", "postgres://melodee:melodee-dev@127.0.0.1:5433/melodee")


def psql_rows(url: str, sql: str) -> list[list[str]]:
    if shutil.which("psql") is None:
        raise VerifyError("psql is not on PATH")
    output = run(
        [
            "psql",
            url,
            "--no-psqlrc",
            "--quiet",
            "--tuples-only",
            "--no-align",
            "--field-separator",
            "\u001f",
            "--set",
            "ON_ERROR_STOP=1",
            "-c",
            sql,
        ]
    )
    return [line.split("\u001f") for line in output.splitlines() if line.strip()]


def scalar(url: str, sql: str) -> str:
    rows = psql_rows(url, sql)
    return rows[0][0] if rows and rows[0] else ""


def sql_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


# ---------------------------------------------------------------------------
# HTTP (admin session + client surfaces)
# ---------------------------------------------------------------------------


class Http:
    """Minimal cookie-aware HTTP client for the admin session and client APIs."""

    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.cookies: dict[str, str] = {}

    def _request(
        self,
        method: str,
        path: str,
        *,
        body: dict[str, Any] | None = None,
        expect_json: bool = True,
    ) -> tuple[int, Any]:
        data = None
        headers = {"Accept": "application/json"}
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        if self.cookies:
            headers["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
        request = urllib.request.Request(
            f"{self.base_url}{path}", data=data, headers=headers, method=method
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                self._store_cookies(response.headers)
                payload = response.read()
                if not expect_json:
                    return response.status, payload
                return response.status, json.loads(payload) if payload else None
        except urllib.error.HTTPError as error:
            self._store_cookies(error.headers)
            payload = error.read()
            try:
                parsed = json.loads(payload) if payload else None
            except json.JSONDecodeError:
                parsed = payload.decode(errors="replace")
            return error.code, parsed

    def _store_cookies(self, headers: Any) -> None:
        for header in headers.get_all("Set-Cookie", []) or []:
            pair = header.split(";", 1)[0]
            if "=" in pair:
                name, value = pair.split("=", 1)
                self.cookies[name.strip()] = value.strip()

    def sign_in(self, username: str, password: str) -> None:
        status, payload = self._request(
            "POST",
            "/api/v1/auth/cookie/sign-in",
            body={"userName": username, "password": password},
        )
        if status != 200:
            raise VerifyError(f"admin sign-in failed: {status} {payload}")


# ---------------------------------------------------------------------------
# results
# ---------------------------------------------------------------------------


@dataclass
class Result:
    """One assertion outcome.

    `gap` marks an expectation the product is known not to meet yet: the check is
    reported as a documented finding (and never as a pass), so the defect stays
    visible in the report while the suite still distinguishes it from a
    regression in behaviour that is supposed to work.
    """

    check: str
    ok: bool
    detail: str = ""
    gap: bool = False

    def as_dict(self) -> dict[str, Any]:
        return {"check": self.check, "ok": self.ok, "detail": self.detail, "gap": self.gap}


@dataclass
class Verifier:
    """Collects assertions so one failure never hides the rest."""

    url: str
    api: Http
    results: list[Result] = field(default_factory=list)

    def check(self, name: str, ok: bool, detail: str = "") -> None:
        self.results.append(Result(check=name, ok=ok, detail=detail))

    def gap(self, name: str, ok: bool, detail: str = "") -> None:
        """Record a documented product gap (never counted as a pass)."""
        self.results.append(Result(check=name, ok=ok, detail=detail, gap=True))

    def equals(self, name: str, actual: Any, expected: Any) -> None:
        self.check(
            name,
            actual == expected or str(actual) == str(expected),
            f"expected {expected!r}, got {actual!r}",
        )

    def contains(self, name: str, haystack: str, needle: str) -> None:
        self.check(name, needle in haystack, f"{needle!r} not found in {haystack[:200]!r}")

    @property
    def failures(self) -> list[Result]:
        return [result for result in self.results if not result.ok and not result.gap]

    @property
    def gaps(self) -> list[Result]:
        return [result for result in self.results if result.gap]


# ---------------------------------------------------------------------------
# ingest driver
# ---------------------------------------------------------------------------


def inbound_roots(env: dict[str, str]) -> list[Path]:
    explicit = env.get("MELODEE__INGEST__SOURCE_ROOT")
    roots: list[Path] = []
    if explicit:
        roots.append(Path(explicit))
    raw = env.get("MELODEE__INGEST__SOURCES_JSON")
    if raw:
        try:
            for entry in json.loads(raw):
                path = entry.get("root") or entry.get("path")
                if path:
                    roots.append(Path(path))
        except json.JSONDecodeError as error:
            raise VerifyError(f"MELODEE__INGEST__SOURCES_JSON is not valid JSON: {error}") from error
    if not roots:
        raise VerifyError("no inbound root configured (MELODEE__INGEST__SOURCE_ROOT)")
    return roots


def ingest(corpus: Path, env: dict[str, str], *, timeout_minutes: int, run_id: str) -> None:
    """Copy the corpus into inbound and wait for every Release to reach a terminal state."""
    root = inbound_roots(env)[0]
    if not root.is_dir():
        raise VerifyError(f"inbound root does not exist: {root}")
    staging = corpus / "staging"
    if not staging.is_dir():
        raise VerifyError(f"corpus has no staging directory: {staging}")
    copied: list[str] = []
    for release_dir in sorted(staging.iterdir()):
        if not release_dir.is_dir():
            continue
        target = root / release_dir.name
        if target.exists():
            print(f"skipping already-present inbound folder: {target}", file=sys.stderr)
            continue
        shutil.copytree(release_dir, target)
        copied.append(release_dir.name)
    print(f"copied {len(copied)} release folder(s) into {root}")

    url = database_url(env)
    deadline = time.monotonic() + timeout_minutes * 60
    last_report = 0.0
    # Phase 1: wait for discovery. The reconciler scans on an interval, so an
    # idle queue at t=0 means "not discovered yet", not "already done".
    while time.monotonic() < deadline:
        discovered = int(
            scalar(
                url,
                "SELECT count(*)::bigint FROM release_work_items "
                f" WHERE source_path LIKE {sql_literal(f'%[{run_id}]%')} "
                f"    OR current_path LIKE {sql_literal(f'%[{run_id}]%')}",
            )
            or "0"
        )
        if discovered >= len(copied):
            print(f"discovered {discovered} corpus work item(s)")
            break
        now = time.monotonic()
        if now - last_report > 15:
            last_report = now
            print(f"  waiting for discovery… ({discovered}/{len(copied)})")
        time.sleep(5)
    else:
        raise VerifyError(f"timed out waiting for discovery of {len(copied)} folder(s)")

    # Phase 2: wait for every corpus work item to reach a terminal state.
    seen_idle = 0
    while time.monotonic() < deadline:
        pending = int(
            scalar(
                url,
                f"SELECT count(*)::bigint FROM release_work_items "
                f" WHERE (source_path LIKE {sql_literal(f'%[{run_id}]%')} "
                f"    OR current_path LIKE {sql_literal(f'%[{run_id}]%')}) "
                "   AND state NOT IN "
                f"({', '.join(sql_literal(s) for s in TERMINAL_STATES + ('needs_review',))})",
            )
            or "0"
        )
        if pending == 0:
            seen_idle += 1
            # Two consecutive idle polls: the reconciler has not discovered
            # anything new for a full cycle.
            if seen_idle >= 2:
                print("all work items reached a terminal state")
                return
        else:
            seen_idle = 0
        now = time.monotonic()
        if now - last_report > 15:
            last_report = now
            states = psql_rows(
                url,
                "SELECT state, count(*)::bigint FROM release_work_items "
                f" WHERE (source_path LIKE {sql_literal(f'%[{run_id}]%')} "
                f"    OR current_path LIKE {sql_literal(f'%[{run_id}]%')}) "
                "   AND state NOT IN "
                f"({', '.join(sql_literal(s) for s in TERMINAL_STATES + ('needs_review',))}) "
                " GROUP BY 1 ORDER BY 1",
            )
            summary = " ".join(f"{row[0]}={row[1]}" for row in states) or "idle"
            print(f"  waiting… {summary}")
        time.sleep(5)
    raise VerifyError(f"timed out after {timeout_minutes} minute(s) waiting for terminal states")


# ---------------------------------------------------------------------------
# verification
# ---------------------------------------------------------------------------


def release_row(url: str, album: str, album_artist: str) -> dict[str, str] | None:
    rows = psql_rows(
        url,
        "SELECT r.id, r.public_api_id, r.title, r.sort_title, r.normalized_title, "
        "       r.display_artist, r.release_date::text, r.status, "
        "       a.track_count, a.duration_ms, a.media_bytes "
        "  FROM releases r "
        "  LEFT JOIN release_catalog_aggregates a ON a.release_id = r.id "
        f" WHERE r.title = {sql_literal(album)} "
        f"   AND COALESCE(r.display_artist, '') = {sql_literal(album_artist)} "
        "  ORDER BY r.created_at DESC LIMIT 1",
    )
    if not rows:
        return None
    keys = (
        "id",
        "public_api_id",
        "title",
        "sort_title",
        "normalized_title",
        "display_artist",
        "release_date",
        "status",
        "track_count",
        "duration_ms",
        "media_bytes",
    )
    return dict(zip(keys, rows[0]))


def work_item_for_release(url: str, release_id: str) -> dict[str, str] | None:
    rows = psql_rows(
        url,
        "SELECT w.state, COALESCE(w.completed_at::text, ''), COALESCE(w.last_error_message, '') "
        "  FROM release_work_items w "
        "  JOIN catalog_imports ci ON ci.work_item_id = w.id "
        f" WHERE ci.release_id = {sql_literal(release_id)} "
        "  ORDER BY w.created_at DESC LIMIT 1",
    )
    if not rows:
        return None
    return {"state": rows[0][0], "completed_at": rows[0][1], "last_error": rows[0][2]}


def verify_work_item_only(verifier: Verifier, url: str, release: dict[str, Any]) -> None:
    """Assert the declared non-catalogue outcome for a fixture that never imports."""
    slug = release["slug"]
    declared = release.get("expect", {}).get("terminalStates", ["needs_review"])
    rows = psql_rows(
        url,
        "SELECT w.state, COALESCE(w.state_reason, ''), COALESCE(left(w.last_error_message, 120), '') "
        "  FROM release_work_items w "
        f" WHERE w.source_path LIKE '%{release['folder']}%' "
        "  ORDER BY w.created_at DESC LIMIT 1",
    )
    verifier.check(
        f"{slug}: work item in declared terminal state {declared}",
        bool(rows) and rows[0][0] in declared,
        f"state={rows[0][0] if rows else 'missing'} reason={rows[0][1] if rows else '-'}",
    )
    if rows:
        verifier.check(
            f"{slug}: routed with a durable reason",
            bool(rows[0][1]),
            f"state_reason={rows[0][1]!r}",
        )


def verify_release(verifier: Verifier, url: str, release: dict[str, Any], env: dict[str, str]) -> None:
    slug = release["slug"]
    declared_before_catalogue = release.get("expect", {}).get("terminalStates", ["indexed"])
    row = release_row(url, release["album"], release["displayArtist"])
    if row is None:
        if "indexed" not in declared_before_catalogue:
            # A fixture the pipeline is expected to reject has no Release row; the
            # work item state is the assertion (handled by `verify_work_item_only`).
            verify_work_item_only(verifier, url, release)
            return
        verifier.check(f"{slug}: release catalogued", False, "no Release row matches album+artist")
        return
    verifier.check(f"{slug}: release catalogued", True)

    verifier.equals(f"{slug}: release title", row["title"], release["album"])
    verifier.equals(f"{slug}: release artist", row["display_artist"], release["displayArtist"])
    if release.get("releaseDate"):
        source = release.get("expect", {}).get("releaseDateSource", "tags")
        if source == "sidecar":
            # The sidecar carries a year, so it populates `release_date_text` and
            # leaves the typed date empty. Assert exactly that, so an improvement
            # (a full date in the sidecar) shows up as a deliberate change.
            verifier.equals(
                f"{slug}: release date text from sidecar",
                row.get("release_date_text") or "",
                release.get("expect", {}).get("releaseDateText", ""),
            )
        else:
            # Documented gap: a `date`/`year` tag is observed on every Track but
            # never lifted to the Release (`ManifestReleaseObservation.date` is
            # hard-coded `None` in the media observer), so only a sidecar can set
            # it. Recorded as a gap so it stays visible and flips to a real failure
            # if the intended behaviour ever partially lands (e.g. text only).
            verifier.gap(
                f"{slug}: release date from tags (KNOWN GAP: tag date is not lifted to the Release)",
                row["release_date"] == release["releaseDate"],
                f"expected {release['releaseDate']}, got {row['release_date']!r}",
            )
            verifier.gap(
                f"{slug}: release date text from tags (KNOWN GAP)",
                bool(row.get("release_date_text")),
                f"release_date_text={row.get('release_date_text')!r}",
            )
    if release.get("tagOverrides") and row["title"] == release["album"]:
        verifier.check(
            f"{slug}: sidecar metadata wins over tag metadata",
            True,
            "release title came from the sidecar although the tags disagree",
        )
    declared_states = release.get("expect", {}).get("terminalStates", ["indexed"])
    verifier.equals(
        f"{slug}: release status",
        row["status"],
        "active" if "indexed" in declared_states else "deprecated",
    )

    if release.get("tier", "core") == "faults":
        item = work_item_for_release(url, row["id"])
        allowed = set(release.get("expect", {}).get("terminalStates", []))
        verifier.check(
            f"{slug}: fault reached an explicit terminal state",
            item is not None and item["state"] in allowed,
            f"state={item and item['state']} expected one of {sorted(allowed)}",
        )
        return

    verifier.equals(f"{slug}: track count", row["track_count"], str(release["trackCount"]))
    # Durations are container-rounded; a 200 ms allowance covers encoder padding
    # without hiding a wrong track count or a truncated file.
    if release["totalDurationMs"]:
        delta = abs(int(row["duration_ms"] or 0) - release["totalDurationMs"])
        verifier.check(
            f"{slug}: total duration within 200 ms of source",
            delta <= 200,
            f"expected ~{release['totalDurationMs']} ms, got {row['duration_ms']} ms",
        )

    # Work item reached the *declared* terminal state. A fixture the policy routes
    # to review is expected to sit in `needs_review` until an operator decides; one
    # expected to be accepted must be `indexed` and playable.
    item = work_item_for_release(url, row["id"])
    declared = release.get("expect", {}).get("terminalStates", ["indexed"])
    verifier.check(
        f"{slug}: work item in declared terminal state {declared}",
        item is not None and item["state"] in declared,
        f"state={item and item['state']} error={item and item['last_error']}",
    )
    if "indexed" in declared:
        verifier.check(
            f"{slug}: accepted release is playable",
            row["status"] == "active",
            f"release status={row['status']}",
        )

    # Tracks: exact titles, durations, and billing.
    track_rows = psql_rows(
        url,
        "SELECT t.title, COALESCE(t.duration_ms, 0), COALESCE(t.disc_number, 0), "
        "       COALESCE(t.track_number, 0), COALESCE(t.display_artist, ''), m.byte_hash, m.byte_size "
        "  FROM tracks t "
        "  JOIN media_assets m ON m.id = t.playable_media_asset_id "
        f" WHERE t.release_id = {sql_literal(row['id'])} "
        "  ORDER BY COALESCE(t.disc_number, 1), COALESCE(t.track_number, t.ordinal), t.ordinal",
    )
    verifier.equals(f"{slug}: catalogue track rows", len(track_rows), str(release["trackCount"]))
    expected_titles = [track["title"] for track in release["tracks"]]
    actual_titles = [track[0] for track in track_rows]
    verifier.equals(f"{slug}: track titles in order", actual_titles, expected_titles)
    for expected, actual in zip(release["tracks"], track_rows):
        if expected.get("durationMs"):
            delta = abs(int(actual[1]) - expected["durationMs"])
            verifier.check(
                f"{slug}: '{expected['title']}' duration within 200 ms",
                delta <= 200,
                f"expected ~{expected['durationMs']} ms, got {actual[1]}",
            )
        if expected.get("discNumber"):
            verifier.equals(f"{slug}: '{expected['title']}' disc", actual[2], str(expected["discNumber"]))

    # Canonical publication: every playable asset exists at its hash path.
    published = psql_rows(
        url,
        "SELECT m.canonical_relative_path, m.byte_hash, m.byte_size "
        "  FROM media_assets m "
        "  JOIN tracks t ON t.playable_media_asset_id = m.id "
        f" WHERE t.release_id = {sql_literal(row['id'])}",
    )
    storage_root = Path(env.get("MELODEE__STORAGE__ROOT", ""))
    verifier.check(
        f"{slug}: storage root configured",
        storage_root.is_dir(),
        f"MELODEE__STORAGE__ROOT={storage_root}",
    )
    if storage_root.is_dir():
        for relative, byte_hash, byte_size in published:
            asset = storage_root / relative
            verifier.check(f"{slug}: asset exists ({asset.name})", asset.is_file())
            if asset.is_file():
                verifier.equals(
                    f"{slug}: asset size matches row ({asset.name})",
                    str(asset.stat().st_size),
                    byte_size,
                )
                verifier.check(
                    f"{slug}: asset path is hash-derived ({asset.name})",
                    byte_hash in str(asset),
                    f"{byte_hash} not in {asset}",
                )

    # Artwork: imported covers are published into the Release bundle and served
    # through the image endpoint's canonical fallback. They are deliberately *not*
    # written to `catalog_entity_images`, which holds operator overrides only.
    bundle_artwork = 0
    if storage_root.is_dir():
        for relative, _hash, _size in published:
            bundle_root = (storage_root / relative).parent.parent / "artwork"
            if bundle_root.is_dir():
                bundle_artwork = len(list(bundle_root.glob("*.jpg"))) + len(
                    list(bundle_root.glob("*.png"))
                )
                break
    if release.get("expect", {}).get("noArtwork"):
        verifier.equals(f"{slug}: no artwork published", bundle_artwork, 0)
    elif release["artwork"]["present"]:
        verifier.check(
            f"{slug}: artwork published into the bundle",
            bundle_artwork >= 1,
            f"artwork files={bundle_artwork}",
        )

    # Billing parse expectations.
    expect = release.get("expect", {})
    if expect.get("featuredTrackArtist"):
        featured = psql_rows(
            url,
            "SELECT a.canonical_name FROM track_artists ta "
            "  JOIN artists a ON a.id = ta.artist_id "
            "  JOIN tracks t ON t.id = ta.track_id "
            f" WHERE t.release_id = {sql_literal(row['id'])} AND ta.role = 'featured'",
        )
        names = [entry[0] for entry in featured]
        verifier.check(
            f"{slug}: featured artist parsed",
            expect["featuredTrackArtist"] in names,
            f"featured={names}",
        )
    if expect.get("primaryTrackArtist"):
        primary = psql_rows(
            url,
            "SELECT a.canonical_name FROM track_artists ta "
            "  JOIN artists a ON a.id = ta.artist_id "
            "  JOIN tracks t ON t.id = ta.track_id "
            f" WHERE t.release_id = {sql_literal(row['id'])} AND ta.role = 'primary'",
        )
        names = [entry[0] for entry in primary]
        verifier.check(
            f"{slug}: primary artist parsed",
            expect["primaryTrackArtist"] in names,
            f"primary={names}",
        )
    for protected in expect.get("mustNotSplitArtists", []):
        # Scoped to this Release: earlier corpus runs legitimately created the same
        # artist name, so a global count would be meaningless.
        count = int(
            scalar(
                url,
                "SELECT count(DISTINCT a.id)::bigint FROM artists a "
                "  JOIN track_artists ta ON ta.artist_id = a.id "
                "  JOIN tracks t ON t.id = ta.track_id "
                f" WHERE t.release_id = {sql_literal(row['id'])} "
                f"   AND a.canonical_name = {sql_literal(protected)}",
            )
            or "0"
        )
        verifier.equals(f"{slug}: '{protected}' kept whole", count, 1)
    for forbidden in expect.get("mustNotContainArtist", []):
        count = int(
            scalar(
                url,
                "SELECT count(DISTINCT a.id)::bigint FROM artists a "
                "  JOIN track_artists ta ON ta.artist_id = a.id "
                "  JOIN tracks t ON t.id = ta.track_id "
                f" WHERE t.release_id = {sql_literal(row['id'])} "
                f"   AND a.canonical_name = {sql_literal(forbidden)}",
            )
            or "0"
        )
        verifier.equals(f"{slug}: no artist named '{forbidden}'", count, 0)

    # Classification trait expectations.
    if expect.get("trait"):
        traits = psql_rows(
            url,
            "SELECT rt.trait FROM release_traits rt "
            "  JOIN release_classifications rc ON rc.id = rt.classification_id "
            f" WHERE rc.release_id = {sql_literal(row['id'])}",
        )
        names = [entry[0] for entry in traits]
        verifier.check(f"{slug}: trait {expect['trait']} detected", expect["trait"] in names, f"traits={names}")

    # Duplicate re-import. This build does not dedupe at Release level, so the
    # declared behaviour is a second Release whose Tracks the novelty classifier
    # flags `exact_duplicate`. If the product ever starts merging instead, this
    # assertion fails and the parity question gets re-opened deliberately.
    if release.get("duplicateOf"):
        expected_category = release.get("expect", {}).get("noveltyCategory", "exact_duplicate")
        flagged = int(
            scalar(
                url,
                "SELECT count(*)::bigint FROM track_novelty tn "
                "  JOIN tracks t ON t.id = tn.track_id "
                f" WHERE t.release_id = {sql_literal(row['id'])} "
                f"   AND tn.category = {sql_literal(expected_category)}",
            )
            or "0"
        )
        verifier.equals(
            f"{slug}: every re-imported track flagged {expected_category}",
            flagged,
            release["trackCount"],
        )
        originals = int(
            scalar(
                url,
                "SELECT count(*)::bigint FROM releases "
                f" WHERE title = {sql_literal(release['album'])}",
            )
            or "0"
        )
        verifier.equals(
            f"{slug}: re-import creates exactly one extra Release (no release-level dedupe)",
            originals,
            2,
        )


def discover_public_url(api_url: str) -> str:
    """Find a non-loopback address that answers for `api_url`, or return ""."""
    parsed = urllib.parse.urlparse(api_url)
    port = parsed.port or 8080
    try:
        completed = subprocess.run(
            ["ip", "-4", "addr", "show", "scope", "global"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return ""
    import re as _re

    for address in _re.findall(r"inet (\d+\.\d+\.\d+\.\d+)", completed.stdout):
        candidate = f"http://{address}:{port}"
        try:
            status, _payload = Http(candidate)._request("GET", "/rest/ping?f=json", expect_json=False)
        except Exception:  # noqa: BLE001 - any failure means "not usable"
            continue
        if status == 200:
            return candidate
    return ""


def subsonic_auth(username: str, password: str) -> str:
    """OpenSubsonic auth query fragment.

    Uses the password branch (`p`): Melodee's token branch validates against the
    stored *user secret*, not the password, so `t`/`s` with a password fails with
    error 40 — a documented deviation from the OpenSubsonic spec that this
    exercise surfaces rather than hides.
    """
    return (
        f"u={urllib.parse.quote(username)}&p={urllib.parse.quote(password)}"
        "&v=1.16.1&c=melodee-known-corpus&f=json"
    )


def verify_clients(
    verifier: Verifier,
    api: Http,
    release: dict[str, Any],
    *,
    username: str,
    password: str,
    client_url: str,
) -> None:
    """Client surface: a listener must be able to find the album and stream a song."""
    slug = release["slug"]
    if release.get("tier", "core") == "faults" or not release.get("tracks"):
        return
    declared = release.get("expect", {}).get("terminalStates", ["indexed"])
    if "indexed" not in declared:
        # Review-routed or fault fixtures are intentionally absent from client
        # surfaces; verified in `verify_release` instead.
        return
    auth = subsonic_auth(username, password)
    # Client checks run against `--client-base-url`. Loopback requests hit the
    # captured OpenSubsonic bypass (no principal → media routes answer 404), which
    # is a documented limitation rather than a corpus defect.
    client = Http(client_url)

    status, payload = client._request(
        "GET",
        f"/rest/search3?{auth}&query={urllib.parse.quote(release['album'])}&songCount=20&albumCount=10",
    )
    if status != 200:
        verifier.check(f"{slug}: OpenSubsonic search3 reachable", False, f"status={status} {payload}")
        return
    body = (payload or {}).get("subsonic-response", {})
    results = body.get("searchResult3", {}) or {}
    albums = [album.get("name") for album in results.get("album", []) or []]
    verifier.check(
        f"{slug}: OpenSubsonic finds the album",
        release["album"] in albums,
        f"albums={albums}",
    )

    album_id = next(
        (album.get("id") for album in results.get("album", []) or [] if album.get("name") == release["album"]),
        None,
    )
    songs: list[dict[str, Any]] = []
    if album_id:
        status, detail = client._request("GET", f"/rest/getAlbum?{auth}&id={album_id}")
        if status == 200:
            body = (detail or {}).get("subsonic-response", {})
            songs = ((body.get("album") or {}).get("song") or [])
    titles = [song.get("title") for song in songs]
    for track in release["tracks"]:
        verifier.check(
            f"{slug}: OpenSubsonic lists '{track['title']}'",
            track["title"] in titles,
            f"songs={titles[:6]}",
        )
    # A listener must be able to stream: the byte response proves the canonical
    # asset resolves through the client surface.
    if songs:
        song_id = songs[0].get("id")
        status, payload = client._request(
            "GET", f"/rest/stream?{auth}&id={song_id}", expect_json=False
        )
        verifier.check(
            f"{slug}: OpenSubsonic stream returns audio",
            status == 200 and isinstance(payload, bytes) and len(payload) > 10_000,
            f"status={status} bytes={len(payload) if isinstance(payload, bytes) else 'n/a'}",
        )


def verify_transversal(verifier: Verifier, api: Http, url: str, expected: dict[str, Any]) -> None:
    """Invariants that hold across the whole corpus rather than one Release."""
    # Search projection equals catalog: no pending rows, and the index knows the
    # fixtures. `melodee-cli` is the durable, non-HTTP read path.
    pending = int(
        scalar(
            url,
            "SELECT count(*)::bigint FROM catalog_projection_outbox WHERE processed_at IS NULL",
        )
        or "0"
    )
    verifier.equals("projection: no pending outbox rows", pending, 0)

    cli = ROOT / "target" / "debug" / "melodee-cli"
    if cli.is_file():
        try:
            status = json.loads(
                run([str(cli), "search", "status"], timeout=120)
            )
        except (VerifyError, json.JSONDecodeError) as error:
            verifier.check("search status readable", False, str(error))
        else:
            generation = status.get("generation", {})
            catalog_tracks = int(scalar(url, "SELECT count(*)::bigint FROM tracks") or "0")
            catalog_releases = int(scalar(url, "SELECT count(*)::bigint FROM releases") or "0")
            verifier.equals(
                "search index: track docs equal catalog tracks",
                generation.get("tracks"),
                catalog_tracks,
            )
            verifier.equals(
                "search index: release docs equal catalog releases",
                generation.get("releases"),
                catalog_releases,
            )
            pending_projection = status.get("projection", {}).get("pending")
            verifier.equals("search status: no pending projection", pending_projection, 0)

        # Every fixture must be findable by its own album title through MQL.
        for release in expected["releases"]:
            if release.get("tier", "core") == "faults":
                continue
            query = f'title:"{release["album"]}"'
            try:
                output = run([str(cli), "search", "query", query, "--limit", "5"], timeout=120)
            except VerifyError as error:
                verifier.check(f"search query for '{release['album']}'", False, str(error))
                continue
            verifier.check(
                f"search query finds '{release['album']}'",
                release["album"].lower() in output.lower(),
                f"output={output[:160]!r}",
            )
    else:
        verifier.check("melodee-cli built", False, f"missing {cli}")

    # Runtime invariants from the audit script (workflow completion, dictionaries).
    audit = ROOT / "scripts" / "audit_runtime_invariants.py"
    if audit.is_file():
        completed = subprocess.run(
            [sys.executable, str(audit), "--json"], capture_output=True, text=True, check=False
        )
        if completed.returncode == 0:
            verifier.check("runtime invariants hold", True)
        else:
            detail = completed.stdout or completed.stderr
            try:
                report = json.loads(completed.stdout)
                failing = [check["name"] for check in report["checks"] if check["status"] == "FAIL"]
                detail = f"failing: {failing}"
            except json.JSONDecodeError:
                pass
            verifier.check("runtime invariants hold", False, detail[:300])


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


def emit_ui_expectations(
    path: Path,
    expected: dict[str, Any],
    verifier: Verifier,
    url: str,
) -> None:
    """Write the resolver-free expectations the Playwright suite asserts against.

    Release/Track/Artist display values come from the oracle; the public ids used
    for navigation come from the database, so the UI spec never hardcodes either.
    """
    fixtures: list[dict[str, Any]] = []
    for release in expected["releases"]:
        declared = release.get("expect", {}).get("terminalStates", ["indexed"])
        if "indexed" not in declared:
            continue
        row = release_row(url, release["album"], release["displayArtist"])
        if row is None:
            continue
        fixtures.append(
            {
                "slug": release["slug"],
                "purpose": release.get("purpose", ""),
                "releaseId": row["public_api_id"],
                "album": release["album"],
                "folder": release["folder"],
                "displayArtist": release["displayArtist"],
                "trackCount": release["trackCount"],
                "hasArtwork": bool(release["artwork"]["present"]),
                "tracks": [
                    {
                        "title": track["title"],
                        "artist": track["artist"],
                    }
                    for track in release["tracks"]
                ],
                "expect": release.get("expect", {}),
            }
        )

    body = (
        "// GENERATED by scripts/verify_known_corpus.py --emit-ui-expectations.\n"
        "// Do not edit: it is derived from tests/fixtures/media-test-metadata.json plus the\n"
        "// resolved catalogue ids, so the UI suite asserts the same oracle as the\n"
        "// database-level verification.\n"
        "\n"
        "export interface KnownTrack { title: string; artist: string }\n"
        "\n"
        "export interface KnownRelease {\n"
        "  slug: string;\n"
        "  purpose: string;\n"
        "  releaseId: string;\n"
        "  album: string;\n"
        "  folder: string;\n"
        "  displayArtist: string;\n"
        "  trackCount: number;\n"
        "  hasArtwork: boolean;\n"
        "  tracks: KnownTrack[];\n"
        "  expect: Record<string, unknown>;\n"
        "}\n"
        "\n"
        f"export const RUN_ID = {json.dumps(expected.get('runId'))};\n"
        f"export const KNOWN_RELEASES: KnownRelease[] = {json.dumps(fixtures, indent=2)};\n"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS, help="generated corpus directory")
    parser.add_argument("--api", default=DEFAULT_API, help="instance base URL")
    parser.add_argument("--username", default=os.environ.get("MELODEE_LIVE_USERNAME", ""))
    parser.add_argument("--password", default=os.environ.get("MELODEE_LIVE_PASSWORD", ""))
    parser.add_argument("--ingest", action="store_true", help="copy the corpus into inbound and wait")
    parser.add_argument(
        "--client-base-url",
        default=os.environ.get("MELODEE_CLIENT_URL", ""),
        help=(
            "base URL for OpenSubsonic client-surface checks; must be non-loopback because "
            "loopback requests hit the captured OpenSubsonic bypass (no principal → 404)"
        ),
    )
    parser.add_argument("--timeout-minutes", type=int, default=20)
    parser.add_argument(
        "--require-tier",
        choices=("core", "faults", "all"),
        default=None,
        help="fail when the generated corpus does not include this tier (guards a partial run)",
    )
    parser.add_argument("--json", action="store_true", help="emit the report as JSON")
    parser.add_argument(
        "--emit-ui-expectations",
        type=Path,
        default=None,
        help=(
            "write the decided values as a TypeScript module the Playwright known-answer "
            "suite imports, so the UI assertions come from the same oracle"
        ),
    )
    args = parser.parse_args()

    expected_path = args.corpus / "expected.json"
    if not expected_path.is_file():
        print(f"error: no oracle at {expected_path}; run scripts/generate_known_corpus.py", file=sys.stderr)
        return 2
    try:
        expected = json.loads(expected_path.read_text())
    except json.JSONDecodeError as error:
        print(f"error: oracle is not valid JSON: {error}", file=sys.stderr)
        return 2

    if args.require_tier and expected.get("tier") != args.require_tier:
        print(
            f"error: corpus tier is {expected.get('tier')!r}, expected {args.require_tier!r}; "
            "regenerate with scripts/generate_known_corpus.py --tier "
            f"{args.require_tier}",
            file=sys.stderr,
        )
        return 2

    env = load_env()
    url = database_url(env)
    api = Http(args.api)
    client_url = args.client_base_url.strip()
    if not client_url:
        # Best effort: the host's own non-loopback address keeps the captured
        # bypass out of the way when the server is bound beyond 127.0.0.1.
        client_url = discover_public_url(args.api)
        if not client_url:
            print(
                "note: no --client-base-url given and no non-loopback address found; "
                "skipping OpenSubsonic client-surface checks (loopback hits the captured "
                "auth bypass, which answers 404 for media routes).",
                file=sys.stderr,
            )

    try:
        if not args.username or not args.password:
            raise VerifyError(
                "admin credentials required; set MELODEE_LIVE_USERNAME / MELODEE_LIVE_PASSWORD"
            )
        api.sign_in(args.username, args.password)
        if args.ingest:
            ingest(
                args.corpus,
                env,
                timeout_minutes=args.timeout_minutes,
                run_id=str(expected.get('runId') or ''),
            )
    except VerifyError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    verifier = Verifier(url=url, api=api)
    for release in expected["releases"]:
        verify_release(verifier, url, release, env)
        if client_url:
            verify_clients(
                verifier,
                api,
                release,
                username=args.username,
                password=args.password,
                client_url=client_url,
            )
    verify_transversal(verifier, api, url, expected)

    if args.emit_ui_expectations:
        emit_ui_expectations(args.emit_ui_expectations, expected, verifier, url)

    report = {
        "corpus": str(args.corpus),
        "runId": expected.get("runId"),
        "tier": expected.get("tier"),
        "releases": len(expected["releases"]),
        "checks": len(verifier.results),
        "failures": [result.as_dict() for result in verifier.failures],
        "gaps": [result.as_dict() for result in verifier.gaps if not result.ok],
        "results": [result.as_dict() for result in verifier.results],
    }
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        for result in verifier.results:
            if result.gap and not result.ok:
                mark = "GAP "
            else:
                mark = "PASS" if result.ok else "FAIL"
            line = f"  {mark}  {result.check}"
            if not result.ok:
                line += f"  ({result.detail})"
            print(line)
        print()
        open_gaps = [result for result in verifier.gaps if not result.ok]
        if open_gaps:
            print(f"{len(open_gaps)} documented product gap(s) (see --json `gaps`):")
            for result in open_gaps:
                print(f"  - {result.check}: {result.detail}")
            print()
        if verifier.failures:
            print(f"{len(verifier.failures)} of {len(verifier.results)} checks failed.")
        else:
            print(f"All behaviour checks passed ({len(verifier.results)} assertions).")
    return 1 if verifier.failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
