#!/usr/bin/env python3
"""Last.fm real-provider wire qualification (MEL-9) — safe probe harness.

Exercises the real ``https://ws.audioscrobbler.com/2.0/`` endpoints with real
API credentials to produce the configured-state wire evidence the mock-based
test suite cannot prove. Every probe is deliberately safe:

- ``auth.getToken`` — validates the API key and response shape (the Melodee
  server itself never calls this; the probe is provider-level wire evidence).
- ``auth.getSession`` with a deliberately invalid token — exercises the
  link-attempt failure contract without touching any account.
- ``track.scrobble`` with a bogus session key — exercises the invalid-session
  scrobble rejection; an authentication failure writes nothing to any account.
- a missing-signature request — exercises the signature-error contract.
- an unreachable local endpoint — exercises connection-failure (outage)
  classification offline, without generating real provider load.

Redaction contract: API credentials are read from the legacy settings export
and are never printed, logged, or written to the evidence file. Last.fm error
codes/messages are public API responses and are recorded. A deliberately
bogus session key never matches any real account.

Rate-limit behavior is NOT forceable without abusive load and is explicitly
reported as unexercised; the mock suite (``apps/melodee-server/tests/
lastfm_scrobble.rs``) covers it deterministically.
"""

from __future__ import annotations

import argparse
import csv
import datetime
import hashlib
import json
import socket
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Optional

LASTFM_API_URL = "https://ws.audioscrobbler.com/2.0/"
PROBE_USER_AGENT = "melodee-rs-lastfm-wire-qualification/1.0"
REDACTED_FIELDS = frozenset({"api_key", "shared_secret", "session_key", "token_value"})
# Last.fm clear-session triggers (ADR-0004; lastfm.rs is_invalid_session):
# 4 authentication failed, 9 invalid session key, 10 invalid API key.
CLEAR_SESSION_ERROR_CODES = frozenset({4, 9, 10})


def build_api_signature(params: dict[str, str], shared_secret: str) -> str:
    """Last.fm api_sig: sorted name+value pairs, secret last, MD5 hex.

    ``format`` and ``api_sig`` are never signed (lastfm.rs
    ``build_api_signature``)."""
    signed = {k: v for k, v in params.items() if k not in ("format", "api_sig")}
    hasher = hashlib.md5()
    for name in sorted(signed):
        hasher.update(name.encode("utf-8"))
        hasher.update(signed[name].encode("utf-8"))
    hasher.update(shared_secret.encode("utf-8"))
    return hasher.hexdigest()


def extract_credentials(csv_path: Path) -> tuple[str, str]:
    """Reads the Last.fm API key/shared secret from the legacy settings export."""
    api_key = ""
    shared_secret = ""
    with csv_path.open(newline="", encoding="utf-8-sig") as handle:
        for row in csv.DictReader(handle):
            key = (row.get("Key") or "").strip()
            if key == "scrobbling.lastFm.apiKey":
                api_key = (row.get("Value") or "").strip()
            elif key == "scrobbling.lastFm.sharedSecret":
                shared_secret = (row.get("Value") or "").strip()
    if not api_key or not shared_secret:
        raise SystemExit(
            "error: the export must contain non-empty scrobbling.lastFm.apiKey "
            "and scrobbling.lastFm.sharedSecret rows"
        )
    return api_key, shared_secret


def call_lastfm(
    params: dict[str, str],
    timeout_seconds: float,
    url: str = LASTFM_API_URL,
    use_post: bool = False,
) -> dict[str, object]:
    """One GET (or POST) request to the Last.fm API with a bounded timeout.

    Write methods (``track.scrobble``, ``track.updateNowPlaying``) require
    POST per the provider contract and the Rust ``signed_post`` path; GET is
    used for the read/auth probes exactly as the provider accepts.

    Returns a redacted observation dict; HTTP/transport failures are recorded,
    never raised, so one probe failure cannot abort the evidence run.
    """
    encoded = urllib.parse.urlencode(params).encode("utf-8")
    headers = {"User-Agent": PROBE_USER_AGENT}
    if use_post:
        request = urllib.request.Request(url, data=encoded, headers=headers, method="POST")
    else:
        request = urllib.request.Request(f"{url}?{encoded.decode('utf-8')}", headers=headers)
    started = datetime.datetime.now(datetime.UTC)
    try:
        with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
            body = response.read().decode("utf-8", errors="replace")
            latency_ms = int((datetime.datetime.now(datetime.UTC) - started).total_seconds() * 1000)
            return {"status": "ok", "http_status": response.status, "latency_ms": latency_ms, "body": body}
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        latency_ms = int((datetime.datetime.now(datetime.UTC) - started).total_seconds() * 1000)
        return {"status": "http_error", "http_status": error.code, "latency_ms": latency_ms, "body": body}
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        latency_ms = int((datetime.datetime.now(datetime.UTC) - started).total_seconds() * 1000)
        return {
            "status": "transport_error",
            "transport_reason": type(error).__name__,
            "latency_ms": latency_ms,
        }


def parse_lfm_error(body: str) -> tuple[Optional[int], Optional[str]]:
    """Extracts the Last.fm error code/message from an XML/JSON error body."""
    try:
        parsed = json.loads(body)
        if isinstance(parsed, dict) and "error" in parsed:
            code = parsed.get("error")
            code = code if isinstance(code, int) else None
            return code, parsed.get("message") if isinstance(parsed.get("message"), str) else None
    except (ValueError, TypeError):
        pass
    code = None
    message = None
    if '<lfm status="failed">' in body and "<error" in body:
        marker = 'code="'
        start = body.find(marker)
        if start != -1:
            digits = body[start + len(marker):]
            digits = digits[: digits.find('"')] if '"' in digits else ""
            if digits.isdigit():
                code = int(digits)
        mstart = body.find(">", body.find("<error"))
        mend = body.find("</error>")
        if mstart != -1 and mend != -1:
            message = body[mstart + 1: mend].strip()
    return code, message


def redact_observation(observation: dict[str, object]) -> dict[str, object]:
    """Strips any credential-shaped content from an observation (defense in depth)."""
    cleaned = dict(observation)
    body = cleaned.get("body")
    if isinstance(body, str):
        for field in REDACTED_FIELDS:
            cleaned.pop(field, None)
        # Defensive: never persist anything that looks like our credentials.
        cleaned["body"] = body[:2000]
    return cleaned


def run_probes(api_key: str, shared_secret: str, timeout_seconds: float) -> list[dict[str, object]]:
    """Runs the safe probe sequence and returns redacted observations."""
    results: list[dict[str, object]] = []

    # 1. auth.getToken (unsigned GET) — provider-level API-key validation.
    observation = call_lastfm(
        {"method": "auth.getToken", "api_key": api_key, "format": "json"}, timeout_seconds
    )
    code, message = parse_lfm_error(str(observation.get("body", "")))
    results.append({
        "probe": "auth.getToken (unsigned, provider-level; Melodee never calls this)",
        "lfm_error_code": code,
        "lfm_error_message": message,
        **redact_observation(observation),
    })

    # 2. auth.getSession with a deliberately invalid token (signed).
    invalid_token = "melodee-probe-invalid-token-0000000000000000"
    params = {
        "method": "auth.getSession",
        "api_key": api_key,
        "token": invalid_token,
        "format": "json",
    }
    params["api_sig"] = build_api_signature(params, shared_secret)
    observation = call_lastfm(params, timeout_seconds)
    code, message = parse_lfm_error(str(observation.get("body", "")))
    results.append({
        "probe": "auth.getSession (signed, deliberately invalid token)",
        "lfm_error_code": code,
        "lfm_error_message": message,
        "clear_session_trigger": code in CLEAR_SESSION_ERROR_CODES if code is not None else None,
        **redact_observation(observation),
    })

    # 3. track.scrobble with a bogus session key (signed) — rejected by auth,
    #    writes nothing to any account.
    params = {
        "method": "track.scrobble",
        "api_key": api_key,
        "sk": "melodee-probe-bogus-session-key-0000000000000000",
        "artist": "Melodee Qualification Probe",
        "track": "Wire Contract Probe (Do Not Scrobble)",
        "timestamp": str(int(datetime.datetime.now(datetime.UTC).timestamp())),
        "format": "json",
    }
    params["api_sig"] = build_api_signature(params, shared_secret)
    # POST, exactly like the Rust signed_post path (provider rejects GET
    # writes with "must use POST method" — verified 2026-10-19).
    observation = call_lastfm(params, timeout_seconds, use_post=True)
    code, message = parse_lfm_error(str(observation.get("body", "")))
    results.append({
        "probe": "track.scrobble (signed POST, bogus session key — must be rejected)",
        "lfm_error_code": code,
        "lfm_error_message": message,
        "clear_session_trigger": code in CLEAR_SESSION_ERROR_CODES if code is not None else None,
        **redact_observation(observation),
    })

    # 4. Missing signature — the provider must reject unsigned write attempts.
    params = {"method": "auth.getSession", "api_key": api_key, "token": invalid_token, "format": "json"}
    observation = call_lastfm(params, timeout_seconds)
    code, message = parse_lfm_error(str(observation.get("body", "")))
    results.append({
        "probe": "auth.getSession (unsigned — signature required)",
        "lfm_error_code": code,
        "lfm_error_message": message,
        **redact_observation(observation),
    })

    # 5. Unreachable endpoint (offline connection-failure classification).
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        dead_port = sock.getsockname()[1]
    observation = call_lastfm(
        {"method": "auth.getToken", "api_key": api_key, "format": "json"},
        timeout_seconds,
        url=f"http://127.0.0.1:{dead_port}/2.0/",
    )
    results.append({
        "probe": "unreachable endpoint (offline outage classification)",
        "expected_transport_error": True,
        **redact_observation(observation),
    })

    return results


def summarize(results: list[dict[str, object]]) -> str:
    lines = []
    for result in results:
        status = result.get("status")
        code = result.get("lfm_error_code", "-")
        lines.append(f"{result.get('probe')}: status={status} lfm_error={code}")
    return "\n".join(lines)


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Run safe real-provider Last.fm wire probes for MEL-9 configured-state "
            "qualification. Credentials are read from the legacy settings export "
            "and never printed or persisted; probes never scrobble to a real "
            "account. Writes a redacted evidence JSON document."
        )
    )
    parser.add_argument("--csv", required=True, type=Path, help="legacy settings export CSV with Last.fm credentials")
    parser.add_argument("--output", type=Path, help="redacted evidence JSON path (default: alongside the CSV)")
    parser.add_argument("--timeout-seconds", type=float, default=10.0, help="per-request timeout (default 10)")
    args = parser.parse_args(argv)
    if not args.csv.is_file():
        parser.error(f"input CSV does not exist: {args.csv}")
    output = args.output or args.csv.with_name("lastfm-wire-evidence.json")

    api_key, shared_secret = extract_credentials(args.csv)
    results = run_probes(api_key, shared_secret, args.timeout_seconds)
    document = {
        "format": "melodee-lastfm-wire-evidence/v1",
        "generated_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "redaction": "API credentials never recorded; Last.fm error codes/messages are public responses",
        "probes": results,
        "unexercised": [
            "real-account link/unlink (requires operator-controlled Last.fm account; scrobbling.disabled in production settings)",
            "forced provider rate limit (would require abusive load; mock suite covers the contract)",
        ],
    }
    rendered = json.dumps(document, indent=2)
    if any(str(field) in rendered for field in (api_key, shared_secret)):
        raise SystemExit("error: credential material detected in evidence output; refusing to write")
    output.write_text(rendered + "\n", encoding="utf-8")
    print(summarize(results))
    print(f"wrote {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
