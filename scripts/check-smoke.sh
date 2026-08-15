#!/usr/bin/env bash
set -euo pipefail

# Fast, broad feedback for normal iteration. This intentionally excludes
# PostgreSQL integration, compatibility captures, UI builds, and dependency
# policy auditing; use scripts/check.sh before merge/release or after changes
# to those areas.
python3 scripts/validate_repository.py
python3 scripts/extract_requirements.py --check
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --lib
