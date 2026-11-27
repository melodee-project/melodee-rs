#!/usr/bin/env python3
"""Run the release-build production identity benchmark on the owned fixture."""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys

from run_perf_mutation_benchmarks import validate_fixture

ROOT = Path(__file__).resolve().parents[1]


def benchmark_command(binary: Path) -> list[str]:
    return [str(binary), "measure_scoped_identity_discovery_and_production_drain", "--exact",
            "--ignored", "--nocapture", "--test-threads=1"]


def resolve_binary(requested: Path | None) -> Path:
    test_dir = (ROOT / "target/release/deps").resolve()
    if requested is None:
        candidates = [p for p in test_dir.glob("perf_identity_reconciliation-*")
                      if p.is_file() and os.access(p, os.X_OK)]
        if len(candidates) != 1:
            raise ValueError("provide --test-binary when there is not exactly one release test executable")
        binary = candidates[0].resolve()
    else:
        binary = requested.resolve()
    if (binary.parent != test_dir or not binary.name.startswith("perf_identity_reconciliation-")
            or not binary.is_file() or not os.access(binary, os.X_OK)):
        raise ValueError("test binary must be a prebuilt identity test in target/release/deps")
    return binary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, help="prebuilt release test executable")
    parser.add_argument("--augment-identities", action="store_true",
                        help="populate observations and identity evidence before timing")
    args = parser.parse_args()
    try:
        env = os.environ.copy()
        validate_fixture(env)
        binary = resolve_binary(args.test_binary)
        if args.augment_identities:
            result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_identity_fixture.py")],
                                    cwd=ROOT, env=env, check=False)
            if result.returncode:
                return result.returncode
        return subprocess.run(
            benchmark_command(binary),
            cwd=ROOT, env=env, check=False,
        ).returncode
    except (OSError, ValueError) as error:
        print(f"Identity benchmark failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
