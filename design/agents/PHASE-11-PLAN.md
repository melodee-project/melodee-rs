# Canonical Phase 11 Plan

Status: **Complete with operator blockers** — safe local work is closed; production, external
acceptance, publication, and cleanup gates remain operator-controlled.

Date opened and closed for safe local scope: 2026-08-25

## Objective and authority

Prove that a supported C# Melodee installation can be migrated to Rust without losing public
identity, catalog relationships, media references, users, user state, or governed configuration,
and that the target can be backed up, restored, verified, operated, rolled back, packaged, and
released safely. The authoritative closeout is
[PHASE-11-REPORT.md](../evidence/phase-11/PHASE-11-REPORT.md).

Scope: MIGRATE-001, MIGRATE-002, PACKAGING-001, applicable OPS-001 obligations,
PRD-MIG-001..007, PRD-DEPLOY-001..006, PRD-OBS-001..005, PRD-SEC-006..007, and
PRD-TEST-001..007. Governing decisions are ADR-0144, ADR-0148, ADR-0149, ADR-0150, ADR-0151,
the accepted compatibility/security decisions, and ADR-0161.

## Baselines

- Rust: 91167a1ad90d293d5fc82b2bb3a13d61b856eb00, main, clean at entry.
- C# oracle: 4028b58dbd3302ed431e2c5b32d9dde5634db218, sph.2026-07-11.01, clean; read-only.
- MeloAmp: 096c76d810af91205aeee54da58131766b0f076b, sph.2026-07-14.01, clean; read-only.

Historical R3 is synthetic/production-shaped local importer evidence, R4 is isolated rehearsal
evidence, and R5 is historical packaging/hardening evidence. None substitutes for an operator
approved production operation.

## Dependency-ordered workstreams and outcome

| Workstream | Dependency and owner | Safe local outcome | Remaining gate |
|---|---|---|---|
| Entry audit and traceability | Agent; first | Baselines, source inventory, historical evidence classification, requirement/ADR review, and approval register | None |
| A. Migration state machine | Agent; after entry | Read-only source, schema/snapshot gates, durable checkpoints/leases/fencing, idempotent import/delta, durable issues, status, and validate/repair surface | Production source access |
| B. Domain mappings | Agent; after source audit | 52-table inventory with import, transform, regeneration, exclusion, or issue dispositions; no silent domain skip | Data-owner review on approved snapshot |
| C. Identity | Agent; with mappings | Public-ID preservation, conflict issue behavior, repeat/delta mapping, relationship verification | Migrated production API acceptance |
| D. Credentials/secrets | Agent; with identity | Supported bcrypt verification, unsupported reset-required behavior, secret re-sealing, missing/wrong-key failure, redaction | Live OIDC/provider acceptance |
| E. Media safety | Agent; with mappings | In-place adoption, path/symlink/size checks, hash sentinel rules, read-only validation and confirmed idempotent target metadata repair | Approved production media mount |
| F. Rehearsal/recovery | Agent; after A–E | 12,000-Track synthetic inventory/import/verify/search/backup/restore/freeze/rollback drill; retained failed target | Private production-sized rehearsal |
| G. Differential verification | Agent; after import | Existing native-v1/OpenSubsonic/Jellyfin evidence preserved; migration verifier and search/artwork/stream checks exercised locally | New dual-server migrated-data capture |
| H. Client acceptance | Client owner; after G | Existing MeloAmp/Android/Feishin evidence kept current; local-safe preparation complete | Physical clients, live credentials, production services |
| I. Backup/restore/rollback | Agent; after F | PostgreSQL restore, Tantivy rebuild, topology-bound rollback runbook, durable synthetic rollback audit | Production rollback rehearsal and activation |
| J. Cutover orchestration | Release operator; after I | Named roles, exact targets, impact/confirmation/topology checks, freeze/final-delta/cutover/rollback instructions | Write freeze, final delta, traffic switch |
| K. Packaging | Agent; independent after entry | Current amd64 Trixie/non-root image build; PostgreSQL smoke; media tools, CLI, Admin UI, readiness, graceful stop, volume restart; Syft SBOM and Trivy result | Critical findings, arm64 CI, provenance, publication |
| L. Observability | Agent; across A–K | Correlation, durable issues, progress, health/readiness distinction, search status, rollback audit, operator diagnostics | Production topology and deployment monitoring |
| Docs and closeout | Agent; last | Plan/report, roadmaps, matrix, overrides/index, runbook, public installation/backup/upgrade/release/troubleshooting docs | Operator signoff only |

## Approval gates

Separate approval is required for:

1. Production database/snapshot/replica/media access and private-data rehearsal.
2. Live provider, Google OIDC, registry, deployment, or hardware credentials.
3. C# write freeze, final production delta, and traffic switch.
4. Production rollback and post-rollback verification.
5. GitHub Release/GHCR publication and deletion of temporary or rollback assets.

The agent may build disposable fixtures, run local scans, create local images, validate workflow
semantics, and prepare commands. It must not execute a gated external action or claim it passed.

## Exit assessment

| Criterion | Assessment |
|---|---|
| Every active source domain/field has a disposition | Current local inventory and ADR-0148 ledger; operator data-owner review remains |
| Restartable, fenced, checkpointed, idempotent migration | Proven by migration tests and rehearsal |
| Public IDs and relationships preserved | Proven by importer, conflict, repeat/delta, and verifier tests |
| Supported credentials and secret re-sealing | Proven by migration/auth/secret tests; live OIDC remains open |
| Safe media adoption without mandatory full hashing | Proven by path/symlink/size and read-only repair tests |
| Inventory/import/verify/delta/freeze/cutover-ready | Proven on synthetic fixture; production execution gated |
| Backup/restore/search rebuild/rollback | Proven in isolated synthetic drill; production rollback gated |
| Differential and client acceptance | Existing accepted fixture/client evidence retained; migrated-production differential open |
| Official OCI artifact | Local amd64 build/smoke and SBOM pass; Trivy critical policy blocks, arm64/provenance/publication open |
| Documentation and traceability | Updated and repository-validated |
| Full Phase 11 exit | Complete with operator blockers; not a production release approval |

## Required closeout evidence

The report records source/target counts, mappings and issues, public IDs, credentials/secrets,
media observations, rehearsal timing/resource use, backup/restore/search/rollback, differential
and client boundaries, image/SBOM/vulnerability/provenance boundaries, approval actions,
requirements/ADR dispositions, exact validation commands, compatibility/data/security/operational
impact, remaining risks, final-audit readiness, and the required documentation-impact statement.
