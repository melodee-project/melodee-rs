.PHONY: bootstrap check check-fast fmt lint test test-unit test-integration docs deny postgres-up postgres-down postgres-reset capture-baseline admin-ui-check

bootstrap:
	python3 scripts/bootstrap.py

check:
	python3 scripts/check.py full

check-fast:
	python3 scripts/check.py fast

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace --all-features

test-unit:
	cargo test --workspace --all-features --lib --bins

test-integration:
	cargo test --workspace --all-features --test '*'

docs:
	python3 scripts/validate_repository.py
	python3 scripts/extract_requirements.py --check

deny:
	cargo deny check

admin-ui-check:
	cd admin-ui && pnpm install --frozen-lockfile && pnpm check && pnpm lint && pnpm i18n:check && pnpm test && pnpm build

postgres-up:
	docker compose -f compose.dev.yaml up -d postgres

postgres-down:
	docker compose -f compose.dev.yaml down

postgres-reset:
	python3 scripts/reset_dev_database.py

capture-baseline:
	python3 scripts/capture_baseline.py
