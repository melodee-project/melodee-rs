#!/usr/bin/env python3
"""Recover an invocation-owned benchmark clone from a protected manifest.

A candidate is dropped only after connecting to the same PostgreSQL server,
checking its current database, and reading the exact inherited random marker.
This does not terminate an orphaned workload process or remove media files.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
from urllib.parse import parse_qsl, urlsplit

import test_database

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = re.compile(r"perf_owner_[0-9a-f]{32}\Z")
TOKEN = re.compile(r"[0-9a-f]{64}\Z")
MAX_MANIFEST_BYTES = 16 * 1024


def sql(url: str, statement: str) -> str:
    result = subprocess.run(["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", url,
                             "--tuples-only", "--no-align", "--command", statement],
                            cwd=ROOT, capture_output=True, text=True, check=False, timeout=45,
                            env={k: v for k, v in os.environ.items() if not k.upper().startswith("PG")})
    if result.returncode:
        raise RuntimeError("database operation failed")
    return result.stdout.strip()


def endpoint(admin: str) -> dict:
    parsed = urlsplit(admin)
    forbidden = {"host", "hostaddr", "port", "dbname", "database", "service", "options"}
    if (parsed.scheme not in {"postgres", "postgresql"} or not parsed.hostname
            or any(k.lower() in forbidden for k, _ in parse_qsl(parsed.query, keep_blank_values=True))):
        raise ValueError("ambiguous PostgreSQL administrator endpoint")
    return {"host": parsed.hostname, "port": parsed.port or 5432}


def server_identity(admin: str) -> dict:
    identity = json.loads(sql(admin, """SELECT json_build_object('system_id',(SELECT system_identifier::text FROM pg_control_system()),
        'server_address',inet_server_addr()::text,'server_port',inet_server_port(),
        'in_recovery',pg_is_in_recovery())"""))
    if (not isinstance(identity, dict) or not isinstance(identity.get("system_id"), str)
            or not identity["system_id"].isdigit() or identity.get("in_recovery") is not False
            or not isinstance(identity.get("server_port"), int)):
        raise ValueError("server identity cannot be established")
    return identity


def validate_record(record: object) -> dict:
    if not isinstance(record, dict) or set(record) != {"version", "name", "source", "schema", "token", "endpoint", "server"}:
        raise ValueError("invalid ownership manifest")
    if (type(record["version"]) is not int or record["version"] != 1 or not isinstance(record["name"], str)
            or not test_database.OWNED.fullmatch(record["name"])
            or not isinstance(record["source"], str) or not test_database.OWNED.fullmatch(record["source"])
            or record["name"] == record["source"] or not isinstance(record["schema"], str)
            or not SCHEMA.fullmatch(record["schema"]) or not isinstance(record["token"], str)
            or not TOKEN.fullmatch(record["token"]) or not isinstance(record["endpoint"], dict)
            or not isinstance(record["server"], dict)):
        raise ValueError("invalid ownership fields")
    ep, server = record["endpoint"], record["server"]
    if (set(ep) != {"host", "port"} or not isinstance(ep["host"], str) or not ep["host"]
            or type(ep["port"]) is not int or not 1 <= ep["port"] <= 65535
            or set(server) != {"system_id", "server_address", "server_port", "in_recovery"}
            or not isinstance(server["system_id"], str) or not server["system_id"].isdigit()
            or len(server["system_id"]) > 32
            or not (server["server_address"] is None or isinstance(server["server_address"], str))
            or type(server["server_port"]) is not int or not 1 <= server["server_port"] <= 65535
            or server["in_recovery"] is not False):
        raise ValueError("invalid server identity")
    return record


def write_manifest(path: Path, record: dict) -> None:
    validate_record(record)
    parent = path.parent
    metadata = parent.stat(follow_symlinks=False)
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
        raise ValueError("manifest directory is not private and owned")
    if path.exists() or path.is_symlink():
        raise ValueError("manifest already exists")
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        stream = os.fdopen(fd, "w", encoding="utf-8")
        fd = -1
        with stream:
            json.dump(record, stream, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        if fd >= 0:
            try:
                os.close(fd)
            except OSError:
                pass
    directory = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def read_manifest(path: Path) -> dict:
    parent = path.parent.stat(follow_symlinks=False)
    if not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.geteuid() or stat.S_IMODE(parent.st_mode) != 0o700:
        raise ValueError("manifest directory is not private and owned")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or stat.S_IMODE(info.st_mode) != 0o600 or info.st_size > MAX_MANIFEST_BYTES):
            raise ValueError("manifest is not a private owned regular file")
        stream = os.fdopen(fd, "r", encoding="utf-8")
        fd = -1
        with stream:
            payload = stream.read(MAX_MANIFEST_BYTES + 1)
        if len(payload.encode("utf-8")) > MAX_MANIFEST_BYTES:
            raise ValueError("manifest exceeds size limit")
        record = json.loads(payload)
    finally:
        if fd >= 0:
            try:
                os.close(fd)
            except OSError:
                pass
    return validate_record(record)


def verify_candidate(record: dict, admin: str) -> int:
    record = validate_record(record)
    if endpoint(admin) != record["endpoint"] or server_identity(admin) != record["server"]:
        raise ValueError("PostgreSQL server identity changed")
    name, schema, token = record["name"], record["schema"], record["token"]
    url = test_database.database_url(admin, name)
    proof = json.loads(sql(url, f'''SELECT json_build_object('database',current_database(),
        'marker',(SELECT token FROM "{schema}".ownership WHERE singleton),
        'oid',(SELECT oid::bigint FROM pg_database WHERE datname=current_database()),
        'system_id',(SELECT system_identifier::text FROM pg_control_system()),
        'server_address',inet_server_addr()::text,'server_port',inet_server_port(),
        'in_recovery',pg_is_in_recovery())'''))
    oid = proof.pop("oid", None)
    if (type(oid) is not int or oid <= 0 or proof != {"database": name, "marker": token, **record["server"]}):
        raise ValueError("candidate ownership marker or server identity mismatch")
    return oid


def recover(record: dict, admin: str) -> str:
    record = validate_record(record)
    if endpoint(admin) != record["endpoint"] or server_identity(admin) != record["server"]:
        raise ValueError("PostgreSQL server identity changed")
    name = record["name"]
    exists = sql(admin, f"SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname='{name}')")
    if exists == "f":
        return "absent"
    if exists != "t":
        raise RuntimeError("cannot establish candidate database existence")
    oid = verify_candidate(record, admin)
    if server_identity(admin) != record["server"] or sql(admin, f"SELECT oid::bigint FROM pg_database WHERE datname='{name}'") != str(oid):
        raise ValueError("candidate database changed during verification")
    # The random marker establishes ownership only under the documented
    # assumption that other privileged actors do not race database replacement.
    sql(admin, f'DROP DATABASE "{name}" WITH (FORCE)')
    if sql(admin, f"SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname='{name}')") != "f":
        raise RuntimeError("candidate database remains after cleanup")
    return "dropped_verified"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path, help="private manifest left by an interrupted owned run")
    args = parser.parse_args()
    try:
        admin = os.environ.get("MELODEE_TEST_DATABASE_URL", "")
        result = recover(read_manifest(args.manifest), admin)
        print(json.dumps({"cleanup": result}))
        # Keep the manifest for human audit; the operator may remove its own
        # private directory once all orphaned processes/media are addressed.
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, KeyError) as error:
        print(f"Owned clone recovery failed closed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
