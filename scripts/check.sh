#!/usr/bin/env bash
set -euo pipefail

python3 scripts/validate_repository.py
python3 scripts/extract_requirements.py --check
python3 scripts/audit_requirement_overrides.py
python3 scripts/validate_r4_client_acceptance.py
python3 -m unittest discover -s tests/python -p 'test_*.py' -v

# Admin SPA gate (ADR-0138): skipped locally when pnpm is absent; CI installs it.
if command -v pnpm >/dev/null 2>&1; then
  (cd admin-ui && pnpm install --frozen-lockfile && pnpm check && pnpm lint && pnpm i18n:check && pnpm test && pnpm build)
else
  echo "pnpm not installed; skipping local admin-ui gate (CI installs it)." >&2
fi
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check
else
  echo "cargo-deny not installed; skipping local dependency audit (CI installs it)." >&2
fi
