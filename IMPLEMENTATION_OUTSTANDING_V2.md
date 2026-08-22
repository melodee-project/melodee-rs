# Melodee 1.0.0 Release Closure Plan (V2)

**Document status:** Active Release Path
**Baseline date:** 2026-08-22
**Current State:** R4 (Validation) is considered complete.
**Objective:** Finalize all remaining code implementation, security hardening, and operational tooling to reach "Code Complete" for the 1.0.0 production release.

---

## 1. Release Phase Map

| Phase | Focus | Status | Primary Goal |
| :--- | :--- | :--- | :--- |
| **R0-R3** | Foundations & Tooling | ✅ Complete | Core logic and migration scripts. |
| **R4** | Real-World Validation | ✅ Complete | Production-scale and hardware acceptance. |
| **R5** | Hardening & Security | ✅ Complete | System armor, determinism, and stability. |
| **R6** | Operational Closure | ✅ Complete | Cutover automation, rollback tooling, and health monitoring. |
| **UI-X** | Admin Experience | ⬜ Pending | Closing deferred curation features. |

---

## 2. Task-Based Implementation Backlog

### Phase R5: Hardening & Security
*Focus: Transforming security inventory into implemented code and ensuring runtime stability.*

#### Task R5.1: System Armor (Security Implementation)
- [x] **Path Containment:** Implement strict path canonicalization and boundary checks for all path-consuming operations to prevent directory traversal.
- [x] **Resource Guardrails:** Add hard size limits, timeouts, and mandatory cancellation propagation for all subprocesses and external provider requests.
- [x] **Race Condition Protection:** Implement atomic operations or lock-checks for all filesystem removal/move operations to prevent TOCTOU vulnerabilities.
- [x] **Secret Redaction:** Implement a redaction layer in the logging/diagnostics module to scrub credentials/secrets from logs.
- [x] **HTTP Security Headers:** Configure and deploy the full security header suite (HSTS, CSP, etc.) across all API surfaces.

#### Task R5.2: Runtime Determinism & Stability
- [x] **Test Suite Stabilization:** Identify and fix all timing-sensitive/flaky tests, replacing `sleep` calls with async synchronization.
- [x] **Error Path Hardening:** Implement final edge-case handling for network/FS failures identified during the R4 production rehearsal.

**Status:** ✅ Complete as of 2026-08-22

**Evidence:**
- Security headers implemented in `apps/melodee-server/src/security_headers.rs`
- Admin API CSP, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Permissions-Policy` configured
- `melodee_server::security_headers::middleware` applied in `main.rs`
- HSTS documented at reverse proxy level in `/docs/src/content/docs/api/admin-api.md`
- Gitleaks secret scan in `.github/workflows/security.yml`
- CodeQL SAST in `.github/workflows/codeql.yml`
- Dependabot/Renovate in `.github/dependabot.yml`
- SMTP password test in `apps/melodee-server/tests/notification_smtp.rs::delivery_failure_logs_never_contain_the_smtp_password`
- Unit and integration tests in `apps/melodee-server/tests/security_headers_test.rs`
- Security inventory documented in `design/evidence/r1/security-baseline-inventory.md`

---

### Phase R6: Operational Closure
*Focus: Creating the "Switch" — the code and scripts required to move to production safely.*

#### Task R6.1: Cutover Automation
- [x] **Write-Freeze Logic:** Implemented as `melodee legacy freeze <RUN_ID>` in `apps/melodee-cli/src/legacy/freeze.rs`. Proves source write-stillness (polls legacy API every second for 10s), applies final convergent delta, verifies post-delta parity gate, and records durable freeze evidence with operator acknowledgement prompt.
- [x] **Final Delta Sync Script:** Implemented as `melodee legacy delta <RUN_ID>` in `apps/melodee-cli/src/legacy/delta.rs`. Convergent incremental pass that imports new rows by re-running idempotent import loops, updates changed catalog rows, and reports deletions as issues (never destructive). Used internally by `freeze` for the final convergent delta sync.

#### Task R6.2: Rollback & Recovery Tooling
- [x] **Rollback Orchestrator:** Implemented as `melodee rollback status` and `melodee rollback runbook --confirm` in `apps/melodee-cli/src/main.rs`. Inspects the latest migration run state, reports PostgreSQL reachability and migration currency, checks for freeze evidence, and generates a machine-readable JSON runbook with step-by-step instructions to revert traffic. Does not modify legacy code or infrastructure — produces audit evidence and documented operator steps. Runbook: `design/runbooks/CUTOVER-AND-ROLLBACK.md`.
- [x] **Integrity Verification Tool:** Implemented as `melodee legacy verify <RUN_ID>` in `apps/melodee-cli/src/legacy/verify.rs`. Performs source/target parity checks across all domain categories, bounded sample verification, deep checks (media checksums, relationship integrity), and records durable mismatch issues. Transitions run state from `verifying` to `delta-ready` on clean verification.

#### Task R6.3: Production Health Monitoring
- [x] **Advanced Health Probes:** Extended `melodee doctor --deep` in `apps/melodee-cli/src/main.rs` with five operational checks beyond basic local preflight: (1) PostgreSQL connectivity with bounded 5-second timeout, (2) migration currency check via `_sqlx_migrations` table, (3) catalog entity count sanity query, (4) Tantivy search index diagnostic (active generation validation with bounded timeout), and (5) PostgreSQL readiness probe (bounded, same contract as `/readyz`). The existing admin API already aggregates these subsystem checks on `GET /api/admin/v1/system/health`.

---

### Phase UI-X: Admin Experience Completion
*Focus: Closing the remaining functional gaps in the Admin SPA.*

#### Task UI-X.1: Metadata Curation
- [ ] **Metadata Edit Forms:** Implement the full UI forms and API integration for editing Artist, Release, and Track metadata.
- [ ] **Curation Workflow:** Implement the "Save/Apply" logic for metadata changes, including audit logging of the changes.

---

## 3. Definition of Code Complete (v1.0.0)

The system is considered **Code Complete** when:
1. All tasks in **R5**, **R6**, and **UI-X** are marked as complete.
2. `./scripts/check.sh` (Full Gate) is green.
3. Every requirement in `PRD.md` is either:
    - Marked as `Implemented` with linked test evidence.
    - Formally `Waived` via an accepted ADR.
4. Public documentation in `/docs` reflects all implemented 1.0.0 behavior.
5. The migration/rollback runbooks are tested and executable.
