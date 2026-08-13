#!/usr/bin/env python3
"""Phase 8 real-client smoke runner.

Boots the real ``melodee-server`` against a disposable PostgreSQL database
seeded with the frozen fixture catalog (the same seeder the differential
replay uses), serves a pinned Feishin web build, and drives the shipping
client through the documented smoke workflows with Playwright/Chromium.

Feishin v1.15.1 supports both server types this phase implements —
Jellyfin and Subsonic/OpenSubsonic — so one real client exercises both
surfaces.

The Chromium instance runs with ``--disable-web-security``: the fixture
server and the client dev server are different localhost origins, and the
frozen baseline's strict CORS allowlist (empty by default) blocks that
combination identically. This relaxes browser-origin enforcement only; the
client application code and the server are untouched.

Usage:
    python3 scripts/run_phase8_client_smoke.py --feishin-dist /path/to/out/web

Evidence (JSON request log, screenshots) is written to
``compatibility/captures/phase8-client-smoke/`` (gitignored captures tree).
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import random
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

CAPTURES_DIR = ROOT / "compatibility" / "captures" / "phase8-client-smoke"


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


oracle = load_module("legacy_oracle", ROOT / "scripts" / "legacy_oracle.py")
differential = load_module(
    "run_phase8_differential", ROOT / "scripts" / "run_phase8_differential.py"
)


def wait_ready(base_url: str, process: subprocess.Popen) -> None:
    deadline = time.monotonic() + 120.0
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--feishin-dist",
        type=Path,
        required=True,
        help="Path to the Feishin web build (out/web) of the pinned client release.",
    )
    parser.add_argument("--db-name", default="melodee_phase8_client_smoke")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()

    if not (args.feishin_dist / "index.html").exists():
        raise SystemExit(f"no index.html under {args.feishin_dist}")

    work_dir = Path(tempfile.mkdtemp(prefix="phase8-client-smoke-"))
    storage_root = work_dir / "storage"
    inbound_root = work_dir / "inbound"
    storage_root.mkdir()
    inbound_root.mkdir()

    catalog = json.loads(differential.CATALOG_PATH.read_text(encoding="utf-8"))
    media = differential.generate_media(catalog, storage_root)
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
    seed_sql = differential.build_seed_sql(catalog, media, password_hash)
    seed_path = work_dir / "seed.sql"
    seed_path.write_text(seed_sql, encoding="utf-8")

    differential.psql(f'DROP DATABASE IF EXISTS "{args.db_name}" WITH (FORCE);')
    differential.psql(f'CREATE DATABASE "{args.db_name}";')

    port = random.randint(20000, 45000)
    base_url = f"http://127.0.0.1:{port}"
    env = {
        **os.environ,
        "MELODEE__DATABASE__URL": f"postgres://melodee:melodee-dev@127.0.0.1:5433/{args.db_name}",
        "MELODEE__SERVER__BIND": f"127.0.0.1:{port}",
        "MELODEE__STORAGE__ROOT": str(storage_root),
        "MELODEE__INGEST__ENABLED": "false",
        "MELODEE__INGEST__SOURCES_JSON": json.dumps(
            [
                {
                    "id": differential.FIXTURE_LIBRARY_ID,
                    "root": str(inbound_root),
                    "name": catalog["library"]["name"],
                }
            ]
        ),
        "MELODEE__NATIVE__AUTH__JWT_KEY": oracle.FIXTURE_JWT_KEY,
        "MELODEE__NATIVE__TRUSTED_PROXIES": "127.0.0.1",
        "MELODEE__PODCAST__ENABLED": "true",
        "MELODEE__JELLYFIN__TOKEN_PEPPER": "phase8-client-smoke-pepper-NOT-A-SECRET",
        "RUST_LOG": "warn",
    }

    server_log = work_dir / "server.log"
    server = subprocess.Popen(
        [str(ROOT / "target" / "debug" / "melodee-server")],
        cwd=ROOT,
        env=env,
        stdout=server_log.open("w"),
        stderr=subprocess.STDOUT,
    )
    wait_ready(base_url, server)

    # Migrations ran at boot; seed the fixture catalog.
    subprocess.run(
        [
            "psql", "-h", "127.0.0.1", "-p", "5433", "-U", "melodee",
            "-v", "ON_ERROR_STOP=1", "-d", args.db_name, "-f", str(seed_path),
        ],
        env={**os.environ, "PGPASSWORD": "melodee-dev"},
        check=True,
        capture_output=True,
        text=True,
    )

    # Serve the pinned Feishin web build.
    client_port = random.randint(46000, 55000)
    client_server = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(client_port), "--bind", "127.0.0.1"],
        cwd=args.feishin_dist,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    client_url = f"http://127.0.0.1:{client_port}"

    # The captured OpenSubsonic auth bypass trusts loopback clients, so a
    # same-host browser never gets a principal; route the OpenSubsonic surface
    # through the XFF-injecting relay to emulate a remote client (the same
    # mechanism Phase 0 used to defeat the bypass).
    relay_port = random.randint(20000, 45000)
    while relay_port == port:
        relay_port = random.randint(20000, 45000)
    relay = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "compatibility" / "clients" / "phase8" / "opensubsonic_remote_relay.py"),
            str(relay_port),
            str(port),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    relay_url = f"http://127.0.0.1:{relay_port}"

    CAPTURES_DIR.mkdir(parents=True, exist_ok=True)
    probe = ROOT / "compatibility" / "clients" / "phase8" / "feishin_smoke.cjs"
    probe_env = {
        **os.environ,
        "FEISHIN_URL": client_url,
        "MELODEE_URL": base_url,
        "MELODEE_OS_URL": relay_url,
        "MELODEE_USER": oracle.FIXTURE_USER,
        "MELODEE_PASSWORD": oracle.FIXTURE_PASSWORD,
        "MELODEE_OS_PASSWORD": oracle.FIXTURE_OPENSUBSONIC_SECRET,
        "MELODEE_SERVER_NAME": "melodee-rs-phase8",
        "SMOKE_EVIDENCE_DIR": str(CAPTURES_DIR),
        "PLAYWRIGHT_NODE_PATH": os.environ.get("PLAYWRIGHT_NODE_PATH", ""),
    }
    exit_code = 1
    try:
        result = subprocess.run(
            ["node", str(probe)],
            env=probe_env,
            timeout=900,
        )
        exit_code = result.returncode
    finally:
        for process in (client_server, relay, server):
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
        if not args.keep:
            differential.psql(f'DROP DATABASE IF EXISTS "{args.db_name}" WITH (FORCE);')
            shutil.rmtree(work_dir, ignore_errors=True)
        else:
            print(f"kept work dir: {work_dir}")

    raise SystemExit(exit_code)


if __name__ == "__main__":
    main()
