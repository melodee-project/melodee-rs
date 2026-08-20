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
HEX16 = re.compile(r"^[0-9a-f]{16}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
ISO_UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
CONTENT_RANGE = re.compile(r"^bytes 0-(\d+)/(\d+|\*)$")
SECRET_VALUE = re.compile(
    r"(?i)(?:\bbearer\s+[A-Za-z0-9._~-]{12,}\b|"
    r"(?:password|passwd|secret|token|cookie|authorization)\s*[=:]\s*[^\s,;]+|"
    r"postgres(?:ql)?://[^\s/]+:[^\s@]+@|-----BEGIN [^-]*PRIVATE KEY-----)"
)


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


def validate_oidc_discovery_document(document: object) -> tuple[str, str]:
    """Validate the bounded discovery shape before accessing object fields."""
    if not isinstance(document, dict):
        raise ProbeError("OIDC discovery response must be a JSON object")
    issuer = document.get("issuer")
    jwks_uri = document.get("jwks_uri")
    if not isinstance(issuer, str) or not isinstance(jwks_uri, str):
        raise ProbeError("OIDC discovery response lacks issuer/jwks_uri")
    return validate_url(issuer, "OIDC issuer"), validate_url(jwks_uri, "OIDC jwks_uri")


def validate_podcast_xml(body: bytes) -> None:
    """Reject DTD/entity declarations before parsing untrusted feed bytes."""
    if re.search(rb"<!\s*(?:DOCTYPE|ENTITY)\b", body, flags=re.IGNORECASE):
        raise ProbeError("podcast feed must not contain DTD or entity declarations")
    try:
        ElementTree.fromstring(body)
    except ElementTree.ParseError as error:
        raise ProbeError("podcast feed was not well-formed XML") from error


def _require_status(value: object, expected: set[int], field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value not in expected:
        raise ProbeError(f"integration evidence {field} has an invalid status")
    return value


def _require_fingerprint(value: object, pattern: re.Pattern[str], field: str) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise ProbeError(f"integration evidence {field} has an invalid fingerprint")
    return value


def _require_nonnegative_int(value: object, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ProbeError(f"integration evidence {field} must be a non-negative integer")
    return value


def validate_probe_artifact(value: object) -> dict[str, object]:
    """Validate the stable, redacted JSON emitted by the integration probe."""
    if not isinstance(value, dict):
        raise ProbeError("integration evidence must be a JSON object")
    expected_keys = {
        "artifact",
        "schema_version",
        "started_at",
        "base_endpoint_fingerprint",
        "checks",
        "result",
    }
    if set(value) != expected_keys:
        raise ProbeError("integration evidence has an unsupported top-level shape")
    if value.get("artifact") != "melodee.r4-integration-probe" or value.get("schema_version") != 1:
        raise ProbeError("integration evidence schema is unsupported")
    if not isinstance(value.get("started_at"), str) or not ISO_UTC.fullmatch(value["started_at"]):
        raise ProbeError("integration evidence started_at is not UTC")
    _require_fingerprint(value.get("base_endpoint_fingerprint"), HEX16, "base_endpoint_fingerprint")
    if value.get("result") != "pass":
        raise ProbeError("integration evidence result is not pass")
    checks = value.get("checks")
    if not isinstance(checks, dict):
        raise ProbeError("integration evidence checks must be an object")
    if set(checks) - {
        "/healthz",
        "/readyz",
        "range",
        "oidc_discovery",
        "oidc_jwks",
        "podcast_feed",
        "lastfm_endpoint",
        "proxy_tls",
        "smtp",
        "canonical_root",
        "backup",
        "storage_topology",
    }:
        raise ProbeError("integration evidence contains an unknown check")
    for path in ("/healthz", "/readyz"):
        check = checks.get(path)
        if not isinstance(check, dict):
            raise ProbeError(f"integration evidence is missing {path}")
        _require_status(check.get("status"), {200}, f"{path}.status")
        if not isinstance(check.get("content_type"), str):
            raise ProbeError(f"integration evidence {path}.content_type is invalid")

    range_check = checks.get("range")
    if range_check is not None:
        if not isinstance(range_check, dict):
            raise ProbeError("integration evidence range check is invalid")
        _require_status(range_check.get("status"), {206}, "range.status")
        bytes_read = range_check.get("bytes_read")
        if isinstance(bytes_read, bool) or not isinstance(bytes_read, int) or not 1 <= bytes_read <= MAX_RESPONSE_BYTES:
            raise ProbeError("integration evidence range.bytes_read is invalid")
        content_range = range_check.get("content_range")
        if not isinstance(content_range, str):
            raise ProbeError("integration evidence range.content_range is invalid")
        match = CONTENT_RANGE.fullmatch(content_range)
        if match is None or int(match.group(1)) + 1 != bytes_read:
            raise ProbeError("integration evidence range does not match the bounded body")
        if match.group(2) != "*" and int(match.group(2)) < bytes_read:
            raise ProbeError("integration evidence range total is smaller than the body")

    discovery = checks.get("oidc_discovery")
    if discovery is not None:
        if not isinstance(discovery, dict):
            raise ProbeError("integration evidence OIDC discovery check is invalid")
        _require_status(discovery.get("status"), {200}, "oidc_discovery.status")
        _require_fingerprint(discovery.get("issuer_fingerprint"), HEX16, "oidc_discovery.issuer_fingerprint")
        _require_fingerprint(discovery.get("jwks_fingerprint"), HEX16, "oidc_discovery.jwks_fingerprint")
        _require_status(discovery.get("jwks_status"), {200}, "oidc_discovery.jwks_status")

    jwks = checks.get("oidc_jwks")
    if jwks is not None:
        if not isinstance(jwks, dict):
            raise ProbeError("integration evidence OIDC JWKS check is invalid")
        _require_status(jwks.get("status"), {200}, "oidc_jwks.status")
        key_count = jwks.get("key_count")
        if isinstance(key_count, bool) or not isinstance(key_count, int) or not 1 <= key_count <= MAX_OIDC_KEYS:
            raise ProbeError("integration evidence OIDC key_count is invalid")
        key_types = jwks.get("key_types")
        if (
            not isinstance(key_types, list)
            or not key_types
            or key_types != sorted(key_types)
            or any(not isinstance(key_type, str) or not key_type for key_type in key_types)
        ):
            raise ProbeError("integration evidence OIDC key_types is invalid")

    podcast = checks.get("podcast_feed")
    if podcast is not None:
        if not isinstance(podcast, dict):
            raise ProbeError("integration evidence podcast check is invalid")
        _require_status(podcast.get("status"), {200}, "podcast_feed.status")
        bytes_read = podcast.get("bytes")
        if isinstance(bytes_read, bool) or not isinstance(bytes_read, int) or not 1 <= bytes_read <= MAX_RESPONSE_BYTES:
            raise ProbeError("integration evidence podcast_feed.bytes is invalid")

    lastfm = checks.get("lastfm_endpoint")
    if lastfm is not None:
        if not isinstance(lastfm, dict):
            raise ProbeError("integration evidence Last.fm check is invalid")
        _require_status(lastfm.get("status"), {200, 400, 401, 403, 404}, "lastfm_endpoint.status")
        _require_fingerprint(lastfm.get("endpoint_fingerprint"), HEX16, "lastfm_endpoint.endpoint_fingerprint")

    proxy = checks.get("proxy_tls")
    if proxy is not None:
        if not isinstance(proxy, dict):
            raise ProbeError("integration evidence proxy/TLS check is invalid")
        status = proxy.get("status")
        if isinstance(status, bool) or not isinstance(status, int) or not 100 <= status <= 599:
            raise ProbeError("integration evidence proxy_tls.status is invalid")
        if proxy.get("scheme") not in {"http", "https"}:
            raise ProbeError("integration evidence proxy_tls.scheme is invalid")
        _require_fingerprint(proxy.get("endpoint_fingerprint"), HEX16, "proxy_tls.endpoint_fingerprint")
        headers = proxy.get("asserted_headers")
        if not isinstance(headers, list) or any(
            not isinstance(header, str) or HEADER_NAME.fullmatch(header) is None for header in headers
        ):
            raise ProbeError("integration evidence proxy_tls.asserted_headers is invalid")

    smtp = checks.get("smtp")
    if smtp is not None:
        if not isinstance(smtp, dict):
            raise ProbeError("integration evidence SMTP check is invalid")
        _require_fingerprint(smtp.get("host_fingerprint"), HEX16, "smtp.host_fingerprint")
        port = smtp.get("port")
        if isinstance(port, bool) or not isinstance(port, int) or not 1 <= port <= 65535:
            raise ProbeError("integration evidence SMTP port is invalid")
        if not isinstance(smtp.get("starttls"), bool):
            raise ProbeError("integration evidence SMTP starttls is invalid")

    canonical = checks.get("canonical_root")
    if canonical is not None:
        if not isinstance(canonical, dict) or canonical.get("label") != "canonical-root":
            raise ProbeError("integration evidence canonical-root check is invalid")
        _require_fingerprint(canonical.get("path_fingerprint"), SHA256, "canonical_root.path_fingerprint")
        _require_nonnegative_int(canonical.get("device"), "canonical_root.device")
        free_bytes = _require_nonnegative_int(canonical.get("free_bytes"), "canonical_root.free_bytes")
        total_bytes = _require_nonnegative_int(canonical.get("total_bytes"), "canonical_root.total_bytes")
        headroom = canonical.get("headroom_ratio")
        if not isinstance(headroom, (int, float)) or isinstance(headroom, bool) or not 0 <= headroom <= 1:
            raise ProbeError("integration evidence canonical_root.headroom_ratio is invalid")
        if total_bytes and free_bytes > total_bytes:
            raise ProbeError("integration evidence canonical_root free bytes exceed total bytes")

    backup = checks.get("backup")
    if backup is not None:
        if not isinstance(backup, dict):
            raise ProbeError("integration evidence backup check is invalid")
        bytes_read = backup.get("bytes")
        if isinstance(bytes_read, bool) or not isinstance(bytes_read, int) or bytes_read <= 0:
            raise ProbeError("integration evidence backup.bytes is invalid")
        _require_fingerprint(backup.get("sha256"), SHA256, "backup.sha256")
        _require_nonnegative_int(backup.get("device"), "backup.device")
        if _require_nonnegative_int(backup.get("toc_entries"), "backup.toc_entries") < 1:
            raise ProbeError("integration evidence backup.toc_entries is empty")

    topology = checks.get("storage_topology")
    if topology is not None:
        if not isinstance(topology, dict):
            raise ProbeError("integration evidence storage topology is invalid")
        _require_nonnegative_int(topology.get("canonical_device"), "storage_topology.canonical_device")
        _require_nonnegative_int(topology.get("backup_device"), "storage_topology.backup_device")
        if not isinstance(topology.get("same_filesystem"), bool):
            raise ProbeError("integration evidence storage_topology.same_filesystem is invalid")

    rendered = json.dumps(value, sort_keys=True, ensure_ascii=False)
    if SECRET_VALUE.search(rendered):
        raise ProbeError("integration evidence contains an apparently unredacted secret")
    return value


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
        match = CONTENT_RANGE.fullmatch(content_range)
        if (
            status != 206
            or match is None
            or int(match.group(1)) + 1 != len(body)
            or len(body) > args.range_bytes
        ):
            raise ProbeError("range probe did not return a bounded 206 Content-Range")
        checks["range"] = {"status": status, "bytes_read": len(body), "content_range": content_range}

    if args.oidc_discovery_url:
        oidc_url = validate_url(args.oidc_discovery_url, "--oidc-discovery-url")
        status, _headers, body = http_get(oidc_url)
        try:
            document = json.loads(body)
        except json.JSONDecodeError as error:
            raise ProbeError("OIDC discovery response was not JSON") from error
        if status != 200:
            raise ProbeError("OIDC discovery endpoint did not return 200")
        issuer_url, jwks_url = validate_oidc_discovery_document(document)
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
        validate_podcast_xml(body)
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
    return validate_probe_artifact(results)


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
