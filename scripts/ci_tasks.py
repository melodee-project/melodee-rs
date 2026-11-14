#!/usr/bin/env python3
"""Small tested CI operations behind single-command workflow entry points."""
from __future__ import annotations
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
HOSTED_TOOL_DIRECTORIES = (Path('/usr/local/lib/android'), Path('/usr/share/dotnet'), Path('/opt/ghc'))


def run(command: list[str], *, cwd: Path = ROOT) -> None:
    subprocess.run(command, cwd=cwd, check=True)


def free_space() -> None:
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('MELODEE_CI_RUNNER_ENVIRONMENT') != 'github-hosted':
        raise ValueError('tool cleanup is restricted to GitHub-hosted workflow runners')
    # Validate every target before deleting any; no globs, symlink traversal, or caller paths.
    for target in HOSTED_TOOL_DIRECTORIES:
        if target.is_symlink() or target.resolve() != target:
            raise ValueError('hosted tool topology drift; refusing cleanup')
        if target.exists() and not target.is_dir():
            raise ValueError('hosted tool target is not a directory')
    print(shutil.disk_usage('/'))
    for target in HOSTED_TOOL_DIRECTORIES:
        if target.exists():
            shutil.rmtree(target)
    print(shutil.disk_usage('/'))


def execute(task: str) -> None:
    if task == 'free-space':
        free_space()
    elif task == 'toolchain':
        version = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
        run(['rustup', 'toolchain', 'install', version, '--profile', 'minimal', '--component', 'rustfmt', '--component', 'clippy'])
        run(['rustup', 'default', version])
    elif task == 'media-tools':
        run(['sudo', 'apt-get', 'update'])
        run(['sudo', 'apt-get', 'install', '--no-install-recommends', '-y', 'ffmpeg', 'libchromaprint-tools', 'postgresql-client'])
        for tool in ('ffprobe', 'ffmpeg', 'fpcalc'):
            result = subprocess.run([tool, '-version'], cwd=ROOT, check=True, capture_output=True, text=True)
            print(result.stdout.splitlines()[0])
    elif task == 'versions':
        for tool in ('rustc', 'cargo', sys.executable):
            run([tool, '--version'])
    elif task == 'repository-docs':
        for script, args in [('validate_repository.py', []), ('extract_requirements.py', ['--check'])]:
            run([sys.executable, str(ROOT / 'scripts' / script), *args])
    elif task in {'admin-static', 'docs-static'}:
        sys.path.insert(0, str(ROOT / 'scripts'))
        import node_toolchain
        project = ROOT / ('admin-ui' if task == 'admin-static' else 'docs')
        commands = ('check', 'lint', 'i18n:check') if task == 'admin-static' else ('check', 'check:links')
        pnpm = node_toolchain.command(project)
        for command in commands:
            run([*pnpm, command], cwd=project)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('task', choices=('free-space', 'toolchain', 'media-tools', 'versions', 'repository-docs', 'admin-static', 'docs-static'))
    args = parser.parse_args()
    try:
        execute(args.task)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'CI task failed: {error}', file=sys.stderr)
        return 1
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
