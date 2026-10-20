# Release-Obligation Ledger (v1.0.0)

**Status:** Current as of 2026-10-19, repository main at `9189576`.
Maintained under MEL-18. This ledger maps every mandatory v1.0.0 release
obligation to its Hamstik work item, the owning evidence role, the required
artifact, and the acceptance criterion that closes it. It distinguishes:

- **Nonterminal requirement IDs** — the 15 `in-progress` rows in
  [`REQUIREMENTS-INDEX.md`](../../requirements/REQUIREMENTS-INDEX.md) (count
  enforced by `tests/python/test_requirements_index.py`). These are
  release-readiness/operator-acceptance obligations, not missing runtime
  workflows.
- **Capability/client backlog** — the `UNVERIFIED` capability rows and the
  five-client acceptance matrix in
  [`FEATURE-PARITY-MATRIX.md`](../../requirements/FEATURE-PARITY-MATRIX.md) and
  `compatibility/clients/r4/acceptance-matrix.json`. A requirement ID and a
  capability row may map to the same MEL item; they are not interchangeable
  completion metrics.

Owner roles follow MEL-1's ownership section. **No named owner is assigned
yet**; assignment is a pending MEL-1/MEL-12 operator input. Until named, each
row names the role that must own its evidence.

## Mapped obligations

| # | Requirement ID(s) | Capability row(s) | MEL item | Evidence owner role | Required artifact | Acceptance criterion |
|---|---|---|---|---|---|---|
| 1 | PRD-API-M1-004 | Public-ID preservation (matrix "Public identity" rows) | MEL-12 (evidence: MEL-15 report) | release owner | MEL-15 differential report JSON + golden captures for the exact release candidate SHA | Frozen legacy Artist/Release/Track public UUIDs round-trip verbatim on the candidate; zero unexplained differences |
| 2 | PRD-SCALE-001, PRD-SCALE-002 | Catalog scale/query-shape rows | MEL-16 | release owner + performance owner | Versioned benchmark report at representative topology, candidate SHA pinned | Documented PRD-SCALE budgets met (ingestion, search, catalog, provider workers); query shapes remain set-based/indexed |
| 3 | PRD-PERF-003, PRD-PERF-004 | Performance/fault rows | MEL-16 | release owner + performance owner | Same versioned report incl. fault matrix, resource caps, cancellation/restart convergence | Fault matrix and recovery budgets measured reproducibly; release owner approves |
| 4 | PRD-TEST-001, PRD-TEST-002, PRD-TEST-005, PRD-TEST-006, PRD-TEST-007 | Differential/capture rows | MEL-15 (done — report committed) → re-run for the final candidate under MEL-12 | compatibility owner | MEL-15 report JSON for the frozen candidate SHA | Zero unexplained differences; every intentional difference carries an accepted ADR |
| 5 | PRD-STORAGE-004 | Media preservation row (valid MP3 bytes retained) | MEL-8 (done — runbook + tests) | data owner | METADATA-TRANSFORMS runbook + normalization_real tests | Valid existing MP3 bytes retained without re-encoding; transforms atomic with revert evidence |
| 6 | PRD-MIG-007 | Migration/rollback rows | MEL-13 (done), MEL-14 (done) → production-shaped rehearsal per MEL-12 sequencing | migration operator + data owner | MEL-13 rehearsal report; MEL-14 cutover/rollback drill JSON | Authorized rehearsal completed on isolated data; rollback drill produces durable audit ID; production rehearsal still requires operator approval |
| 7 | PRD-API-JF-006 | Jellyfin + five-client matrix | MEL-17 | compatibility/client owner | `compatibility/clients/r4/acceptance-matrix.json` with passing rows per workflowSchema | All five client families pass the complete workflow matrix against the same migrated candidate |
| 8 | PRD-SCRIPT-002 | Scripting row | MEL-18 | integration owner | Current inventory record (docs/ADRs/parity matrix); classification decision | Inventory traceable and current; script-quarantine/deferred-classification contract closed |
| 9 | PRD-DEPLOY-001 | OCI artifacts row | MEL-12 | release owner | Exact-SHA amd64+arm64 images, smoke evidence, digests, SBOM/scan per workflow | Non-root, media-tool, CLI, Admin UI, graceful-stop smoke pass on both architectures; publication separately approved |
| 10 | — (release-owner acceptance) | ADR-0150 reference-only exclusions; device-profile reference-only row | MEL-12 (recorded under MEL-13/MEL-15/MEL-18) | release owner | Signed acceptance of the reference-only dispositions as final 1.0 exclusions, with production-snapshot affected counts and differential classifications attached | ADR-0150 follow-up satisfied; parity-matrix rows updated to final dispositions |
| 11 | — (configured-state acceptance) | Last.fm / providers / podcasts / OIDC / SMTP / MPD-MPV rows | MEL-9, MEL-10, MEL-11, MEL-19 | integration owner (credentials from data owner) | Redacted live acceptance reports per item | Link/unlink, rate limit, outage, delivery, playback legs exercised per each item's criteria |

## Rules recorded here

1. Withdrawn evidence reports and old candidate SHAs are never current
   acceptance artifacts (see the addendum in
   [`CURRENT-CSHARP-RECONCILIATION.md`](CURRENT-CSHARP-RECONCILIATION.md)).
2. Rehearsal evidence (MEL-13/14/16-rehearsal) is qualification *preparation*;
   the production-shaped rehearsal and target-scale acceptance remain separate
   operator-gated obligations.
3. Terminal statuses may only follow actual evidence; count assertions
   (`tests/python/test_requirements_index.py`) may only change when
   evidence/status genuinely changes.
