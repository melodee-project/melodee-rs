.PHONY: bootstrap check check-full smoke fmt lint test test-smoke docs deny postgres-up postgres-down capture-baseline admin-ui-check

bootstrap:
	./scripts/bootstrap.sh

check:
	./scripts/check.sh

check-full:
	./scripts/check.sh

smoke:
	./scripts/check-smoke.sh

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace --all-features

test-smoke:
	cargo test --workspace --all-features --lib

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

capture-baseline:
	python3 scripts/capture_baseline.py
