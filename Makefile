.PHONY: bootstrap check fmt lint test docs deny postgres-up postgres-down capture-baseline

bootstrap:
	./scripts/bootstrap.sh

check:
	./scripts/check.sh

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace --all-features

docs:
	python3 scripts/validate_repository.py
	python3 scripts/extract_requirements.py --check

deny:
	cargo deny check

postgres-up:
	docker compose -f compose.dev.yaml up -d postgres

postgres-down:
	docker compose -f compose.dev.yaml down

capture-baseline:
	python3 scripts/capture_baseline.py
