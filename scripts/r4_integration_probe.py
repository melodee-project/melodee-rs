#!/usr/bin/env python3
"""Bounded R4 production-integration probe.

This probe performs only read-only checks. It never submits credentials,
creates accounts, sends mail, mutates a database, or writes to a media root.
Optional checks are enabled explicitly so a local run cannot accidentally
contact an operator endpoint merely because an environment variable exists.

Examples:

    python3 scripts/r4_integration_probe.py --base-url http://127.0.0.1:8080
    python3 scripts/r4_integration_probe.py --base-url https://server.example \
      --oidc-discovery-url https://accounts.example/.well-known/openid-configuration \
      --podcast-feed https://feeds.example/feed.xml \
      --canonical-root /srv/melodee/canonical --backup-file /srv/backups/latest.dump
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import smtplib
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ElementTree
from pathlib import Path
from urllib.parse import urljoin, urlsplit


MAX_RESPONSE_BYTES = 4 * 1024 * 1024


class ProbeError(RuntimeError):
    """A failed bounded probe with no sensitive endpoint material."""


def validate_url(value: str, label: str) -> str:
    parsed = urlsplit(value)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ProbeError(f"{label} must be an http(s) URL with a host")
    if parsed.username or parsed.password:
        raise ProbeError(f"{label} must not contain embedded credentials")
    return value.rstrip("/")


def endpoint_fingerprint(value: str) -> str:
    parsed = urlsplit(value)
    material = f"{parsed.scheme}://{parsed.hostname}:{parsed.port or ''}{parsed.path}"
    return hashlib.sha256(material.encode()).hexdigest()[:16]


def read_response(response: urllib.response.addinfourl) -> bytes:
    body = response.read(MAX_RESPONSE_BYTES + 1)
    if len(body) > MAX_RESPONSE_BYTES:
        raise ProbeError("response exceeded the 4 MiB bounded probe limit")
    return body


def http_get(url: str, headers: dict[str, str] | None = None) -> tuple[int, dict[str, str], bytes]:
    request = urllib.request.Request(url, headers=headers or {}, method="GET")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, dict(response.headers.items()), read_response(response)
    except urllib.error.HTTPError as error:
        body = error.read(MAX_RESPONSE_BYTES + 1)
        if len(body) > MAX_RESPONSE_BYTES:
            raise ProbeError("error response exceeded the 4 MiB bounded probe limit") from error
        return error.code, dict(error.headers.items()), body
    except (urllib.error.URLError, TimeoutError) as error:
        raise ProbeError(f"HTTP probe failed for endpoint {endpoint_fingerprint(url)}") from error


def filesystem_probe(path: Path, label: str) -> dict[str, object]:
    if path.is_symlink() or not path.exists() or not path.is_dir():
        raise ProbeError(f"{label} must be an existing regular directory and not a symlink")
    usage = shutil.disk_usage(path)
    return {
        "label": label,
        "path_fingerprint": hashlib.sha256(str(path).encode()).hexdigest(),
        "device": path.stat().st_dev,
        "free_bytes": usage.free,
        "total_bytes": usage.total,
        "headroom_ratio": round(usage.free / usage.total, 6) if usage.total else 0.0,
    }


def backup_probe(path: Path) -> dict[str, object]:
    if path.is_symlink() or not path.is_file():
        raise ProbeError("backup file must be a regular, non-symlink file")
    if path.stat().st_size <= 0:
        raise ProbeError("backup file is empty")
    result = subprocess.run(
        ["pg_restore", "--list", str(path)],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise ProbeError("backup file failed pg_restore --list validation")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return {
        "bytes": path.stat().st_size,
        "sha256": digest.hexdigest(),
        "toc_entries": sum(
            1 for line in result.stdout.splitlines() if line.strip() and not line.startswith(";")
        ),
    }


def run(args: argparse.Namespace) -> dict[str, object]:
    base = validate_url(args.base_url, "--base-url")
    results: dict[str, object] = {
        "artifact": "melodee.r4-integration-probe",
        "schema_version": 1,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "base_endpoint_fingerprint": endpoint_fingerprint(base),
        "checks": {},
    }
    checks: dict[str, object] = results["checks"]  # type: ignore[assignment]

    for path, expected in (("/healthz", 200), ("/readyz", 200)):
        status, headers, _body = http_get(urljoin(base + "/", path.lstrip("/")))
        if status != expected:
            raise ProbeError(f"{path} returned {status}, expected {expected}")
        checks[path] = {"status": status, "content_type": headers.get("Content-Type", "")}

    if args.range_url:
        range_url = validate_url(args.range_url, "--range-url")
        request = urllib.request.Request(
            range_url,
            headers={"Range": f"bytes=0-{args.range_bytes - 1}"},
            method="GET",
        )
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                body = read_response(response)
                status = response.status
                content_range = response.headers.get("Content-Range", "")
        except (urllib.error.URLError, TimeoutError) as error:
            raise ProbeError(f"range probe failed for endpoint {endpoint_fingerprint(range_url)}") from error
        if status != 206 or not content_range.startswith("bytes 0-"):
            raise ProbeError("range probe did not return a bounded 206 Content-Range")
        checks["range"] = {"status": status, "bytes_read": len(body), "content_range": content_range}

    if args.oidc_discovery_url:
        oidc_url = validate_url(args.oidc_discovery_url, "--oidc-discovery-url")
        status, _headers, body = http_get(oidc_url)
        try:
            document = json.loads(body)
        except json.JSONDecodeError as error:
            raise ProbeError("OIDC discovery response was not JSON") from error
        if status != 200 or not isinstance(document.get("issuer"), str) or not isinstance(document.get("jwks_uri"), str):
            raise ProbeError("OIDC discovery response lacks issuer/jwks_uri")
        checks["oidc_discovery"] = {
            "status": status,
            "issuer_fingerprint": endpoint_fingerprint(document["issuer"]),
            "jwks_fingerprint": endpoint_fingerprint(document["jwks_uri"]),
        }

    if args.podcast_feed:
        feed_url = validate_url(args.podcast_feed, "--podcast-feed")
        status, _headers, body = http_get(feed_url)
        if status != 200:
            raise ProbeError(f"podcast feed returned {status}, expected 200")
        try:
            ElementTree.fromstring(body)
        except ElementTree.ParseError as error:
            raise ProbeError("podcast feed was not well-formed XML") from error
        checks["podcast_feed"] = {"status": status, "bytes": len(body)}

    if args.lastfm_api_url:
        lastfm_url = validate_url(args.lastfm_api_url, "--lastfm-api-url")
        status, _headers, _body = http_get(lastfm_url)
        # A request without an API key/session is expected to be rejected by
        # Last.fm. The probe records reachability without accepting or
        # printing credentials.
        if status not in {200, 400, 401, 403, 404}:
            raise ProbeError(f"Last.fm endpoint returned unexpected status {status}")
        checks["lastfm_endpoint"] = {
            "status": status,
            "endpoint_fingerprint": endpoint_fingerprint(lastfm_url),
        }

    if args.smtp_host:
        if args.smtp_port < 1 or args.smtp_port > 65535:
            raise ProbeError("--smtp-port must be between 1 and 65535")
        context = ssl.create_default_context()
        try:
            with smtplib.SMTP(args.smtp_host, args.smtp_port, timeout=10) as client:
                client.ehlo()
                if args.smtp_starttls:
                    client.starttls(context=context)
                    client.ehlo()
        except (OSError, smtplib.SMTPException) as error:
            raise ProbeError("SMTP connectivity/TLS probe failed") from error
        checks["smtp"] = {"host_fingerprint": hashlib.sha256(args.smtp_host.encode()).hexdigest()[:16], "port": args.smtp_port, "starttls": args.smtp_starttls}

    if args.canonical_root:
        checks["canonical_root"] = filesystem_probe(args.canonical_root, "canonical-root")
    if args.backup_file:
        checks["backup"] = backup_probe(args.backup_file)
    if args.canonical_root and args.backup_file:
        backup_device = args.backup_file.stat().st_dev
        checks["storage_topology"] = {
            "canonical_device": args.canonical_root.stat().st_dev,
            "backup_device": backup_device,
            "same_filesystem": args.canonical_root.stat().st_dev == backup_device,
        }
    results["result"] = "pass"
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--range-url")
    parser.add_argument("--range-bytes", type=int, default=1024)
    parser.add_argument("--oidc-discovery-url")
    parser.add_argument("--podcast-feed")
    parser.add_argument("--lastfm-api-url")
    parser.add_argument("--smtp-host")
    parser.add_argument("--smtp-port", type=int, default=587)
    parser.add_argument("--smtp-starttls", action="store_true")
    parser.add_argument("--canonical-root", type=Path)
    parser.add_argument("--backup-file", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.range_bytes < 1 or args.range_bytes > MAX_RESPONSE_BYTES:
        parser.error("--range-bytes must be between 1 and 4194304")
    try:
        evidence = run(args)
    except ProbeError as error:
        print(f"probe failed: {error}", file=sys.stderr)
        return 1
    rendered = json.dumps(evidence, indent=2) if args.json else "R4 integration probe: pass\n"
    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
