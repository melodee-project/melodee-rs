PYTHON ?= python3

.PHONY: verify-android-client seed-client-data up down bootstrap prechecks check check-fast fmt lint test test-unit test-integration test-coverage docs docs-site-check deny postgres-up postgres-down postgres-reset capture-baseline admin-ui-check admin-ui-e2e audit audit-live corpus corpus-verify

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
	$(PYTHON) scripts/test_database.py -- cargo test --workspace --all-features

test-unit:
	$(PYTHON) scripts/check.py unit

test-integration:
	$(PYTHON) scripts/test_database.py -- cargo test --workspace --all-features --test "*"

test-coverage:
	$(PYTHON) scripts/check.py coverage

docs:
	$(PYTHON) scripts/ci_tasks.py repository-docs

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

corpus:
	$(PYTHON) scripts/generate_known_corpus.py --tier all

corpus-verify:
	$(PYTHON) scripts/verify_known_corpus.py --require-tier all \
		--emit-ui-expectations admin-ui/e2e/live/known-corpus.generated.json

audit-live:
	$(PYTHON) scripts/check.py audit-live

verify-android-client:
	$(PYTHON) scripts/verify_melodee_player_api.py

seed-client-data:
	$(PYTHON) scripts/seed_meloamp_parity_data.py

postgres-reset:
	$(PYTHON) scripts/reset_dev_database.py

capture-baseline:
	$(PYTHON) scripts/capture_baseline.py
