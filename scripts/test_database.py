#!/usr/bin/env python3
"""Run PostgreSQL tests with a disposable migrated template, or benchmark setup.

Only UUID-named databases created by this invocation are ever removed. Forward
migration tests ignore the template and continue to create empty databases.
"""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from urllib.parse import urlsplit, urlunsplit
import uuid

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ADMIN = "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres"
OWNED = re.compile(r"melodee_template_[0-9a-f]{32}\Z")


def database_url(admin: str, name: str) -> str:
    if not OWNED.fullmatch(name):
        raise ValueError("refusing an unowned template database name")
    parsed = urlsplit(admin)
    if parsed.scheme not in {"postgres", "postgresql"}:
        raise ValueError("test administrator connection must be a PostgreSQL URL")
    # A database query parameter overrides the path in some PostgreSQL clients.
    from urllib.parse import parse_qsl, urlencode
    query = urlencode([(k, v) for k, v in parse_qsl(parsed.query, keep_blank_values=True) if k not in {"dbname", "database"}])
    return urlunsplit(parsed._replace(path="/" + name, query=query))


def sql(admin: str, statement: str) -> str:
    result = subprocess.run(["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", admin,
                             "--tuples-only", "--no-align", "--command", statement],
                            cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode:
        # Don't include argv/connection strings in exceptions or logs.
        raise RuntimeError("test database operation failed (check PostgreSQL availability/permissions)")
    return result.stdout.strip()


def drop_owned(admin: str, name: str, created: set[str]) -> None:
    if name not in created or not OWNED.fullmatch(name):
        raise ValueError("refusing cleanup outside this invocation's created databases")
    sql(admin, f'DROP DATABASE IF EXISTS "{name}" WITH (FORCE)')
    created.remove(name)


def create(admin: str, created: set[str], template: str | None = None) -> str:
    name = "melodee_template_" + uuid.uuid4().hex
    suffix = ""
    if template is not None:
        if template not in created or not OWNED.fullmatch(template):
            raise ValueError("template was not created by this invocation")
        suffix = f' TEMPLATE "{template}"'
    sql(admin, f'CREATE DATABASE "{name}"{suffix}')
    created.add(name)
    target = database_url(admin, name)
    if sql(target, "SELECT current_database()") != name:
        raise RuntimeError("test database identity mismatch; refusing migrations")
    return name


def migrate(admin: str, name: str) -> None:
    env = os.environ.copy()
    env["MELODEE__DATABASE__URL"] = database_url(admin, name)
    target = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
    if not target.is_absolute():
        target = ROOT / target
    if os.environ.get("CARGO_BUILD_TARGET"):
        target /= os.environ["CARGO_BUILD_TARGET"]
    cli = target / "debug" / ("melodee-cli.exe" if os.name == "nt" else "melodee-cli")
    subprocess.run([str(cli), "migrate"], cwd=ROOT, env=env, check=True, stdout=subprocess.DEVNULL)


def execute(command: list[str], *, benchmark: bool = False) -> int:
    admin = os.environ.get("MELODEE_TEST_DATABASE_URL", DEFAULT_ADMIN)
    created: set[str] = set()
    subprocess.run(["cargo", "build", "--locked", "-p", "melodee-cli", "-p", "melodee-application", "--bin", "melodee-cli", "--bin", "melodee-script-worker"], cwd=ROOT, check=True)
    try:
        start = time.monotonic()
        template = create(admin, created)
        migrate(admin, template)
        migrate_seconds = time.monotonic() - start
        if benchmark:
            start = time.monotonic()
            clone = create(admin, created, template)
            clone_seconds = time.monotonic() - start
            print(json.dumps({"fresh_migration_seconds": migrate_seconds, "template_clone_seconds": clone_seconds}))
            drop_owned(admin, clone, created)
            return 0
        env = os.environ.copy()
        env["MELODEE_TEST_TEMPLATE_DATABASE"] = template
        return subprocess.run(command, cwd=ROOT, env=env, check=False).returncode
    finally:
        failures = []
        for name in list(created):
            try:
                drop_owned(admin, name, created)
            except (RuntimeError, ValueError, OSError) as error:
                failures.append(f"{name}: {error}")
        if failures:
            raise RuntimeError("owned test database cleanup failed: " + "; ".join(failures))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--benchmark", action="store_true")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command and not args.benchmark:
        parser.error("provide the test command after --")
    try:
        return execute(command, benchmark=args.benchmark)
    except (RuntimeError, ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"PostgreSQL test setup failed: {error}", file=sys.stderr)
        return 1

if __name__ == "__main__":
    raise SystemExit(main())
