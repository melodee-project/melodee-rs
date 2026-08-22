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
| **R5** | Hardening & Security | ⬜ Pending | System armor, determinism, and stability. |
| **R6** | Operational Closure | ⬜ Pending | Cutover automation and release gates. |
| **UI-X** | Admin Experience | ⬜ Pending | Closing deferred curation features. |

---

## 2. Task-Based Implementation Backlog

### Phase R5: Hardening & Security
*Focus: Transforming security inventory into implemented code and ensuring runtime stability.*

#### Task R5.1: System Armor (Security Implementation)
- [ ] **Path Containment:** Implement strict path canonicalization and boundary checks for all path-consuming operations to prevent directory traversal.
- [ ] **Resource Guardrails:** Add hard size limits, timeouts, and mandatory cancellation propagation for all subprocesses and external provider requests.
- [ ] **Race Condition Protection:** Implement atomic operations or lock-checks for all filesystem removal/move operations to prevent TOCTOU vulnerabilities.
- [ ] **Secret Redaction:** Implement a redaction layer in the logging/diagnostics module to scrub credentials/secrets from logs.
- [ ] **HTTP Security Headers:** Configure and deploy the full security header suite (HSTS, CSP, etc.) across all API surfaces.

#### Task R5.2: Runtime Determinism & Stability
- [ ] **Test Suite Stabilization:** Identify and fix all timing-sensitive/flaky tests, replacing `sleep` calls with async synchronization.
- [ ] **Error Path Hardening:** Implement final edge-case handling for network/FS failures identified during the R4 production rehearsal.

---

### Phase R6: Operational Closure
*Focus: Creating the "Switch" — the code and scripts required to move to production safely.*

#### Task R6.1: Cutover Automation
- [ ] **Write-Freeze Logic:** Implement the mechanism to lock the legacy system into read-only mode during the final delta sync.
- [ ] **Final Delta Sync Script:** Create the production-ready script to perform the final data synchronization before cutover.

#### Task R6.2: Rollback & Recovery Tooling
- [ ] **Rollback Orchestrator:** Create the automation to revert API traffic to the legacy server and restore system state in the event of a critical v1.0 failure.
- [ ] **Integrity Verification Tool:** Implement a post-migration check tool to verify catalog counts and checksums between legacy and new systems.

#### Task R6.3: Production Health Monitoring
- [ ] **Advanced Health Probes:** Implement deep-check probes for PostgreSQL connectivity and Tantivy index health (beyond simple process checks).

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
