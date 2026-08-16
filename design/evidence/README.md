# Evidence Registry

Evidence makes performance, compatibility, migration, and phase-completion claims reproducible.
Commit sanitized summaries, schemas, commands, fixture manifests, and small golden outputs. Large or
sensitive raw captures stay outside Git and are referenced by immutable checksum/location.

Every evidence record states: date/time UTC, commit, legacy/Rust version, dataset identity/counts,
configuration, hardware/environment, exact command, result, limitations, and related requirement/ADR.

## Phase reports

- [Phase 0](phase-0/PHASE-0-REPORT.md)
- [Phase 1](phase-1/PHASE-1-REPORT.md)
- [Phase 2](phase-2/PHASE-2-REPORT.md) — complete; ADR-0145 corrective closure in R0
- [Phase 3](phase-3/PHASE-3-REPORT.md) — complete; ADR-0145 corrective closure in R0
- [Phase 4](phase-4/PHASE-4-REPORT.md)
- [Phase 5](phase-5/PHASE-5-REPORT.md) — complete; ADR-0145 corrective note recorded
- [Phase 6](phase-6/PHASE-6-REPORT.md)
- [Phase 7](phase-7/PHASE-7-REPORT.md) — Final, independently reviewed and accepted;
  [compatibility acceptance map](phase-7/COMPATIBILITY-ACCEPTANCE-MAP.md),
  [real-client acceptance](phase-7/REAL-CLIENT-ACCEPTANCE.md)
- [Phase 8](phase-8/PHASE-8-REPORT.md) — Final: 81/81 OpenSubsonic endpoints, 60/60 Jellyfin
  routes, zero-diff replay, Feishin real-client acceptance
- [Phase 9](phase-9/PHASE-9-REPORT.md) — Final: all twelve parity domains green or waived by
  accepted ADR
- [Phase 10](phase-10/PHASE-10-REPORT.md) — Final: Carbon admin SPA complete with all canonical
  screens evidenced
- [Release distribution](release-distribution/container-validation.md) — container/release-process
  evidence (ADR-0144)

## Release-closure reports (1.0.0)

- [R0 — Corrective baseline and durable staging closure](r0/README.md) — ADR-0145 acceptance:
  crash matrix, 200-Release drain acceptance, adversarial suite, four-role diagnostics
- [R1 — Requirement reconciliation and release truthfulness](r1/README.md) — 12 requirement
  closures, parity-matrix re-reconciliation, security baseline inventory, residual-risk
  disposition
