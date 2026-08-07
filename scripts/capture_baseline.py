#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SCENARIOS = ROOT / "compatibility" / "scenarios" / "public.json"
SENSITIVE_QUERY_KEYS = {"p", "password", "t", "s", "jwt", "apikey", "token"}
SAFE_HEADERS = {
    "accept-ranges",
    "cache-control",
    "content-length",
    "content-type",
    "etag",
    "location",
    "x-total-count",
}


def redact_url(url: str) -> str:
    parsed = urllib.parse.urlsplit(url)
    query = []
    for key, value in urllib.parse.parse_qsl(parsed.query, keep_blank_values=True):
        if key.lower() in SENSITIVE_QUERY_KEYS:
            value = "<redacted>"
        query.append((key, value))
    return urllib.parse.urlunsplit(
        (parsed.scheme, parsed.netloc, parsed.path, urllib.parse.urlencode(query), parsed.fragment)
    )


def safe_name(value: str) -> str:
    return re.sub(r"[^a-zA-Z0-9._-]+", "-", value).strip("-") or "scenario"


def capture_scenario(
    base_url: str,
    scenario: dict[str, object],
    output: Path,
    bearer_token: str | None,
) -> dict[str, object]:
    name = str(scenario["name"])
    method = str(scenario.get("method", "GET")).upper()
    path = str(scenario["path"])
    auth = str(scenario.get("auth", "none"))
    allow_failure = bool(scenario.get("allowFailure", False))
    url = urllib.parse.urljoin(base_url.rstrip("/") + "/", path.lstrip("/"))

    headers = {"User-Agent": "melodee-rs-phase0-capture/1"}
    for key, value in dict(scenario.get("headers", {})).items():
        headers[str(key)] = str(value)
    if auth == "bearer":
        if not bearer_token:
            return {"name": name, "skipped": True, "reason": "bearer token not configured"}
        headers["Authorization"] = f"Bearer {bearer_token}"

    body = None
    if "json" in scenario:
        body = json.dumps(scenario["json"]).encode("utf-8")
        headers["Content-Type"] = "application/json"

    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    status = 0
    response_headers: dict[str, str] = {}
    response_body = b""
    error_message = None

    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            status = response.status
            response_headers = {
                key.lower(): value
                for key, value in response.headers.items()
                if key.lower() in SAFE_HEADERS
            }
            response_body = response.read()
    except urllib.error.HTTPError as error:
        status = error.code
        response_headers = {
            key.lower(): value
            for key, value in error.headers.items()
            if key.lower() in SAFE_HEADERS
        }
        response_body = error.read()
        error_message = str(error)
    except Exception as error:  # noqa: BLE001 - capture must persist transport failures
        error_message = f"{type(error).__name__}: {error}"
        if not allow_failure:
            raise

    stem = safe_name(name)
    content_type = response_headers.get("content-type", "")
    extension = ".bin"
    if "json" in content_type:
        extension = ".json"
        try:
            parsed = json.loads(response_body)
            response_body = (json.dumps(parsed, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
        except json.JSONDecodeError:
            pass
    elif "xml" in content_type or response_body.lstrip().startswith(b"<"):
        extension = ".xml"
    elif content_type.startswith("text/"):
        extension = ".txt"

    body_path = output / f"{stem}{extension}"
    body_path.write_bytes(response_body)
    metadata = {
        "name": name,
        "request": {
            "method": method,
            "url": redact_url(url),
            "auth": auth,
        },
        "response": {
            "status": status,
            "headers": response_headers,
            "bodyFile": body_path.name,
            "sha256": hashlib.sha256(response_body).hexdigest(),
            "bytes": len(response_body),
        },
        "error": error_message,
        "allowFailure": allow_failure,
    }
    (output / f"{stem}.metadata.json").write_text(
        json.dumps(metadata, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return metadata


def main() -> None:
    parser = argparse.ArgumentParser(description="Capture a sanitized Phase 0 legacy baseline")
    parser.add_argument(
        "--base-url",
        default=os.environ.get("LEGACY_MELODEE_BASE_URL", "http://127.0.0.1:5157"),
    )
    parser.add_argument("--scenarios", type=Path, default=DEFAULT_SCENARIOS)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    scenarios = json.loads(args.scenarios.read_text(encoding="utf-8"))
    timestamp = dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ")
    output = args.output or ROOT / "compatibility" / "captures" / timestamp
    output.mkdir(parents=True, exist_ok=False)

    bearer_token = os.environ.get("LEGACY_MELODEE_BEARER_TOKEN") or None
    results = []
    for scenario in scenarios:
        result = capture_scenario(args.base_url, scenario, output, bearer_token)
        results.append(result)
        status = "skipped" if result.get("skipped") else result.get("response", {}).get("status")
        print(f"{result['name']}: {status}")

    summary = {
        "capturedAtUtc": timestamp,
        "baseUrl": redact_url(args.base_url),
        "scenarioFile": str(args.scenarios.relative_to(ROOT)),
        "results": results,
    }
    (output / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    failures = [
        item for item in results
        if not item.get("skipped") and item.get("error") and not item.get("allowFailure")
    ]
    if failures:
        print(f"{len(failures)} required captures failed", file=sys.stderr)
        raise SystemExit(1)

    print(f"Capture written to {output.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
