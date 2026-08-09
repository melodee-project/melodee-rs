#!/usr/bin/env python3
"""Capture sanitized Phase 0 legacy behavior into ``compatibility/captures/``.

Scenario format (schemaVersion 2, backward compatible with Issue 4 files):

- Simple scenario: ``name``, ``surface``, ``method``, ``path``, ``auth``,
  optional ``headers``, ``json`` body, ``allowFailure``, ``normalize``.
- Flow scenario: a ``flow`` array of steps. Steps support ``{{variable}}``
  substitution in ``path``/``headers``/``json`` from values captured out of
  earlier step responses (``capture: {var: "$.json.path"}``), plus
  ``expectStatus`` assertions. ``auth: "bearer-var:<name>"`` sends a captured
  access token as the Bearer credential.

Sanitization guarantees (enforced at capture time, before anything is written):

- Sensitive query parameters (``p``, ``password``, ``t``, ``s``, ``jwt``,
  ``apikey``, ``token``) are redacted in recorded request URLs.
- Values captured into variables marked ``secretCapture`` are replaced by
  ``<captured:name>`` in recorded request URLs/headers/bodies.
- The HMAC auth-token path segment of ``/song/stream/{song}/{user}/{token}``
  is masked in recorded URLs and in JSON string values at
  ``normalize.maskUrlAuthTokenPaths``; its structure is validated separately
  via ``normalize.structureChecks``.
- ``normalize.maskJsonPaths`` values are replaced with ``<volatile>`` in the
  stored body (compare-time masking remains idempotent).
- ``normalize.maskBaseUrl`` rewrites the configured origin to
  ``{melodee-origin}`` in stored bodies and recorded URLs so host/port
  volatility cannot contaminate golden fixtures while URL paths and
  identifier construction remain validated.
- ``normalize.structureChecks`` (``jwt``, ``streamAuthToken``, ``opaqueToken``)
  validate token structure at capture time and record the result in metadata;
  a failed check fails the scenario.
"""
from __future__ import annotations

import argparse
import base64
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
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SCENARIOS = ROOT / "compatibility" / "scenarios" / "public.json"
SENSITIVE_QUERY_KEYS = {"p", "password", "t", "s", "jwt", "apikey", "token"}
SENSITIVE_BODY_KEYS = {"password", "refreshtoken", "token", "secret", "apikey"}
SAFE_HEADERS = {
    "accept-ranges",
    "access-control-allow-origin",
    "cache-control",
    "content-disposition",
    "content-duration",
    "content-length",
    "content-range",
    "content-type",
    "etag",
    "expires",
    "last-modified",
    "location",
    "x-total-count",
}
MASK = "<volatile>"
ORIGIN_ALIAS = "{melodee-origin}"
STREAM_URL_RE = re.compile(
    r"(/song/stream/[0-9a-fA-F-]{36}/[0-9a-fA-F-]{36}/)([^/?#\s]+)"
)
CAPTURE_UA = "melodee-rs-phase0-capture/1"


# ---------------------------------------------------------------------------
# URL / body redaction
# ---------------------------------------------------------------------------

def redact_url(url: str) -> str:
    parsed = urllib.parse.urlsplit(url)
    query = []
    for key, value in urllib.parse.parse_qsl(parsed.query, keep_blank_values=True):
        if key.lower() in SENSITIVE_QUERY_KEYS:
            value = "<redacted>"
        query.append((key, value))
    redacted = urllib.parse.urlunsplit(
        (parsed.scheme, parsed.netloc, parsed.path, urllib.parse.urlencode(query), parsed.fragment)
    )
    return STREAM_URL_RE.sub(r"\1<hmac-auth-token>", redacted)


def mask_stream_auth_tokens(value: Any) -> Any:
    """Mask the HMAC token segment of stream URLs inside JSON string values."""
    if isinstance(value, str):
        return STREAM_URL_RE.sub(r"\1<hmac-auth-token>", value)
    if isinstance(value, dict):
        return {key: mask_stream_auth_tokens(item) for key, item in value.items()}
    if isinstance(value, list):
        return [mask_stream_auth_tokens(item) for item in value]
    return value


def replace_origin(value: Any, origin: str) -> Any:
    if isinstance(value, str):
        return value.replace(origin, ORIGIN_ALIAS)
    if isinstance(value, dict):
        return {key: replace_origin(item, origin) for key, item in value.items()}
    if isinstance(value, list):
        return [replace_origin(item, origin) for item in value]
    return value


def redact_body_keys(document: Any) -> Any:
    if isinstance(document, dict):
        return {
            key: ("<redacted>" if key.lower() in SENSITIVE_BODY_KEYS else redact_body_keys(item))
            for key, item in document.items()
        }
    if isinstance(document, list):
        return [redact_body_keys(item) for item in document]
    return document


def safe_name(value: str) -> str:
    return re.sub(r"[^a-zA-Z0-9._-]+", "-", value).strip("-") or "scenario"


def apply_mask_regex(text: str, rules: list[dict[str, str]]) -> str:
    """Apply scenario-declared regex masks to text bodies (XML, JSONP, plain
    text) where JSON-path masking cannot reach. Each rule: {"pattern": ...,
    "replace": ...}."""
    for rule in rules:
        text = re.sub(str(rule["pattern"]), str(rule.get("replace", MASK)), text)
    return text


# ---------------------------------------------------------------------------
# JSON path subset (shared semantics with compare_captures.py)
# ---------------------------------------------------------------------------

_SEGMENT_RE = re.compile(r"\.([^\.\[\]]+)|\[(\*|\d+)\]")


def parse_json_path(path: str) -> list[str | int]:
    if not path.startswith("$"):
        raise ValueError(f"json path must start with '$': {path}")
    segments: list[str | int] = []
    position = 1
    while position < len(path):
        match = _SEGMENT_RE.match(path, position)
        if not match:
            raise ValueError(f"unsupported json path syntax at {position}: {path}")
        key, index = match.groups()
        if key is not None:
            segments.append(key)
        elif index == "*":
            segments.append("*")
        else:
            segments.append(int(index))
        position = match.end()
    return segments


def mask_json_path(document: Any, segments: list[str | int]) -> Any:
    if not segments:
        return MASK
    head, rest = segments[0], segments[1:]
    if isinstance(document, dict):
        if head not in document:
            return document
        return {**document, head: mask_json_path(document[head], rest)}
    if isinstance(document, list):
        if head == "*":
            return [mask_json_path(item, rest) for item in document]
        if isinstance(head, int) and 0 <= head < len(document):
            return [
                mask_json_path(item, rest) if index == head else item
                for index, item in enumerate(document)
            ]
        return document
    return document


def extract_json_path(document: Any, segments: list[str | int]) -> Any:
    """Extract a single value; ``*`` selects the first array element."""
    if not segments:
        return document
    head, rest = segments[0], segments[1:]
    if isinstance(document, dict) and head in document:
        return extract_json_path(document[head], rest)
    if isinstance(document, list) and document:
        index = 0 if head == "*" else head if isinstance(head, int) else None
        if index is not None and 0 <= index < len(document):
            return extract_json_path(document[index], rest)
    raise KeyError(f"json path not present: {head}")


# ---------------------------------------------------------------------------
# Token structure validation
# ---------------------------------------------------------------------------

def _b64url_decode(value: str) -> bytes:
    return base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))


def check_jwt(token: str) -> dict[str, Any]:
    parts = token.split(".")
    result: dict[str, Any] = {"type": "jwt", "valid": False}
    if len(parts) != 3:
        result["reason"] = "expected 3 segments"
        return result
    try:
        header = json.loads(_b64url_decode(parts[0]))
        payload = json.loads(_b64url_decode(parts[1]))
    except (ValueError, json.JSONDecodeError) as error:
        result["reason"] = f"undecodable segment: {error}"
        return result
    result["headerAlg"] = header.get("alg")
    result["payloadClaims"] = sorted(payload.keys())
    result["issuer"] = payload.get("iss")
    result["audience"] = payload.get("aud")
    expires = payload.get("exp")
    result["expiresInFuture"] = isinstance(expires, int) and expires > int(dt.datetime.now(dt.UTC).timestamp())
    result["valid"] = (
        result["headerAlg"] == "HS256"
        and bool(payload)
        and result["expiresInFuture"]
        and len(parts[2]) > 0
    )
    return result


def check_stream_auth_token(token: str, *, user_api_key: str | None = None,
                            song_api_key: str | None = None) -> dict[str, Any]:
    result: dict[str, Any] = {"type": "streamAuthToken", "valid": False}
    try:
        raw = urllib.parse.unquote(token)
        # The oracle's ToBase64 extension strips padding; tolerate that.
        raw += "=" * (-len(raw) % 4)
        decoded = base64.b64decode(raw).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as error:
        result["reason"] = f"not base64: {error}"
        return result
    parts = decoded.split("|")
    if len(parts) != 3:
        result["reason"] = f"expected 3 '|' parts, got {len(parts)}"
        return result
    data, timestamp, hmac_value = parts
    data_parts = data.split(":")
    result["binding"] = data_parts[2] if len(data_parts) == 3 else None
    try:
        expiry = dt.datetime.fromtimestamp(int(timestamp), dt.UTC)
    except ValueError:
        result["reason"] = "bad expiry timestamp"
        return result
    result["expiresAt"] = expiry.strftime("%Y-%m-%dT%H:%M:%SZ")
    result["expiresInFuture"] = expiry > dt.datetime.now(dt.UTC)
    result["expiresInDays"] = round((expiry - dt.datetime.now(dt.UTC)).total_seconds() / 86400)
    result["hmacShape"] = "base64-sha256" if re.fullmatch(r"[A-Za-z0-9+/=]{44}", hmac_value) else "unexpected"
    result["userMatchesUrl"] = (len(data_parts) == 3 and data_parts[0].lower() == (user_api_key or "").lower()) or None
    result["songMatchesUrl"] = (len(data_parts) == 3 and data_parts[1].lower() == (song_api_key or "").lower()) or None
    result["valid"] = (
        len(data_parts) == 3
        and result["expiresInFuture"]
        and result["hmacShape"] == "base64-sha256"
        and result["userMatchesUrl"] is not False
        and result["songMatchesUrl"] is not False
    )
    return result


def check_opaque_token(token: str) -> dict[str, Any]:
    return {
        "type": "opaqueToken",
        "valid": isinstance(token, str) and len(token) >= 16,
        "length": len(token) if isinstance(token, str) else 0,
    }


# ---------------------------------------------------------------------------
# Capture engine
# ---------------------------------------------------------------------------

def substitute(value: Any, variables: dict[str, str]) -> Any:
    if isinstance(value, str):
        for env_name in re.findall(r"\{\{env:([A-Za-z0-9_]+)\}\}", value):
            env_value = os.environ.get(env_name)
            if env_value is None:
                raise KeyError(f"environment variable {env_name} not set")
            value = value.replace("{{env:" + env_name + "}}", env_value)
        for name, replacement in variables.items():
            value = value.replace("{{" + name + "}}", replacement)
        return value
    if isinstance(value, dict):
        return {key: substitute(item, variables) for key, item in value.items()}
    if isinstance(value, list):
        return [substitute(item, variables) for item in value]
    return value


def unsubstitute_secrets(text: str, secrets: dict[str, str]) -> str:
    for name, value in secrets.items():
        if value:
            text = text.replace(value, f"<captured:{name}>")
    return text


def capture_request(
    base_url: str,
    name: str,
    scenario: dict[str, Any],
    output: Path,
    variables: dict[str, str],
    secrets: dict[str, str],
    bearer_token: str | None,
) -> dict[str, Any]:
    method = str(scenario.get("method", "GET")).upper()
    auth = str(scenario.get("auth", "none"))
    allow_failure = bool(scenario.get("allowFailure", False))
    expect_status = scenario.get("expectStatus")

    # Env-injected values are potential credentials (subsonic tokens, forged
    # JWTs); register them as secrets so recordings never persist them.
    for env_name in re.findall(r"\{\{env:([A-Za-z0-9_]+)\}\}", json.dumps(scenario)):
        env_value = os.environ.get(env_name)
        if env_value:
            secrets.setdefault(f"env:{env_name}", env_value)

    raw_path = str(scenario["path"])
    path = str(substitute(raw_path, variables))
    url = path if path.startswith("http") else urllib.parse.urljoin(
        base_url.rstrip("/") + "/", path.lstrip("/")
    )

    headers = {"User-Agent": CAPTURE_UA}
    for key, value in dict(scenario.get("headers", {})).items():
        headers[str(key)] = str(substitute(str(value), variables))
    if auth == "bearer":
        if not bearer_token:
            return {"name": name, "skipped": True, "reason": "bearer token not configured"}
        headers["Authorization"] = f"Bearer {bearer_token}"
    elif auth.startswith("bearer-var:"):
        var_name = auth.split(":", 1)[1]
        token = variables.get(var_name)
        if not token:
            return {"name": name, "skipped": True, "reason": f"variable {var_name} not captured"}
        headers["Authorization"] = f"Bearer {token}"

    templated_body = scenario.get("json")
    body = None
    if templated_body is not None:
        body = json.dumps(substitute(templated_body, variables)).encode("utf-8")
        headers["Content-Type"] = "application/json"
    elif scenario.get("body") is not None:
        body = str(substitute(str(scenario["body"]), variables)).encode("utf-8")

    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    if scenario.get("followRedirects") is False:
        class _NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, req, fp, code, msg, headers, newurl):
                return None

        opener = urllib.request.build_opener(_NoRedirect)
        open_fn = opener.open
    else:
        open_fn = urllib.request.urlopen
    status = 0
    response_headers: dict[str, str] = {}
    response_body = b""
    error_message = None

    try:
        with open_fn(request, timeout=30) as response:
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

    if expect_status is not None:
        if status != int(expect_status):
            error_message = f"expected status {expect_status}, got {status}"
            if not allow_failure:
                raise SystemExit(f"{name}: {error_message}")
        else:
            # An expected non-2xx (e.g. 401/400 negative-path evidence) is a
            # successful capture, not a transport failure.
            error_message = None

    normalize = dict(scenario.get("normalize", {}))
    origin = urllib.parse.urlsplit(base_url)
    origin_text = f"{origin.scheme}://{origin.netloc}"

    parsed_body: Any = None
    content_type = response_headers.get("content-type", "")
    extension = ".bin"
    if "json" in content_type:
        extension = ".json"
        try:
            parsed_body = json.loads(response_body)
        except json.JSONDecodeError:
            pass
    elif "xml" in content_type or response_body.lstrip().startswith(b"<"):
        extension = ".xml"
    elif content_type.startswith("text/") or "javascript" in content_type:
        extension = ".txt"

    # Structure checks run on the RAW body before any masking.
    structure_results: dict[str, Any] = {}
    if parsed_body is not None:
        for json_path, check_type in dict(normalize.get("structureChecks", {})).items():
            try:
                token_value = extract_json_path(parsed_body, parse_json_path(json_path))
            except KeyError:
                structure_results[json_path] = {"type": check_type, "valid": False, "reason": "path absent"}
                continue
            context: dict[str, str] = {}
            match = STREAM_URL_RE.search(str(token_value))
            if match:
                segments = str(token_value).split("/song/stream/", 1)[1].split("/")
                context = {"song_api_key": segments[0], "user_api_key": segments[1]}
                token_value = match.group(2)
            if check_type == "jwt":
                structure_results[json_path] = check_jwt(str(token_value))
            elif check_type == "streamAuthToken":
                structure_results[json_path] = check_stream_auth_token(str(token_value), **context)
            elif check_type == "opaqueToken":
                structure_results[json_path] = check_opaque_token(token_value)
            else:
                structure_results[json_path] = {"type": check_type, "valid": False, "reason": "unknown check"}
        failed = [p for p, r in structure_results.items() if not r.get("valid")]
        if failed and not allow_failure:
            raise SystemExit(f"{name}: structure checks failed for {failed}")

    # Masking pipeline for the STORED body (golden-safe by construction).
    stored_body = response_body
    if parsed_body is not None:
        masked = parsed_body
        if normalize.get("maskUrlAuthTokenPaths"):
            masked = mask_stream_auth_tokens(masked)
        for json_path in normalize.get("maskJsonPaths", []):
            masked = mask_json_path(masked, parse_json_path(json_path))
        if normalize.get("maskBaseUrl"):
            masked = replace_origin(masked, origin_text)
        masked = json.loads(json.dumps(masked))  # defensive copy
        stored_body = (json.dumps(masked, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    else:
        text_body = response_body
        if extension in {".xml", ".txt"}:
            decoded = response_body.decode("utf-8", "replace")
            if normalize.get("maskRegex"):
                decoded = apply_mask_regex(decoded, list(normalize["maskRegex"]))
            if normalize.get("maskBaseUrl"):
                decoded = decoded.replace(origin_text, ORIGIN_ALIAS)
            text_body = decoded.encode("utf-8")
        stored_body = text_body

    if normalize.get("maskBaseUrl"):
        response_headers = {
            key: value.replace(origin_text, ORIGIN_ALIAS) for key, value in response_headers.items()
        }
    # Redirect targets can carry credentials in their query strings (the error
    # pipeline's ReturnUrl embeds the original request, including t/s/jwt).
    if "location" in response_headers:
        response_headers["location"] = redact_url(
            unsubstitute_secrets(response_headers["location"], secrets)
        )

    stem = safe_name(name)
    body_path = output / f"{stem}{extension}"
    body_path.write_bytes(stored_body)

    # The request URL's origin is always host/port volatility; alias it even
    # when body masking (maskBaseUrl) is not requested.
    recorded_url = redact_url(unsubstitute_secrets(url, secrets)).replace(origin_text, ORIGIN_ALIAS)
    recorded_headers = {
        key: unsubstitute_secrets(value, secrets)
        for key, value in headers.items()
        if key.lower() not in {"authorization", "cookie"}
    }
    if "Authorization" in headers:
        # Never record a credential value, synthetic or otherwise.
        scheme, _, _ = headers["Authorization"].partition(" ")
        recorded_headers["Authorization"] = f"{scheme} <redacted>" if scheme else "<redacted>"
    recorded_body = (
        redact_body_keys(json.loads(json.dumps(templated_body))) if templated_body is not None else None
    )
    recorded_raw_body = None
    if scenario.get("body") is not None:
        recorded_raw_body = redact_url(unsubstitute_secrets(str(scenario["body"]), secrets))

    metadata = {
        "name": name,
        "surface": scenario.get("surface"),
        "request": {
            "method": method,
            "url": recorded_url,
            "auth": auth,
            "headers": recorded_headers,
            "jsonTemplate": recorded_body,
            "bodyTemplate": recorded_raw_body,
        },
        "response": {
            "status": status,
            "headers": response_headers,
            "bodyFile": body_path.name,
            "sha256": hashlib.sha256(stored_body).hexdigest(),
            "bytes": len(stored_body),
        },
        "normalize": normalize,
        "structureChecks": structure_results or None,
        "error": error_message,
        "allowFailure": allow_failure,
    }
    (output / f"{stem}.metadata.json").write_text(
        json.dumps(metadata, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    # Variable capture for subsequent flow steps (from the raw body).
    if parsed_body is not None:
        for var_name, json_path in dict(scenario.get("capture", {})).items():
            try:
                value = extract_json_path(parsed_body, parse_json_path(json_path))
            except KeyError:
                raise SystemExit(f"{name}: cannot capture {var_name} from {json_path}")
            variables[var_name] = str(value)
        for var_name, spec in dict(scenario.get("captureRegex", {})).items():
            try:
                value = extract_json_path(parsed_body, parse_json_path(str(spec["path"])))
            except KeyError:
                raise SystemExit(f"{name}: cannot capture {var_name} from {spec['path']}")
            match = re.search(str(spec["regex"]), str(value))
            if not match:
                raise SystemExit(f"{name}: regex did not match for {var_name}")
            variables[var_name] = match.group(1)
        for var_name in scenario.get("secretCapture", []):
            if var_name in variables:
                secrets[var_name] = variables[var_name]
    # Header capture (e.g. ETag for If-None-Match flows).
    for var_name, header_name in dict(scenario.get("captureHeaders", {})).items():
        value = response_headers.get(str(header_name).lower())
        if value is None:
            raise SystemExit(f"{name}: cannot capture header {header_name}")
        variables[var_name] = value

    return metadata


def iter_steps(scenario: dict[str, Any]) -> list[tuple[str, dict[str, Any]]]:
    flow = scenario.get("flow")
    if isinstance(flow, list):
        inherited = {key: scenario[key] for key in ("surface",) if key in scenario}
        return [
            (
                f"{scenario['name']}/{step.get('name', f'step-{index + 1}')}",
                {**inherited, **step},
            )
            for index, step in enumerate(flow)
        ]
    return [(str(scenario["name"]), scenario)]


def capture_scenario(
    base_url: str,
    scenario: dict[str, Any],
    output: Path,
    bearer_token: str | None,
) -> list[dict[str, Any]]:
    variables: dict[str, str] = {"baseUrl": base_url.rstrip("/")}
    secrets: dict[str, str] = {}
    results = []
    for step_name, step in iter_steps(scenario):
        try:
            results.append(
                capture_request(base_url, step_name, step, output, variables, secrets, bearer_token)
            )
        except KeyError as missing:
            results.append({"name": step_name, "skipped": True, "reason": str(missing)})
    return results


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
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
        for result in capture_scenario(args.base_url, scenario, output, bearer_token):
            results.append(result)
            status = "skipped" if result.get("skipped") else result.get("response", {}).get("status")
            print(f"{result['name']}: {status}")

    summary = {
        "capturedAtUtc": timestamp,
        "baseUrl": redact_url(args.base_url),
        "scenarioFile": str(
            args.scenarios.resolve().relative_to(ROOT)
            if args.scenarios.resolve().is_relative_to(ROOT)
            else args.scenarios.resolve()
        ),
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

    resolved_output = output.resolve()
    display = (
        resolved_output.relative_to(ROOT)
        if resolved_output.is_relative_to(ROOT)
        else resolved_output
    )
    print(f"Capture written to {display}")


if __name__ == "__main__":
    main()
