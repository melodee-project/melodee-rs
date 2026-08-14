# ADR-0134: Phase 9 parity-domain schema for radio, avatars, onboarding, notifications, device profiles, scripts, and providers

- **Date:** 2026-08-13
- **Status:** Accepted
- **Decision owners:** Phase 9 work package
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-RADIO-001, PRD-RADIO-002, PRD-USER-001, PRD-SETUP-001,
  PRD-SETUP-002, PRD-CONFIG-001, PRD-CONFIG-002, PRD-CONFIG-003, PRD-AUTH-007, PRD-NOTIF-001,
  PRD-NOTIF-002, PRD-NOTIF-003, PRD-DEVICE-001, PRD-DEVICE-002, PRD-SCRIPT-001, PRD-SCRIPT-004,
  PRD-EXT-003, PRD-AUTH-002

## Context

Phase 9 domains need new durable state that no existing migration covers. AGENTS.md forbids any
database schema without an accepted schema ADR; this ADR is that schema authority for all Phase 9
work packages. Each work package still lands its own numbered migration, rollback/forward plan,
and data-integrity tests (per the database/data-safety rules); this ADR fixes the table inventory,
ownership, and invariants so those migrations are not designed ad hoc.

Existing stores being extended rather than duplicated:

- `opensubsonic_radio_stations` (migration 0020) is the complete captured radio-station model.
- `native_library_access_groups`, `native_user_library_groups`,
  `native_library_group_libraries` (migration 0010) already model group/ACL state; group CRUD is
  an API gap, not a schema gap.
- `native_user_social_logins` (migration 0010) models external account linking generically.

## Decision

1. **Internet radio (WP-06):** `opensubsonic_radio_stations` is the single authoritative radio
   store. The native-API radio surface reads/writes it through `melodee-persistence`. No rename
   and no parallel table: renaming a differentially tested Phase 8 table buys cosmetics at real
   migration risk. Adapter separation (ADR-0102) governs *wire DTOs*, not table names.
2. **Avatars (WP-03):** new `native_user_avatars` — one row per user (PK `user_id` FK
   `native_users`), `content_type`, bytea `image` (bounded, ≤ 1 MiB enforced in code and by a
   CHECK on `octet_length`), `updated_at` driving `avatarLastChanged` semantics. Serves the
   existing OpenSubsonic `getAvatar` read path and the new native upload/change route.
3. **Onboarding + settings (WP-08):** new `server_settings` (`key` text PK, `value` jsonb,
   `is_secret` bool, `updated_at`) for DB-backed settings layered under env/file configuration
   (PRD-CONFIG-003); and `onboarding_state` (singleton-enforced row: `id` bool PK CHECK `id`,
   `completed_at`, `completed_by`, `evidence` jsonb) implementing PRD-SETUP-002 bypass. Secret
   values are write-only over the API and redacted in every export (PRD-AUTH-007).
4. **Notifications (WP-09):** new `notification_templates` (`template_key`, `locale`, `subject`,
   `body_text`, `body_html` nullable, `version`, `updated_at`; PK (`template_key`, `locale`)) and
   `notification_deliveries` (append-only audit: `id`, `template_key`, `recipient`,
   `status`, `error`, `created_at`; bounded retention enforced by the worker). Delivery failures
   never roll back the triggering operation (PRD-NOTIF-002).
5. **Device profiles (WP-10):** new `device_profiles` (`id` uuid PK, `name`, `client_match`
   jsonb, `profile` jsonb, `is_default` bool, timestamps). Rows describe *metadata only*; no
   column or API may claim transcoding control (PRD-DEVICE-002). Profile-driven stream selection
   stays post-parity (PRD-DEVICE-003).
6. **Scripting (WP-11):** new `script_registry` (`id` uuid PK, `event`, `name`, `source` text,
   `enabled`, `timeout_ms`/`memory_bytes` limits, `version`, timestamps) and `script_runs`
   (append-only bounded audit: `id`, `script_id`, `event`, `outcome`, `duration_ms`, `error`,
   `created_at`). Scripts are operator-installed (admin trust boundary); there is no
   user-submitted script surface.
7. **External providers + Last.fm (WP-07):** new `external_provider_cache` (`provider`,
   `request_key` (hashed), `payload` jsonb, `fetched_at`, `expires_at`; PK
   (`provider`, `request_key`)) and `native_lastfm_links` (`user_id` PK FK `native_users`,
   `lastfm_username`, `session_key_digest` — hash only, never the raw key, `linked_at`).
   Provider payloads are cache/evidence only; they never become catalog identity (PRD-ARTIST-012,
   PRD-EXT-002).
8. **Worker-pool control (WP-05):** new `worker_pool_state` (`pool` text PK, `paused` bool,
   `paused_by`, `paused_at`, `resume_requested_at`, `updated_at`) giving pause/resume a durable,
   auditable, multi-process-safe home (PRD-WORK-005). Workers check the row between claim
   batches; pausing never strands a claimed lease — in-flight work finishes or is released per
   the existing lease contract.

Invariants for all of the above: additive-only migrations; every mutation bounded; no clear-text
secrets (PRD-AUTH-005); public identifiers are immutable once issued (ADR-0127); no destructive
operation without the Phase 5 durable decision/audit/authorization contract.

## Alternatives considered

### One ADR per table/domain

Benefits: finer-grained history. Costs: six near-identical ADRs whose content is "additive table
for a parity domain," slowing Wave 1/2 without adding review quality. Rejected; this ADR still
requires per-migration tests and rollback plans.

### Reuse key/value `server_settings` for everything

Benefits: one table. Costs: loses relational integrity, bounding, and audit shape for
notifications, scripts, avatars, and provider cache; pushes invariants into code. Rejected.

### Rename `opensubsonic_radio_stations` to a neutral name

Benefits: cleaner naming. Costs: a rename touches a Phase 8 differentially evidenced adapter for
zero behavior change and complicates Phase 11 import evidence. Rejected; naming follows the owning
migration, not the consuming adapter.

## Rationale

This matches the established pattern (ADR-0120 fixed the Phase 5 schema inventory up front, then
migrations implemented it): one schema ADR per phase increment, concrete tables and invariants,
per-WP migrations with integrity tests.

## Consequences

### Positive

- Wave 1/2 work packages have pre-authorized, concrete schema targets.
- Secrets, bounding, and audit invariants are decided once, consistently.

### Negative / trade-offs

- If a WP discovers a genuinely different shape is required, it must amend this ADR rather than
  improvise schema.

### Follow-up work

- WP-03, WP-06, WP-07, WP-08, WP-09, WP-10, WP-11 each land their numbered migration and tests.

## Validation

- `crates/melodee-persistence/tests/` gains migration/integrity tests per table (round-trip,
  bounds, cascade, redaction) with each WP.
- `python3 scripts/validate_repository.py` and `cargo deny check` stay green; `./scripts/check.sh`
  passes after every WP migration.

## References

- [PRD.md](../../PRD.md) §27, §31, §35, §36, §37, §44
- [ADR-0102](ADR-0102-preserve-public-apis-as-separate-adapters.md),
  [ADR-0105](ADR-0105-postgresql-is-authoritative-state-store.md),
  [ADR-0120](ADR-0120-phase5-classification-novelty-and-policy-schema.md),
  [ADR-0127](ADR-0127-native-public-id-migration-and-id-preserving-schema-for-api-v1.md)
- `design/agents/PHASE-9-PLAN.md` §4
