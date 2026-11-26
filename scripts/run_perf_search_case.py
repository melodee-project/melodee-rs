#!/usr/bin/env python3
"""Run an owned-database search rebuild test with a safe bounded duration.

Use inside perf_representative.py. A timeout is recorded as censored evidence;
the test process is terminated and reaped before its private temp tree is
removed and the outer fixture drops its own database.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

OWNED = re.compile(r"melodee_template_[0-9a-f]{32}\Z")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", required=True, type=Path)
    parser.add_argument("--timeout-seconds", type=int, default=600)
    args = parser.parse_args()
    if args.timeout_seconds < 1:
        parser.error("timeout must be positive")
    marker = os.environ.get("MELODEE_PERF_OWNED_DATABASE", "")
    if not OWNED.fullmatch(marker) or not os.environ.get("MELODEE__DATABASE__URL"):
        print("Refusing to run without the owned fixture database marker and URL.", file=sys.stderr)
        return 1
    binary = args.test_binary
    if binary.is_symlink() or not binary.is_file() or not binary.name.startswith("perf_rebuild-"):
        print("Refusing an invalid search benchmark test binary.", file=sys.stderr)
        return 1
    binary = binary.resolve()
    with tempfile.TemporaryDirectory(prefix="melodee-perf-search-") as private_dir:
        env = os.environ.copy()
        for key in ("TMPDIR", "TMP", "TEMP"):
            env[key] = private_dir
        try:
            result = subprocess.run(
                [str(binary), "--ignored", "--nocapture", "--test-threads=1"],
                env=env, check=False, timeout=args.timeout_seconds,
            )
        except subprocess.TimeoutExpired:
            print(f"CENSORED: rebuild exceeded {args.timeout_seconds} seconds.", flush=True)
            return 124
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
