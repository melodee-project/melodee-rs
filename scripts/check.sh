#!/usr/bin/env bash
set -euo pipefail

python3 scripts/validate_repository.py
python3 scripts/extract_requirements.py --check
python3 -m unittest discover -s tests/python -p 'test_*.py' -v
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check
else
  echo "cargo-deny not installed; skipping local dependency audit (CI installs it)." >&2
fi
