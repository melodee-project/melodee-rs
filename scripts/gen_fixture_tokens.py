#!/usr/bin/env python3
"""Generate synthetic Phase 0 fixture JWTs for negative-path capture.

Prints a token to stdout so it can be injected via an environment variable
(never committed):

    export LEGACY_MELODEE_EXPIRED_JWT=$(python3 scripts/gen_fixture_tokens.py expired-jwt)

The token is signed with the committed, public, fixture-only key from
``scripts/legacy_oracle.py`` and is only valid against the disposable capture
database. It exists to characterize the legacy server's behavior with an
expired-but-well-formed credential.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import importlib.util
import json
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load_oracle():
    spec = importlib.util.spec_from_file_location("legacy_oracle", ROOT / "scripts" / "legacy_oracle.py")
    if spec is None or spec.loader is None:
        raise RuntimeError("unable to load legacy_oracle.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def make_jwt(key: str, *, expired: bool, user_api_key: str) -> str:
    header = {"alg": "HS256", "typ": "JWT"}
    now = int(time.time())
    payload = {
        "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress": "phase0-fixture@example.test",
        "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name": "phase0-fixture",
        "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/sid": user_api_key,
        "http://schemas.microsoft.com/ws/2008/06/identity/claims/role": "Administrator",
        "exp": now - 3600 if expired else now + 900,
        "iss": "MelodeeApi",
        "aud": "MelodeeClient",
    }
    signing_input = f"{b64url(json.dumps(header, separators=(',', ':')).encode())}.{b64url(json.dumps(payload, separators=(',', ':')).encode())}"
    signature = hmac.new(key.encode(), signing_input.encode(), hashlib.sha256).digest()
    return f"{signing_input}.{b64url(signature)}"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=["expired-jwt"])
    args = parser.parse_args()
    oracle = load_oracle()
    print(
        make_jwt(
            oracle.FIXTURE_JWT_KEY,
            expired=True,
            user_api_key=oracle.fixture_uuid("user", oracle.FIXTURE_USER),
        )
    )


if __name__ == "__main__":
    main()
