PYTHON ?= python3

.PHONY: up down bootstrap prechecks check check-fast fmt lint test test-unit test-integration test-coverage docs docs-site-check deny postgres-up postgres-down postgres-reset capture-baseline admin-ui-check admin-ui-e2e audit audit-live

bootstrap:
	$(PYTHON) scripts/bootstrap.py

up:
	$(PYTHON) scripts/dev_stack.py up

down:
	$(PYTHON) scripts/dev_stack.py down

prechecks:
	$(PYTHON) scripts/do-prechecks.py

check:
	$(PYTHON) scripts/check.py full

check-fast:
	$(PYTHON) scripts/check.py fast

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace --all-features

test-unit:
	cargo test --workspace --all-features --lib --bins

test-integration:
	cargo test --workspace --all-features --test "*"

test-coverage:
	$(PYTHON) scripts/check.py coverage

docs:
	$(PYTHON) scripts/validate_repository.py
	$(PYTHON) scripts/extract_requirements.py --check

docs-site-check:
	$(PYTHON) scripts/check.py docs

deny:
	cargo deny check

admin-ui-check:
	$(PYTHON) scripts/check.py admin-ui

admin-ui-e2e:
	$(PYTHON) scripts/check.py admin-ui-e2e

postgres-up:
	docker compose -f compose.dev.yaml up -d postgres

postgres-down:
	docker compose -f compose.dev.yaml down

audit:
	$(PYTHON) scripts/audit_runtime_invariants.py

audit-live:
	$(PYTHON) scripts/check.py audit-live

postgres-reset:
	$(PYTHON) scripts/reset_dev_database.py

capture-baseline:
	$(PYTHON) scripts/capture_baseline.py
