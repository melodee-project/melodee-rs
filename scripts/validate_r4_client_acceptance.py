#!/usr/bin/env python3
"""Compatibility wrapper for the historical R4 client acceptance validator."""

from __future__ import annotations

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from validate_client_acceptance import (  # noqa: E402,F401
    DEFAULT_MATRIX,
    EXPECTED_ROWS,
    REQUIRED_WORKFLOWS,
    ValidationError,
    load_and_validate,
    main,
    validate_matrix,
)


if __name__ == "__main__":
    raise SystemExit(main())
