#!/usr/bin/env python3
"""R4.4 dual-run differential (ADR-0148/0149): replay the frozen Phase 0
compatibility corpus against the Rust server whose catalog arrived through
the Phase 11 migration tooling, from the same snapshot as the legacy oracle.

Pipeline per run:

1. Dump the live legacy fixture database (``melodee_phase0``, seeded by
   ``legacy_oracle.py`` from the frozen baseline) and restore it as an
   isolated read-only-mirrored snapshot — the dual-run source.
2. Create a fresh Rust target, apply the schema migrations, then run the
   accepted migration sequence: ``baseline`` -> ``plan`` -> ``import`` with
   the legacy secret key (per-user secrets re-sealed) and the generated
   canonical media tree -> ``verify``.
3. Boot the real ``melodee-server`` against the migrated database and replay
   every native-v1, OpenSubsonic, and Jellyfin scenario file, reseeding
   through the migration between files exactly like the oracle reseeded
   between captures.
4. Diff each produced capture against the frozen goldens with the accepted
   normalization machinery; differences outside the known-difference
   registry fail the run.

Usage:

    python3 scripts/run_r4_dual_run.py [--keep] [--only native|opensubsonic|jellyfin]
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

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import compare_captures  # noqa: E402
import capture_baseline  # noqa: E402
import legacy_oracle as oracle  # noqa: E402


def load_harness():
    spec = importlib.util.spec_from_file_location(
        "run_phase8_differential", ROOT / "scripts" / "run_phase8_differential.py"
    )
    if spec is None or spec.loader is None:
        raise SystemExit("unable to load run_phase8_differential.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


harness = load_harness()

SOURCE_DB = "r4_dual_source"
TARGET_DB = "r4_dual_target"
CLI = ROOT / "target" / "debug" / "melodee-cli"

SURFACE_GOLDENS = {
    "native": ROOT / "compatibility" / "melodee-api-v1" / "golden",
    "opensubsonic": ROOT / "compatibility" / "opensubsonic" / "golden",
    "jellyfin": ROOT / "compatibility" / "jellyfin" / "golden",
}

# All scenario files in the oracle's capture order (state-mutating flows stay
# isolated by a migrated reseed between files, mirroring the oracle's reseed).
SCENARIO_FILES = oracle.SCENARIO_FILES


def scenario_surface(name: str) -> str:
    if name.startswith("jellyfin-"):
        return "jellyfin"
    if name.startswith("opensubsonic-"):
        return "opensubsonic"
    return "native"


def psql(sql: str, *, database: str = "postgres") -> None:
    subprocess.run(
        ["psql", harness.DB_ADMIN_URL.rsplit("/", 1)[0] + "/" + database, "-c", sql],
        check=True,
        capture_output=True,
        env={**os.environ, "PGPASSWORD": "melodee-dev"},
    )


def cli_legacy(target_url: str, args: list[str]) -> str:
    result = subprocess.run(
        [str(CLI), "legacy", *args],
        check=True,
        capture_output=True,
        text=True,
        env={**os.environ, "MELODEE__DATABASE__URL": target_url},
        cwd=ROOT,
    )
    return result.stdout


def snapshot_legacy_source(keep_dump: Path) -> None:
    """Dump the live legacy fixture DB and restore it as the isolated source."""
    dump = keep_dump / "legacy-phase0.dump"
    subprocess.run(
        [
            "pg_dump", "-h", "127.0.0.1", "-p", "5433", "-U", "melodee",
            "-d", "melodee_phase0", "--format=custom", "-f", str(dump),
        ],
        check=True,
        capture_output=True,
        env={**os.environ, "PGPASSWORD": "melodee-dev"},
    )
    psql(f'DROP DATABASE IF EXISTS "{SOURCE_DB}" WITH (FORCE);')
    psql(f'CREATE DATABASE "{SOURCE_DB}";')
    subprocess.run(
        ["pg_restore", "-h", "127.0.0.1", "-p", "5433", "-U", "melodee",
         "-d", SOURCE_DB, str(dump)],
        check=True,
        capture_output=True,
        env={**os.environ, "PGPASSWORD": "melodee-dev"},
    )


def migrate_fresh(source_url: str, target_url: str, library_path: str, canonical_root: Path) -> None:
    """Create a fresh migrated target: schema, then the accepted import
    sequence. One migration run per replay seed mirrors the oracle reseed."""
    psql(f'DROP DATABASE IF EXISTS "{TARGET_DB}" WITH (FORCE);')
    psql(f'CREATE DATABASE "{TARGET_DB}";')
    subprocess.run(
        [str(CLI), "migrate"],
        check=True,
        capture_output=True,
        env={**os.environ, "MELODEE__DATABASE__URL": target_url},
        cwd=ROOT,
    )
    # The target deployment's security key matches the fixture deployment, so
    # legacy-protected per-user secrets re-seal rather than fall out as issues.
    psql(
        "INSERT INTO server_settings (key, value, is_secret, updated_at) "
        f"VALUES ('security.secretKey', '\"{oracle.FIXTURE_SECURITY_KEY}\"'::jsonb, true, now())",
        database=TARGET_DB,
    )
    cli_legacy(target_url, ["import", "--source-db", source_url,
                            "--library-path-map", f"{library_path}=",
                            "--canonical-root", str(canonical_root),
                            "--legacy-secret-key", oracle.FIXTURE_SECURITY_KEY])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--only", choices=sorted(SURFACE_GOLDENS))
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()

    if not args.skip_build:
        subprocess.run(["cargo", "build", "-p", "melodee-cli", "-p", "melodee-server"], cwd=ROOT, check=True)

    work_dir = args.work_dir or Path(tempfile.mkdtemp(prefix="r4-dual-run-"))
    work_dir.mkdir(parents=True, exist_ok=True)
    storage_root = work_dir / "storage"
    inbound_root = work_dir / "inbound"
    storage_root.mkdir(exist_ok=True)
    inbound_root.mkdir(exist_ok=True)
    captures_dir = work_dir / "captures"
    captures_dir.mkdir(exist_ok=True)

    port = args.port or random.randint(20000, 45000)
    base_url = f"http://127.0.0.1:{port}"

    # The deterministic fixture media tree, byte-identical to what the legacy
    # oracle generated for the baseline capture.
    catalog = json.loads(harness.CATALOG_PATH.read_text(encoding="utf-8"))
    harness.generate_media(catalog, storage_root)

    print("snapshotting legacy fixture database ...")
    snapshot_legacy_source(work_dir)
    source_url = harness.DB_ADMIN_URL.rsplit("/", 1)[0] + "/" + SOURCE_DB
    target_url = harness.DB_ADMIN_URL.rsplit("/", 1)[0] + "/" + TARGET_DB
    library_path = subprocess.run(
        ["psql", source_url, "-At", "-c", 'SELECT "Path" FROM "Libraries" ORDER BY "Id" LIMIT 1'],
        check=True, capture_output=True, text=True,
        env={**os.environ, "PGPASSWORD": "melodee-dev"},
    ).stdout.strip()
    print(f"legacy library path: {library_path!r}")

    def server_env(podcasts_enabled: bool) -> dict[str, str]:
        return {
            **os.environ,
            "MELODEE__DATABASE__URL": target_url,
            "MELODEE__SERVER__BIND": f"127.0.0.1:{port}",
            "MELODEE__STORAGE__ROOT": str(storage_root),
            "MELODEE__INGEST__ENABLED": "false",
            "MELODEE__INGEST__SOURCES_JSON": json.dumps(
                [{"id": harness.FIXTURE_LIBRARY_ID, "root": str(inbound_root),
                  "name": catalog["library"]["name"]}]
            ),
            "MELODEE__NATIVE__AUTH__JWT_KEY": oracle.FIXTURE_JWT_KEY,
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
        harness.wait_ready(base_url, process)
        return process

    def stop_server(process: subprocess.Popen) -> None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

    unexpected: list[tuple[str, str, object, object]] = []
    known: list[tuple[str, str, str]] = []
    replayed_groups: set[str] = set()

    def compare_group(golden_dir: Path, surface: str) -> None:
        names = set(compare_captures.metadata_by_name(golden_dir))
        output_dir = captures_dir / "all"
        differences = compare_captures.compare(golden_dir, output_dir, names)
        for difference in differences:
            reason = harness.is_known_difference(
                difference.scenario, difference.field, difference.expected, difference.actual
            )
            if reason is None:
                unexpected.append(
                    (difference.scenario, difference.field, difference.expected, difference.actual)
                )
            else:
                known.append((difference.scenario, difference.field, reason))
        replayed_groups.add(f"{surface}/{golden_dir.name}")
        print(f"  compare {golden_dir.name}: {len(differences)} difference(s)")

    files = [f for f in SCENARIO_FILES if args.only is None or scenario_surface(f) == args.only]
    servers: list[subprocess.Popen] = []
    migrated_once = False
    try:
        # The server boots against an already-migrated target; verify the
        # first migration once so the replay baseline is proven clean.
        migrate_fresh(source_url, target_url, library_path, storage_root)
        out = cli_legacy(target_url, ["inventory", "--source-db", source_url, "--json"])
        inventory = json.loads(out)
        # `import` records its own plan internally; verify the import run.
        import_run = subprocess.run(
            ["psql", target_url, "-At", "-c",
             "SELECT id FROM legacy_migration.runs WHERE state = 'importing' "
             "ORDER BY created_at DESC LIMIT 1"],
            check=True, capture_output=True, text=True,
            env={**os.environ, "PGPASSWORD": "melodee-dev"},
        ).stdout.strip()
        verify_out = cli_legacy(
            target_url, ["verify", "--run", import_run, "--source-db", source_url]
        )
        if "delta-ready" not in verify_out:
            raise SystemExit(f"migrated baseline did not verify clean:\n{verify_out}")
        print(f"migrated baseline verified clean (run {import_run}); inventory media: "
              f"{inventory['media']['file_count']} files")
        migrated_once = True

        servers.append(start_server(True, "server-enabled.log"))
        for scenario_file in files:
            if scenario_file == "opensubsonic-conditional-off.json":
                continue
            scenario_path = ROOT / "compatibility" / "scenarios" / scenario_file
            if not scenario_path.is_file():
                print(f"skip {scenario_file}: missing")
                continue
            # Migrate a fresh target per file: the dual-run equivalent of the
            # oracle's between-file reseed.
            migrate_fresh(source_url, target_url, library_path, storage_root)
            migrated_once = True
            scenarios = json.loads(scenario_path.read_text(encoding="utf-8"))
            output_dir = captures_dir / "all"
            output_dir.mkdir(parents=True, exist_ok=True)
            for key, value in harness.capture_env().items():
                if key.startswith(("PHASE0_", "LEGACY_")):
                    os.environ[key] = value
            print(f"replay {scenario_file}")
            for scenario in scenarios:
                variables: dict[str, str] = {"baseUrl": base_url.rstrip("/")}
                step_secrets: dict[str, str] = {}
                for step_name, step in capture_baseline.iter_steps(scenario):
                    try:
                        result = capture_baseline.capture_request(
                            base_url, step_name, step, output_dir, variables, step_secrets, None,
                        )
                    except SystemExit as error:
                        print(f"  {step_name}: capture assertion failed ({error})")
                        continue
                    except KeyError as missing:
                        result = {"name": step_name, "skipped": True, "reason": str(missing)}
                    status = (
                        "skipped" if result.get("skipped") else result.get("response", {}).get("status")
                    )
                    print(f"  {result['name']}: {status}")
        if any(f == "opensubsonic-conditional-off.json" for f in files):
            stop_server(servers.pop())
            servers.append(start_server(False, "server-disabled.log"))
            migrate_fresh(source_url, target_url, library_path, storage_root)
            scenarios = json.loads(
                (ROOT / "compatibility" / "scenarios" / "opensubsonic-conditional-off.json").read_text()
            )
            output_dir = captures_dir / "all"
            for scenario in scenarios:
                variables = {"baseUrl": base_url.rstrip("/")}
                step_secrets: dict[str, str] = {}
                for step_name, step in capture_baseline.iter_steps(scenario):
                    try:
                        capture_baseline.capture_request(
                            base_url, step_name, step, output_dir, variables, step_secrets, None,
                        )
                    except (SystemExit, KeyError) as error:
                        print(f"  {step_name}: {error}")

        # Compare every golden group that this run's surfaces cover.
        for surface, golden_root in SURFACE_GOLDENS.items():
            if args.only is not None and surface != args.only:
                continue
            for golden_dir in sorted(golden_root.iterdir()):
                if golden_dir.is_dir() and golden_dir.name != "provenance":
                    compare_group(golden_dir, surface)
    finally:
        for process in servers:
            stop_server(process)
        if not args.keep:
            for name in (SOURCE_DB, TARGET_DB):
                psql(f'DROP DATABASE IF EXISTS "{name}" WITH (FORCE);')
            shutil.rmtree(work_dir, ignore_errors=True)

    print(f"\nknown intentional differences: {len(known)}")
    for scenario, field, reason in known:
        print(f"  [known] {scenario}: {field} — {reason}")
    report_dir = ROOT / "design" / "evidence" / "r4"
    report_dir.mkdir(parents=True, exist_ok=True)
    report_path = report_dir / "dual-run-report.json"
    report_path.write_text(
        json.dumps(
            {
                "source": "migrated melodee_phase0 snapshot via melodee-cli legacy import",
                "migration_run_observed": migrated_once,
                "groups_replayed": sorted(replayed_groups),
                "known": [{"scenario": s, "field": f, "reason": r} for s, f, r in known],
                "unexpected": [
                    {"scenario": s, "field": f, "expected": e, "actual": a}
                    for s, f, e, a in unexpected
                ],
            },
            indent=2,
            default=str,
        )
        + "\n"
    )
    print(f"report: {report_path}")
    if unexpected:
        print(f"\nUNEXPECTED DIFFERENCES: {len(unexpected)}")
        for scenario, field, expected, actual in unexpected:
            print(f"  {scenario}: {field}\n    expected: {expected!r}\n    actual:   {actual!r}")
        raise SystemExit(1)
    print("\nR4 dual run: zero unexpected differences")


if __name__ == "__main__":
    main()
