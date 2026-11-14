#!/usr/bin/env python3
"""Build and serve the Admin UI for Playwright using its pinned package manager."""
from __future__ import annotations
import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4173)
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error('port must be between 1 and 65535')
    sys.path.insert(0, str(ROOT / 'scripts'))
    import node_toolchain
    project = ROOT / 'admin-ui'
    pnpm = node_toolchain.command(project)
    subprocess.run([*pnpm, 'build'], cwd=project, check=True)
    return subprocess.run([*pnpm, 'exec', 'vite', 'preview', '--port', str(args.port), '--strictPort', '--host', '127.0.0.1'], cwd=project, check=False).returncode

if __name__ == '__main__':
    raise SystemExit(main())
