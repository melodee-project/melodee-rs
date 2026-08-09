#!/usr/bin/env python3
"""Run the Phase 0 benchmark matrix (FIRST-10-ISSUES.md Issue 9) against a
benchmark dataset tier.

Measures representative legacy endpoints on all three compatibility surfaces
with multiple iterations, records environment metadata, and writes
machine-readable results to ``compatibility/benchmarks/results/`` plus a row
in ``compatibility/benchmarks/SUMMARY.md``.

Usage: python3 scripts/run_benchmarks.py --tier A [--iterations 5]

The benchmark server is the legacy oracle pointed at the tier database via
PHASE0_DB_NAME. Cold (first) and warm timings are recorded separately.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import statistics
import subprocess
import time
import urllib.request
import urllib.error
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RESULTS_DIR = ROOT / "compatibility" / "benchmarks" / "results"
SUMMARY_MD = ROOT / "compatibility" / "benchmarks" / "SUMMARY.md"


def load_oracle():
    spec = importlib.util.spec_from_file_location("legacy_oracle", ROOT / "scripts" / "legacy_oracle.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def env_metadata(tier: str, oracle, config) -> dict:
    def cmd_output(cmd: list[str]) -> str:
        try:
            return subprocess.run(cmd, capture_output=True, text=True, check=False).stdout.strip().splitlines()[0]
        except (OSError, IndexError):
            return "unknown"

    git_sha = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True
    ).stdout.strip()
    return {
        "dateUtc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "melodeeRsCommit": git_sha,
        "legacyBaselineCommit": oracle.FROZEN_COMMIT,
        "tier": tier,
        "database": config.db_name,
        "os": f"{platform.system()} {platform.release()}",
        "architecture": platform.machine(),
        "cpu": cmd_output(["sh", "-c", "grep -m1 'model name' /proc/cpuinfo | cut -d: -f2-"]).strip(),
        "logicalCpus": os.cpu_count(),
        "memory": cmd_output(["sh", "-c", "grep MemTotal /proc/meminfo"]),
        "postgres": cmd_output(["psql", "--version"]),
        "dotnet": cmd_output(["dotnet", "--version"]),
        "ffmpeg": cmd_output(["ffmpeg", "-version"]),
        "server": "Melodee 2.2.0 (Kestrel, Production, QuartzDisabled=true, UseForwardedHeaders=true)",
        "cacheState": "cold-on-first-iteration-then-warm (server restarted before matrix)",
        "datasetSeed": "deterministic md5('bench-<kind>-<i>') rows",
        "iterations": None,  # filled by caller
        "command": f"python3 scripts/run_benchmarks.py --tier {tier}",
    }


def timed_get(url: str, headers: dict[str, str] | None = None, timeout: int = 300) -> tuple[int, float, int]:
    request = urllib.request.Request(url, headers=headers or {})
    start = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        body = error.read()
        status = error.code
    elapsed_ms = (time.monotonic() - start) * 1000
    return status, elapsed_ms, len(body)


def build_matrix(base: str, token_native: str, os_auth: str, jf_token: str) -> list[dict]:
    nh = {"Authorization": f"Bearer {token_native}"}
    xh = {"X-Forwarded-For": "203.0.113.7", "X-Forwarded-Host": "capture-fixture.example.test"}
    jh = {"X-MediaBrowser-Token": jf_token}
    return [
        # Native Melodee API v1 (catalog-size sensitive)
        {"id": "native.artists.list", "surface": "melodee-api-v1", "method": "GET",
         "url": f"{base}/api/v1/artists?page=1&pageSize=50&orderBy=Name&orderDirection=asc", "headers": nh},
        {"id": "native.albums.list", "surface": "melodee-api-v1", "method": "GET",
         "url": f"{base}/api/v1/albums?page=1&pageSize=50&orderBy=Name&orderDirection=asc", "headers": nh},
        {"id": "native.songs.list", "surface": "melodee-api-v1", "method": "GET",
         "url": f"{base}/api/v1/songs?page=1&pageSize=50&orderBy=Title&orderDirection=asc", "headers": nh},
        {"id": "native.albums.recent", "surface": "melodee-api-v1", "method": "GET",
         "url": f"{base}/api/v1/albums/recent?limit=10", "headers": nh},
        {"id": "native.search.exact-artist", "surface": "melodee-api-v1", "method": "POST",
         "url": f"{base}/api/v1/search", "headers": nh,
         "json": {"query": "Artist 00001234", "pageSize": 20}},
        {"id": "native.search.partial-artist", "surface": "melodee-api-v1", "method": "POST",
         "url": f"{base}/api/v1/search", "headers": nh,
         "json": {"query": "Artist 000", "pageSize": 20}},
        {"id": "native.search.track-title", "surface": "melodee-api-v1", "method": "POST",
         "url": f"{base}/api/v1/search", "headers": nh,
         "json": {"query": "Track 000001234", "pageSize": 20}},
        {"id": "native.search.common-word", "surface": "melodee-api-v1", "method": "POST",
         "url": f"{base}/api/v1/search", "headers": nh,
         "json": {"query": "Artist", "pageSize": 20}},
        {"id": "native.search.unicode", "surface": "melodee-api-v1", "method": "POST",
         "url": f"{base}/api/v1/search", "headers": nh,
         "json": {"query": "アーティスト", "pageSize": 20}},
        {"id": "native.search.nonexistent", "surface": "melodee-api-v1", "method": "POST",
         "url": f"{base}/api/v1/search", "headers": nh,
         "json": {"query": "zzzznothing", "pageSize": 20}},
        # OpenSubsonic (enforced auth)
        {"id": "os.getIndexes", "surface": "opensubsonic", "method": "GET",
         "url": f"{base}/rest/getIndexes.view?f=json&v=1.16.1&c=bench&{os_auth}", "headers": xh},
        {"id": "os.getArtists", "surface": "opensubsonic", "method": "GET",
         "url": f"{base}/rest/getArtists.view?f=json&v=1.16.1&c=bench&{os_auth}", "headers": xh},
        {"id": "os.getAlbumList2.newest", "surface": "opensubsonic", "method": "GET",
         "url": f"{base}/rest/getAlbumList2.view?f=json&v=1.16.1&c=bench&type=newest&size=50&{os_auth}", "headers": xh},
        {"id": "os.search3", "surface": "opensubsonic", "method": "GET",
         "url": f"{base}/rest/search3.view?f=json&v=1.16.1&c=bench&query=Artist%20000&artistCount=10&albumCount=10&songCount=10&{os_auth}", "headers": xh},
        {"id": "os.getRandomSongs", "surface": "opensubsonic", "method": "GET",
         "url": f"{base}/rest/getRandomSongs.view?f=json&v=1.16.1&c=bench&size=10&{os_auth}", "headers": xh},
        {"id": "os.getStarred2", "surface": "opensubsonic", "method": "GET",
         "url": f"{base}/rest/getStarred2.view?f=json&v=1.16.1&c=bench&{os_auth}", "headers": xh},
        # Jellyfin
        {"id": "jf.userViews", "surface": "jellyfin", "method": "GET",
         "url": f"{base}/UserViews", "headers": jh},
        {"id": "jf.artists.list", "surface": "jellyfin", "method": "GET",
         "url": f"{base}/Artists?startIndex=0&limit=50", "headers": jh},
        {"id": "jf.items.audio", "surface": "jellyfin", "method": "GET",
         "url": f"{base}/Items?includeItemTypes=Audio&startIndex=0&limit=50", "headers": jh},
        {"id": "jf.items.albums", "surface": "jellyfin", "method": "GET",
         "url": f"{base}/Items?includeItemTypes=MusicAlbum&startIndex=0&limit=50", "headers": jh},
        {"id": "jf.playlists", "surface": "jellyfin", "method": "GET",
         "url": f"{base}/Playlists", "headers": jh},
    ]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tier", choices=["A", "B", "C"], required=True)
    parser.add_argument("--iterations", type=int, default=5)
    args = parser.parse_args()

    oracle = load_oracle()

    class Args:
        legacy_root = None

    os.environ["PHASE0_DB_NAME"] = f"melodee_bench_{args.tier.lower()}"
    config = oracle.OracleConfig(Args())

    oracle.cmd_stop(config)
    oracle.cmd_start(config)

    base = config.base_url
    # Native JWT
    import urllib.request as urlreq
    login = urlreq.Request(
        f"{base}/api/v1/auth/authenticate",
        data=json.dumps({"userName": "phase0-fixture", "password": "Mel0deeR0cks!"}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urlreq.urlopen(login, timeout=60) as response:
        native_token = json.loads(response.read())["token"]
    # OpenSubsonic token (md5(secret+salt)) — secret differs per bench DB
    # (generated at user creation); read it via the legacy decrypt path is not
    # possible, so authenticate OpenSubsonic with the plaintext password form.
    os_auth = "u=phase0-fixture&p=Mel0deeR0cks!"
    # Jellyfin token
    jf_login = urlreq.Request(
        f"{base}/Users/AuthenticateByName",
        data=json.dumps({"Username": "phase0-fixture", "Pw": "Mel0deeR0cks!"}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urlreq.urlopen(jf_login, timeout=60) as response:
        jf_token = json.loads(response.read())["AccessToken"]

    matrix = build_matrix(base, native_token, os_auth, jf_token)
    results = []
    for endpoint in matrix:
        body = None
        headers = dict(endpoint["headers"])
        if endpoint.get("json"):
            body = json.dumps(endpoint["json"]).encode()
            headers["Content-Type"] = "application/json"
        timings = []
        status = 0
        size = 0
        for iteration in range(args.iterations):
            request = urllib.request.Request(endpoint["url"], data=body, headers=headers,
                                             method=endpoint["method"])
            start = time.monotonic()
            try:
                with urlreq.urlopen(request, timeout=600) as response:
                    payload = response.read()
                    status = response.status
            except urllib.error.HTTPError as error:
                payload = error.read()
                status = error.code
            elapsed_ms = (time.monotonic() - start) * 1000
            size = len(payload)
            timings.append(round(elapsed_ms, 1))
        warm = timings[1:] if len(timings) > 1 else timings
        results.append({
            "id": endpoint["id"],
            "surface": endpoint["surface"],
            "status": status,
            "responseBytes": size,
            "coldMs": timings[0],
            "warmMedianMs": round(statistics.median(warm), 1),
            "warmMinMs": min(warm),
            "warmMaxMs": max(warm),
            "iterations": timings,
        })
        print(f"{endpoint['id']}: cold={timings[0]}ms warm-median={round(statistics.median(warm), 1)}ms status={status}")

    metadata = env_metadata(args.tier, oracle, config)
    metadata["iterations"] = args.iterations
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    result = {
        "schemaVersion": 1,
        "environment": metadata,
        "results": results,
    }
    out = RESULTS_DIR / f"tier-{args.tier.lower()}-{stamp}.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=2) + "\n")

    # Summary markdown row
    SUMMARY_MD.parent.mkdir(parents=True, exist_ok=True)
    if not SUMMARY_MD.exists():
        SUMMARY_MD.write_text(
            "# Phase 0 Benchmark Summary\n\n"
            "All timings are server-response wall time (ms) for a single request, "
            "median of warm iterations after the first (cold) call. Environment "
            "details are in the linked result files. Legacy Melodee 2.2.0 at "
            "baseline 4028b58d.\n\n"
            "| Date (UTC) | Tier | Endpoint | Cold (ms) | Warm median (ms) | Status | Result file |\n"
            "|---|---|---|---|---|---|---|\n"
        )
    with SUMMARY_MD.open("a") as handle:
        for entry in results:
            handle.write(
                f"| {metadata['dateUtc']} | {args.tier} | {entry['id']} | {entry['coldMs']} | "
                f"{entry['warmMedianMs']} | {entry['status']} | results/{out.name} |\n"
            )
    print(f"results -> {out.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
