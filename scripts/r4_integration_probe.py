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
import os
import re
import smtplib
import ssl
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ElementTree
from pathlib import Path
from urllib.parse import urljoin, urlsplit


MAX_RESPONSE_BYTES = 4 * 1024 * 1024
MAX_OIDC_KEYS = 256
HEADER_NAME = re.compile(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]+$")


class ProbeError(RuntimeError):
    """A failed bounded probe with no sensitive endpoint material."""


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Keep an explicitly supplied probe endpoint as the only target."""

    def redirect_request(self, request, fp, code, msg, headers, newurl):
        del request, fp, code, msg, headers, newurl
        return None


NO_REDIRECT_OPENER = urllib.request.build_opener(NoRedirectHandler)


def validate_url(value: str, label: str) -> str:
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError as error:
        raise ProbeError(f"{label} must use a valid URL host and port") from error
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ProbeError(f"{label} must be an http(s) URL with a host")
    if parsed.username or parsed.password:
        raise ProbeError(f"{label} must not contain embedded credentials")
    del port
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
        with NO_REDIRECT_OPENER.open(request, timeout=10) as response:
            return response.status, dict(response.headers.items()), read_response(response)
    except urllib.error.HTTPError as error:
        try:
            body = error.read(MAX_RESPONSE_BYTES + 1)
            if len(body) > MAX_RESPONSE_BYTES:
                raise ProbeError("error response exceeded the 4 MiB bounded probe limit") from error
            return error.code, dict(error.headers.items()), body
        finally:
            error.close()
    except (urllib.error.URLError, TimeoutError) as error:
        raise ProbeError(f"HTTP probe failed for endpoint {endpoint_fingerprint(url)}") from error


def parse_expected_header(value: str) -> tuple[str, str]:
    """Parse a response-header assertion without ever echoing its value."""
    name, separator, expected = value.partition("=")
    name = name.strip()
    if not separator or not HEADER_NAME.fullmatch(name):
        raise ProbeError("--expect-header must use a valid Header-Name=value form")
    if any(ord(character) < 32 or ord(character) == 127 for character in expected):
        raise ProbeError("--expect-header contains a control character")
    return name.lower(), expected


def proxy_tls_probe(
    url: str,
    *,
    require_https: bool,
    expected_headers: list[str],
) -> dict[str, object]:
    """Perform a bounded GET through the intended proxy/TLS endpoint.

    Only the requested header names and a fingerprint of the endpoint are
    emitted. Values are compared in memory and are never printed or stored in
    the evidence artifact, preventing accidental capture of cookies or tokens.
    """
    endpoint = validate_url(url, "--proxy-url")
    parsed = urlsplit(endpoint)
    if require_https and parsed.scheme != "https":
        raise ProbeError("--proxy-url must use https when --require-https is set")
    assertions = [parse_expected_header(value) for value in expected_headers]
    status, headers, _body = http_get(endpoint)
    normalized = {name.lower(): value for name, value in headers.items()}
    for name, expected in assertions:
        if normalized.get(name) != expected:
            raise ProbeError(f"proxy/TLS response header assertion failed for {name}")
    return {
        "status": status,
        "scheme": parsed.scheme,
        "endpoint_fingerprint": endpoint_fingerprint(endpoint),
        "asserted_headers": [name for name, _expected in assertions],
    }


def validate_jwks_document(document: object) -> dict[str, object]:
    """Validate only bounded public-key metadata, never returning key material."""
    if not isinstance(document, dict) or not isinstance(document.get("keys"), list):
        raise ProbeError("OIDC JWKS response lacks a keys array")
    keys = document["keys"]
    if len(keys) < 1 or len(keys) > MAX_OIDC_KEYS:
        raise ProbeError(f"OIDC JWKS keys array must contain 1 to {MAX_OIDC_KEYS} keys")
    key_types: set[str] = set()
    for key in keys:
        if not isinstance(key, dict) or not isinstance(key.get("kty"), str) or not key["kty"]:
            raise ProbeError("OIDC JWKS contains a key without a key type")
        if "kid" in key and not isinstance(key["kid"], str):
            raise ProbeError("OIDC JWKS contains a non-string key identifier")
        key_types.add(key["kty"])
    return {"key_count": len(keys), "key_types": sorted(key_types)}


def contains_symlink_component(path: Path) -> bool:
    """Return true when any existing component of ``path`` is a symlink."""
    current = Path(path.anchor) if path.is_absolute() else Path(".")
    for component in path.parts:
        if component in {"", path.anchor}:
            continue
        current /= component
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError:
            break
        except OSError:
            # The caller must fail closed when a component cannot be inspected.
            return True
        if stat.S_ISLNK(mode):
            return True
    return False


def open_descriptor_nofollow(path: Path, leaf_flags: int) -> tuple[int, os.stat_result]:
    """Open a path through directory descriptors without following races."""
    if os.name != "posix":
        raise ProbeError("secure descriptor inspection is unsupported on this platform")
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    directory = getattr(os, "O_DIRECTORY", 0)
    if not nofollow or not directory:
        raise ProbeError("secure descriptor inspection requires no-follow directory support")
    absolute = path if path.is_absolute() else Path.cwd() / path
    components = absolute.parts
    if not components or any(component == ".." for component in components):
        raise ProbeError("backup file path must not contain traversal")
    flags = os.O_RDONLY | os.O_CLOEXEC | nofollow | directory
    current = os.open(absolute.anchor or ".", flags)
    try:
        for component in components[1:] if absolute.anchor else components:
            if component in {"", "."}:
                continue
            is_leaf = component == components[-1]
            next_fd = os.open(
                component,
                leaf_flags if is_leaf else flags,
                dir_fd=current,
            )
            os.close(current)
            current = next_fd
        metadata = os.fstat(current)
        return current, metadata
    except OSError as error:
        os.close(current)
        raise ProbeError("path could not be opened through a safe descriptor") from error


def open_regular_file_nofollow(path: Path) -> tuple[int, os.stat_result]:
    """Open a backup through directory descriptors without following races."""
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    cloexec = getattr(os, "O_CLOEXEC", 0)
    nonblock = getattr(os, "O_NONBLOCK", 0)
    descriptor, metadata = open_descriptor_nofollow(
        path,
        os.O_RDONLY | cloexec | nofollow | nonblock,
    )
    if not stat.S_ISREG(metadata.st_mode):
        os.close(descriptor)
        raise ProbeError("backup file must be a regular, non-symlink file")
    return descriptor, metadata


def open_directory_nofollow(path: Path) -> tuple[int, os.stat_result]:
    """Open a configured directory through held no-follow descriptors."""
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    cloexec = getattr(os, "O_CLOEXEC", 0)
    directory = getattr(os, "O_DIRECTORY", 0)
    descriptor, metadata = open_descriptor_nofollow(
        path,
        os.O_RDONLY | cloexec | directory | nofollow,
    )
    if not stat.S_ISDIR(metadata.st_mode):
        os.close(descriptor)
        raise ProbeError("configured path is not a regular directory")
    return descriptor, metadata


def filesystem_probe(path: Path, label: str) -> dict[str, object]:
    if contains_symlink_component(path):
        raise ProbeError(f"{label} must be an existing regular directory and not a symlink")
    descriptor, metadata = open_directory_nofollow(path)
    try:
        usage = os.fstatvfs(descriptor)
        block_size = usage.f_frsize or usage.f_bsize
        free_bytes = usage.f_bavail * block_size
        total_bytes = usage.f_blocks * block_size
    finally:
        os.close(descriptor)
    return {
        "label": label,
        "path_fingerprint": hashlib.sha256(str(path).encode()).hexdigest(),
        "device": metadata.st_dev,
        "free_bytes": free_bytes,
        "total_bytes": total_bytes,
        "headroom_ratio": round(free_bytes / total_bytes, 6) if total_bytes else 0.0,
    }


def backup_probe(path: Path) -> dict[str, object]:
    if contains_symlink_component(path):
        raise ProbeError("backup file must be a regular, non-symlink file")
    descriptor, metadata = open_regular_file_nofollow(path)
    try:
        if metadata.st_size <= 0:
            raise ProbeError("backup file is empty")
        digest = hashlib.sha256()
        with os.fdopen(os.dup(descriptor), "rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        os.lseek(descriptor, 0, os.SEEK_SET)
        result = subprocess.run(
            ["pg_restore", "--list", f"/proc/self/fd/{descriptor}"],
            check=False,
            capture_output=True,
            text=True,
            pass_fds=(descriptor,),
        )
        if result.returncode != 0 or not result.stdout.strip():
            raise ProbeError("backup file failed pg_restore --list validation")
        return {
            "bytes": metadata.st_size,
            "sha256": digest.hexdigest(),
            "device": metadata.st_dev,
            "toc_entries": sum(
                1
                for line in result.stdout.splitlines()
                if line.strip() and not line.startswith(";")
            ),
        }
    finally:
        os.close(descriptor)


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
            with NO_REDIRECT_OPENER.open(request, timeout=10) as response:
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
        issuer_url = validate_url(document["issuer"], "OIDC issuer")
        jwks_url = validate_url(document["jwks_uri"], "OIDC jwks_uri")
        jwks_status, _jwks_headers, jwks_body = http_get(jwks_url)
        try:
            jwks_document = json.loads(jwks_body)
        except json.JSONDecodeError as error:
            raise ProbeError("OIDC JWKS response was not JSON") from error
        if jwks_status != 200:
            raise ProbeError(f"OIDC JWKS endpoint returned {jwks_status}, expected 200")
        jwks_summary = validate_jwks_document(jwks_document)
        checks["oidc_discovery"] = {
            "status": status,
            "issuer_fingerprint": endpoint_fingerprint(issuer_url),
            "jwks_fingerprint": endpoint_fingerprint(jwks_url),
            "jwks_status": jwks_status,
        }
        checks["oidc_jwks"] = {
            "status": jwks_status,
            **jwks_summary,
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

    proxy_url = getattr(args, "proxy_url", None)
    if proxy_url:
        checks["proxy_tls"] = proxy_tls_probe(
            proxy_url,
            require_https=getattr(args, "require_https", False),
            expected_headers=getattr(args, "expect_header", []),
        )

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
        backup_device = checks["backup"]["device"]  # type: ignore[index]
        canonical_device = checks["canonical_root"]["device"]  # type: ignore[index]
        checks["storage_topology"] = {
            "canonical_device": canonical_device,
            "backup_device": backup_device,
            "same_filesystem": canonical_device == backup_device,
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
    parser.add_argument(
        "--proxy-url",
        help="bounded GET endpoint exposed through the intended proxy/TLS path",
    )
    parser.add_argument(
        "--require-https",
        action="store_true",
        help="require --proxy-url to use HTTPS with normal certificate validation",
    )
    parser.add_argument(
        "--expect-header",
        action="append",
        default=[],
        metavar="NAME=VALUE",
        help="assert a response header without recording its value; repeatable",
    )
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
