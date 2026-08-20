#!/usr/bin/env bash
set -euo pipefail

missing=0
for command in cargo rustc python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Missing required command: $command" >&2
    missing=1
  fi
done

if [[ "$missing" -ne 0 ]]; then
  exit 1
fi

echo "Rust: $(rustc --version)"
echo "Cargo: $(cargo --version)"
echo "Python: $(python3 --version)"

python3 scripts/validate_repository.py
python3 scripts/extract_requirements.py --check
python3 scripts/audit_requirement_overrides.py
python3 -m unittest discover -s tests/python -p 'test_*.py' -v

if [[ ! -f Cargo.lock ]]; then
  echo "Generating Cargo.lock. Commit it with the first dependency-resolving change."
  cargo generate-lockfile
fi

cargo fmt --all --check
cargo test --workspace --all-features

echo "Bootstrap checks completed."
