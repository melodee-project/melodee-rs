# R5 — Release-candidate hardening and complete gate

Status: Formal R5 entry is not started because R4 remains the active closure phase. Local
preparatory correctness, repository, UI, dependency, migration, container-configuration, and
documentation-adjacent checks pass from the current working tree. External scheduled-run, scanner,
physical-client, production-provider, and release-owner gates remain open.

Date: 2026-08-20

## Scope and authority

R5 is the release-candidate hardening portion of the release-closure plan. Its authority is the
PRD Release Gates A–I, the repository Definition of Done, ADR-0144, and the R5 work packages in
`IMPLMENTATION_OUTSTANDING.md` §10. R4 remains the active phase because its production-shaped snapshot,
physical-client, provider, traffic-switch, and rollback acceptance legs are environment-owned.

No version, tag, image, release, or public compatibility contract was changed. R6 therefore
remains not started.

## Local gate evidence

The following checks passed on 2026-08-19 from HEAD `2877e2c` plus the documented
uncommitted closure evidence:

```text
./scripts/check-smoke.sh
./scripts/check.sh
cargo test -p melodee-cli --test legacy --test legacy_freeze
cargo clippy -p melodee-cli --all-targets --all-features -- -D warnings
cargo deny check
python3 -m py_compile scripts/legacy_rehearsal.py scripts/run_r4_dual_run.py
python3 scripts/validate_repository.py
python3 scripts/extract_requirements.py --check
git diff --check
cargo fmt --all --check
docker compose -f compose.dev.yaml config
docker compose config
./scripts/check-smoke.sh
./scripts/check.sh
SMOKE_IMAGE=melodee:r5-iterative-local RELEASE_VERSION=0.1.0 \
  RELEASE_REVISION=2877e2c-dirty SMOKE_RUN_ID=local-runtime-validator-20260820 \
  SMOKE_PORT=18102 ./scripts/container_smoke.sh
cd docs && pnpm install --frozen-lockfile && pnpm check && pnpm check:links && pnpm build
```

The focused migration/freeze suite passed 15/15 tests. The full gate passed repository and
requirements validation, 174 Python tests, Admin UI typecheck/lint/i18n/unit/build (248 unit
tests), workspace formatting, Clippy, the complete workspace test suite including PostgreSQL
integration, and dependency checks. The Admin UI lint reported two existing non-fatal Fast Refresh
warnings; it reported zero errors. `cargo deny check` reported advisories, bans, licenses, and
sources as OK; duplicate transitive packages and an unmatched optional license allowance remain
warnings already accepted by the repository gate.

The Compose checks used synthetic values from `compose.env.example`; they did not contact a
deployment or create persistent volumes. The local smoke and full gates were rerun while the
proposed runtime-base image was under triage; ADR-0151 remains Proposed and no runtime-base
acceptance or publication is claimed. The source Dockerfile retains the accepted Bookworm
runtime until that decision is accepted. The docs site built 42 pages with link validation passing.

The latest current-tree `./scripts/check.sh` rerun after the operator-help and evidence updates
also passed with exit code 0: 181 Python tests, Admin UI 44 files/248 tests, all workspace and
PostgreSQL tests, formatting, Clippy, and cargo-deny. The same non-failing Fast Refresh and
duplicate/unmatched-license warnings remain recorded above. This rerun does not provide the
external R4 acceptance, candidate-SHA, scheduled, arm64, production, physical-client, ADR, or
publication evidence required for release closure.

The latest current-tree `./scripts/check.sh` rerun after CLI search-test environment isolation
also passed with exit code 0: 194 Python tests, Admin UI 44 files/248 tests, all workspace and
PostgreSQL Rust tests and doctests, formatting, Clippy, and cargo-deny. The focused search-root
safety test passed ten consecutive times, and the 12-test legacy migration suite passed,
including the shared-track multi-user state regression. This remains dirty-tree local evidence
only; it does not provide candidate-SHA, scheduled, arm64, production, physical-client, ADR,
or publication evidence.

The latest current-tree rerun after the R4.1 source-backup validation also passed with exit code
0: 184 Python tests, Admin UI 44 files/248 tests, all workspace and PostgreSQL tests, formatting,
Clippy, and cargo-deny. The docs checks and build also passed with 42 pages. This remains local
evidence only and does not provide the external R4 acceptance, candidate-SHA, scheduled, arm64,
production, physical-client, ADR, or publication evidence required for release closure.

The latest current-tree `./scripts/check.sh` rerun after the reproducible operator-command help
audit passed with exit code 0 on 2026-08-20. It ran 197 Python tests, the Admin UI gate with 44
files and 248 unit tests, all workspace/PostgreSQL Rust tests and doctests, formatting, workspace
Clippy with warnings denied, and cargo-deny. A subsequent separate command-help audit passed 78
CLI paths plus `legacy_rehearsal.py drill --help`. This remains dirty-tree local evidence only and does not
provide candidate-SHA, scheduled, arm64, production, physical-client, ADR, or publication
evidence.

The most recent current-tree rerun after release-evidence bundle path-containment hardening also
passed with exit code 0: 186 Python tests, Admin UI 44 files/248 tests, all workspace and
PostgreSQL tests, formatting, Clippy, and cargo-deny. The new focused tests reject symlinked
evidence roots and required files before they are read or hashed. This remains local evidence
only and does not provide the external R4 acceptance, candidate-SHA, scheduled, arm64,
production, physical-client, ADR, or publication evidence required for release closure.

The latest current-tree rerun after migration lease-fencing hardening also passed with exit code
0: 186 Python tests in 110.388 seconds, Admin UI 44 files/248 tests, all workspace and PostgreSQL
tests, formatting, Clippy, and cargo-deny. The complete migration/fault/freeze suite passed 22/22
after final projection, excluded-summary, and verification-issue writes were fenced. This remains
local dirty-tree evidence only and does not provide the external R4 acceptance, candidate-SHA,
scheduled, arm64, production, physical-client, ADR, or publication evidence required for release
closure.

The Compose configuration was also validated with the required variables supplied as synthetic
local values:

```text
docker compose -f compose.dev.yaml config
docker compose config
```

The production-shaped R4 rehearsal remains the source for migration/rollback measurements:
`design/evidence/r4/rehearsal-drill-12000.json` and the surface-scoped reports
`design/evidence/r4/dual-run-report-native.json`,
`design/evidence/r4/dual-run-report-opensubsonic.json`, and
`design/evidence/r4/dual-run-report-jellyfin.json`.

## R5.1 — Data-loss and migration safety review

The local review confirms that the implemented migration is source-read-only, preserves legacy
public IDs, adopts canonical media without re-encoding or relocation, retains uncertain/name-only
observations rather than inventing Artist identities, and preserves weak share links only through
explicit secure-token rotation. Existing migration fault tests cover stale leases, source writes,
orphan references, malformed media metadata, public-ID conflicts, repeatability, and empty-source
imports. The rehearsal rollback report proves the failed target is retained while the source and
canonical media remain unchanged.

The destructive-operation inventory was checked individually:

| Operation | Local preconditions and recovery evidence |
|---|---|
| Inbound acquisition cleanup | Direct-child containment, `symlink_metadata` no-follow replacement fencing, source generation and tree-hash fences, durable receipt, current lease, directory sync, and delayed path-reuse/symlink tests in `acquisition.rs` |
| Partial staging cleanup | Worker-owned direct child of the staging `releases/` root; `symlink_metadata` rejects a replaced symlink or non-directory before recursive removal; unexpected paths fail closed |
| Canonical publication/adoption | Validated roots, symlink/special-file rejection, no-clobber finalization, streamed equality/hash checks, and retry/fencing tests; source is retained |
| Search generation retirement | No-follow root and descriptor-relative unlink/rename on supported Unix platforms; unsupported platforms fail closed; symlink replacement tests pass |
| Podcast retention | Retention policy excludes bookmarked episodes, requires the exact canonical `{public_id}.media` child, locks and rechecks the row, and rejects symlinks/non-regular files |
| Onboarding probes | Removes only the uniquely created probe file |
| Legacy migration | Source database/media are opened read-only; canonical adoption validates containment/size and performs no move, re-encode, hash-required rewrite, or delete |

No constructed or user-controlled path reaches a shell. The remaining production acceptance item is
operator review against the actual approximately 5 TB topology and restore target; the local
review does not authorize destructive action against that library.

This is not a substitute for an operator review of the real approximately 5 TB source and restore
environment. No local test authorizes destructive operations against a production library.

## R5.2 — Security and FOSS review

The workspace adversarial suites and `cargo deny check` pass. Existing tests cover traversal,
symlinks, path reuse, delayed cleanup, uploads/artwork, configuration redaction, provider and
sidecar inputs, auth/authorization, cookie/session behavior, share access, Admin API gates, and
diagnostic secrecy. No production dependency was added for this closure work.

The local review also terminalized `PRD-SEC-001`, `PRD-SEC-002`, and `PRD-SEC-003` with direct
implementation and test evidence in the generated requirements registry. It covered every
production filesystem removal/adoption path, uncertainty-to-review/quarantine behavior, and
artwork/avatar/playlist/theme/config/script upload boundary. The review does not authorize
production cutover or replace the external operator review of the real media root.

The browser-facing Admin API and SPA now receive a scoped security policy from
`apps/melodee-server/src/security_headers.rs`: same-origin CSP, `nosniff`, frame denial,
no-referrer, and restrictive permissions. The middleware intentionally excludes native v1,
OpenSubsonic, Jellyfin, media, and health paths so captured compatibility headers are unchanged.
Unit tests cover both the path boundary and policy values. HSTS remains a TLS-termination concern
and is documented for the reverse proxy rather than asserted on the server's HTTP listener.

Repository security automation is now present for the next clean checkout: `.github/workflows/security.yml`
runs Gitleaks 8.30.0 against repository contents, `.github/workflows/codeql.yml` runs CodeQL for
Rust, TypeScript/JavaScript, and Python, `.github/dependabot.yml` schedules Cargo, both pnpm
workspaces, and GitHub Actions updates, and the docs workflow runs `pnpm audit --prod`. The
Gitleaks configuration allowlists only exact synthetic fixture paths. A clean archive of the
current baseline plus that configuration passed the local Gitleaks scan with zero findings.

The docs audit initially identified `nanoid@3.3.17` through the Astro/PostCSS toolchain. The
patched `3.3.18` release is now enforced through `docs/pnpm-workspace.yaml`, the lockfile was
regenerated, and `cd docs && pnpm audit --prod` reports no known vulnerabilities.

The exact local container scans are recorded below; they are not accepted-candidate evidence and
do not accept ADR-0151. The Trixie prototype was built and smoke-tested before the source
Dockerfile was restored to the accepted Bookworm baseline:

```text
docker build --tag melodee:r5-iterative-local \
  --build-arg VERSION=0.1.0 \
  --build-arg REVISION=2877e2c-dirty .
```

The historical Trixie image content ID is
`sha256:1a8551faf7b973aa78f6cbdcef3be7364eae07c98935a971ef977359e9eea552`.
The Trivy artifact digest is
`sha256:f270191810618e5ebbdf315503358e75a5b02ed98849ad8ab72ae416f26ea547`.
It used Debian Trixie 13.6, FFmpeg `7.1.5-0+deb13u1`, and Chromaprint `1.5.1-7`.
The extracted release-workflow smoke script passed locally with disposable containers: UID/GID 10001, CLI version, FFmpeg,
FFprobe, fpcalc, required OCI labels, image ID, PostgreSQL-backed health/readiness, Admin SPA, and
graceful SIGTERM. Its Bash syntax, input validation, label checks, and image-ID reporting are covered by
`tests/python/test_container_smoke.py`. Trivy 0.72.0 scanned this exact local image and reported 8 critical,
187 high, 203 medium, 183 low, and 87 unknown findings. The exact arm64 image was scanned with the
same tool and database and reported the same severity counts. Both still fail the release workflow's
critical-finding policy and are not release evidence. The unresolved findings require
package/vendor triage and release-owner acceptance; no suppression was added.

Syft 1.51.0 generated SPDX 2.3 SBOMs containing 299 amd64 packages and 296 arm64 packages.
The full reports, SBOMs, and SHA256SUMS are committed under
`design/evidence/r5/artifacts/local-20260820/`; they have no release provenance or signed
attestation.

The earlier `melodee:r5-trixie-local` prototype remains historical triage evidence with content
ID `sha256:7236faeda9024f7adef47736d16fbaf5c59976c6ca96e0dabbedbb00b6f710d5`; it is not the
current-tree image.

ADR-0151 proposes the Trixie runtime-base change because the current Bookworm image had 11
critical and 185 high findings. The proposal remains unaccepted until the candidate scan,
media/client compatibility, both architecture builds, SBOM, and provenance requirements pass.
The historical exact-image scan and the alternatives requiring release-owner disposition are
recorded in [`adr-0151-decision-package.md`](adr-0151-decision-package.md); this package does not
accept the ADR or authorize publication. A new candidate image must be built only after the
release owner accepts or revises ADR-0151.

The individually evaluated PRD Release Gates A–I are recorded in
[`release-gate-report.md`](release-gate-report.md). The report is intentionally Partial: it lists
direct local evidence and each missing external condition rather than treating aggregate local
green checks as release approval.

Containerized tooling was available for supplemental inspection, but it does not replace the
release workflow:

- Trivy 0.72.0 filesystem scanning found no supported language files and no secrets, so it is not
  a meaningful candidate result. Trivy image scanning of the older local image
  `melodee:release-distribution-local` (revision `55fc3eb`, not this working tree) found 773 Debian
  findings: 11 critical, 185 high, 296 medium, 231 low, and 50 unknown. This historical image is
  not a release candidate; its findings remain useful only as the baseline for the current
  prototype comparison.
- Syft generated a package SBOM for that older local image. A local prototype image now also
  exists for security triage, but no accepted release-candidate digest or release artifact exists
  to attach a final SBOM/provenance bundle to.
- Gitleaks 8.30.0 passed a clean repository archive with the exact synthetic-fixture allowlist.
  The earlier dirty-worktree scan reported the ignored local `.env`, generated `target/`, and
  fixture values; that scan was intentionally not used as release evidence.

The accepted release workflow must scan the exact candidate image, classify every finding, and
produce the final SBOM/provenance artifacts.

An earlier arm64 `docker buildx build --platform linux/arm64 --load` attempt was
run with a 1,800-second timeout and was canceled before image load. The later
cached BuildKit retry produced the image recorded in the current package report
below; native arm64 builder performance is still not established.

## R5.3–R5.5 — Correctness, flake, and requirement closure

An earlier full-gate attempt exposed a real timing flake in
`apps/melodee-server/tests/discovery_worker.rs`: after the scheduler repaired the durable
projection, a second no-op pass could replace the returned latest-pass statistics before stop.
The test now asserts the observed durable repair and only requires a completed pass from the
stopped worker. The exact test passed 10/10 consecutive runs, and the corrected full local gate
completed without failures, including the long-running 200-release,
restart/recovery, production-shaped catalog-query, media normalization, publication, search,
native, OpenSubsonic, Jellyfin, and Admin API suites. No correctness test was changed to ignored
or bypassed as part of this work. The local 12,000-Track R4 rehearsal now has two clean runs with
zero verification gaps, zero duplicate public IDs, matching catalog counts, successful restore,
freeze/delta, rollback retention, and unchanged canonical media; this is still synthetic
production-shaped evidence, not the required real-library rehearsal. The current native
differential replay is documented in the R4 report with 267 classified and 181 unclassified
differences; the replay closed the captured native search-suggest, login-version, cookie-cache,
missing-chart-message, missing-theme-asset-envelope, empty-upload-validation, heartbeat-message,
unknown-stream-user-message, and native-image-cache-header mismatches, while the remaining
unclassified set still requires external compatibility classification or implementation fixes and
is not silently waived. The current OpenSubsonic replay records 330 differences after the
adapter's canonical-path projection correction and native-only fixture isolation; 49 path
differences were removed by the tested fix and three cross-surface seed residuals were removed by
the harness correction. The current Jellyfin replay records 149 classified and 17 unclassified
differences after the tested migrated-container and primary-release-count
corrections. Proposed ADR-0150 records the remaining legacy media metadata
boundary.

The generated requirement registry remains truthful rather than being forced terminal: the
remaining statuses cover external migration scale, physical-client, final oracle/differential,
security-scan, and performance evidence. Those rows need their assigned external evidence or an
accepted exclusion before R5 can exit.

Scheduled/nightly workflow URLs, production credentials, physical Android Auto/DHU and MeloAmp
acceptance, and real provider/storage topology are not available in this local workspace. They
remain release blockers.

## R5.6 — Release artifacts

No release candidate was built or published. The workspace remains pre-1.0; no OCI tag, GitHub
Release, image digest, SBOM, provenance attestation, or checksum is claimed. Release execution
requires the release-owner authorization specified by the plan and ADR-0144. The release workflow
now has separate pre-publication amd64 and arm64 build, smoke, and Trivy matrix legs; the publish
job waits for the complete matrix. Each successful matrix leg now captures image identity and
architecture, an SPDX SBOM, JSON Trivy reports, scan status, and a SHA256SUMS manifest as a
short-retention workflow artifact. This workflow change is statically checked, but it has not run
in GitHub Actions for a release candidate and does not create an artifact locally.

### R4.4 aggregate invariant regression and rebuilt replay — 2026-08-20

- Exact files changed: `apps/melodee-cli/src/legacy/import.rs`,
  `crates/melodee-persistence/src/catalog.rs`,
  `crates/melodee-persistence/src/catalog_mutations.rs`,
  `apps/melodee-server/tests/jellyfin_media_api.rs`, the three surface-scoped
  differential reports, this report, the R4 report, the R5 release-gate report,
  and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: a proposed primary-only rewrite of `artist_catalog_aggregates.release_count`
  failed the existing catalog detail regression because this aggregate is a participation
  statistic that includes credited releases. The original semantics were restored across legacy
  import, normal catalog import, and full maintenance rebuild. The separate Jellyfin regression
  forces maintenance rebuild before checking `ChildCount` and passes against its direct
  primary-billing query. R4.4 remains Partial and R5 remains formally unopened.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed. No
  disputed metadata mapping or comparator waiver was added.
- Tests: the initial full workspace gate caught the catalog detail regression; after restoration,
  the focused catalog browse suite passed 7/7, focused Jellyfin regression passed 1/1, isolated
  rebuilt differential runs exited 1 fail-closed with native 185, OpenSubsonic 330, and Jellyfin
  17 unclassified differences, and the corrected full local gate passed with 216 Python tests,
  Admin UI 44 files/248 unit tests, the complete workspace/all-features PostgreSQL Rust and
  doctest matrix, formatting, Clippy, and cargo-deny.
- Compatibility impact: no rejected aggregate behavior was retained; the three compatibility
  products retain separate DTOs and unchanged routes, identifiers, statuses, auth, pagination,
  range, and intentional 410/501 behavior.
- Data/migration impact: only disposable isolated replay databases/media roots were used; counts
  derive from migrated PostgreSQL authority without changing source data or canonical media.
- Security/data-loss impact: no new write path or metadata synthesis; the rejected aggregate
  change did not mutate source/media, and fail-closed differential behavior remains in place.
- Performance/operational impact: aggregate maintenance retains its existing participation-count
  semantics; production-sized measurements are still missing.
- Documentation impact: Updated — this report, the R4 report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed.
- Remaining risks and owner: release/compatibility owner owns the remaining differential
  residuals and ADR decisions; production-sized, physical-client, scheduled, and production
  integration evidence remain outstanding.
- External input still required: ADR-0149/0150/0151 dispositions, approved production snapshot
  and topology, and authorization for external validation.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 native share-update policy compatibility correction — 2026-08-20

- Exact files changed: `apps/melodee-server/src/native/community.rs`,
  `apps/melodee-server/tests/native_community_api.rs`, `docs/src/content/docs/features/sharing.md`,
  the native differential report, this report, the R4 report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: native-v1 share updates continue to accept the legacy `isDownloadable` property
  but preserve the captured share policy. The focused replay removed one unclassified native
  field; no requirement became terminal and R4.4/R5 remain Partial.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed; the
  separate OpenSubsonic/Jellyfin playlist metadata conflict remains a decision item.
- Tests: the focused native community integration test passed 1/1; `cargo fmt --all` and server
  Clippy with `-D warnings` passed; the native replay completed all 12 groups and exited 1
  fail-closed with 181/330/17 residuals, with no unexpected share-update difference; local
  repository/docs/full-gate evidence remains separate and does not make the release gates pass.
- Compatibility impact: native v1 only; OpenSubsonic and Jellyfin routes and DTOs remain separate
  and unchanged.
- Data/migration impact: no schema, migration mapping, source, target, media, or identity change.
- Security/data-loss impact: the update no longer mutates durable share policy from this legacy
  compatibility field; no destructive behavior was introduced.
- Performance/operational impact: negligible and bounded; no dependency or service change.
- Documentation impact: Updated — public sharing documentation and internal R4/R5 evidence files.
- Remaining risks and owner: release/compatibility owner owns the remaining 181/330/17 residuals,
  ADR dispositions, production-sized rehearsal, physical clients, and integration gates.
- External input still required: approved snapshots/topology, ADR dispositions, client/provider
  access, scheduled candidate evidence, and release-owner authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 native playlist comment compatibility correction — 2026-08-20

- Exact files changed: `apps/melodee-server/src/native/playlists.rs`,
  `apps/melodee-server/tests/native_playlists.rs`, `docs/src/content/docs/api/melodee-api.md`,
  the native differential report, the R4 report, the ADR-0150 decision package, this report,
  the R5 release-gate report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: native-v1 playlist create/update now match the frozen behavior by accepting wire
  `comment` while preserving an empty returned `description`; three native unclassified fields
  were removed. R4.4 remains Partial at 182/330/17; no requirement became terminal.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed. The
  OpenSubsonic/Jellyfin playlist metadata conflict remains an explicit decision item.
- Tests: focused native playlist CRUD passed 1/1; all 12 native replay groups completed and the
  replay exited 1 fail-closed with 182 native, 330 OpenSubsonic, and 17 Jellyfin residuals;
  repository validation, requirements validation, `git diff --check`, and `./scripts/check-smoke.sh`
  passed; the docs check/link/build passed with 42 pages; and the exact current-tree
  `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` passed with 217 Python tests,
  Admin UI 44 files/248 unit tests, the full workspace/PostgreSQL Rust and doctest matrix,
  formatting, Clippy `-D warnings`, and cargo-deny.
- Compatibility impact: native-v1 response compatibility was corrected; OpenSubsonic/Jellyfin
  contracts remain unchanged and separate.
- Data/migration impact: no schema, source, target, media, or identity changes.
- Security/data-loss impact: no new destructive behavior; the shared description field is not
  mutated from this native compatibility-only input.
- Performance/operational impact: negligible; no new dependency or service.
- Documentation impact: Updated — public API documentation and the internal R4/R5 evidence files.
- Remaining risks and owner: release/compatibility owner owns the 182/330/17 differential
  residuals and all external R4/R5 evidence.
- External input still required: ADR dispositions, approved production snapshot/topology,
  physical clients, integration endpoints, scheduled candidate run, and release authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6 OIDC JWKS host-bound probe — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, the R4 report, this report, the R5 release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the bounded integration probe now permits a discovery-host JWKS endpoint by
  default and requires explicit repeated `--oidc-allowed-host HOST` arguments before following a
  cross-host JWKS URI from untrusted discovery input. No requirement became terminal; R4.6 remains
  Partial pending production integration evidence.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the focused probe and command-help suites passed 16/16; Python compilation and the
  probe's `--help` path passed. The subsequent `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh`
  exited 0 with 217 Python tests, Admin UI 44 files/248 unit tests, the complete workspace/all-
  features PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with `-D warnings`,
  and cargo-deny.
- Compatibility impact: none; all three compatibility products and their wire contracts are
  unchanged and remain separate.
- Data/migration impact: none; no database, migration source, or canonical media was accessed.
- Security/data-loss impact: positive fail-closed SSRF containment for the operator probe; no
  secret or key material is emitted.
- Performance/operational impact: one bounded in-memory host comparison; Google-style providers
  require their documented JWKS host to be supplied explicitly.
- Documentation impact: Updated — this R5 report, the R4 report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; public documentation unchanged.
- Remaining risks and owner: release owner/operator still owns approved production OIDC, Last.fm,
  SMTP, podcast, proxy/TLS, storage, and restore validation.
- External input still required: approved endpoints and non-sensitive test credentials through a
  secure operator channel; no external endpoint was contacted by this change.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

## Remaining R5/R6 acceptance

- Repeat the R4 rehearsal against the real production-shaped snapshot and restore environment.
- Complete the external differential classification and physical native/Android Auto/MeloAmp/
  OpenSubsonic/Jellyfin client matrix.
- Run scheduled/nightly checks from the candidate SHA.
- Run Trivy, Gitleaks, Syft, image smoke, provenance, and vulnerability gates.
- Replace the local dirty-tree architecture scans with exact candidate-SHA scans,
  signed provenance, and a release-owner severity-policy disposition.
- Complete production OIDC, Last.fm, SMTP, podcast, proxy/TLS, storage, backup/restore, and
  operator cutover/rollback evidence.
- Obtain release-owner acceptance before R6 versioning, tagging, image publication, or GitHub
  release execution.

Documentation impact:

- Updated — this internal R5 evidence report, `design/evidence/r4/README.md`,
  `IMPLMENTATION_OUTSTANDING.md`, and the public migration guide
  `docs/src/content/docs/installation/legacy-migration.md`.

### R5.4 local flake follow-up package report — 2026-08-19

- Exact files changed: `apps/melodee-server/tests/discovery_worker.rs`,
  `design/evidence/r5/README.md`, `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: local R5.4 timing/order-dependence coverage is corrected; scheduled/nightly
  candidate-SHA workflow evidence remains absent, so no requirement was marked terminal.
- ADRs: no new ADR was applied; the accepted discovery-worker authority remains in force and
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the first post-hardening full gate exposed the scheduler-statistics flake; the focused
  scheduler test then passed 10/10 consecutive runs; the final `./scripts/check.sh` passed with
  174 Python tests, Admin UI 44 files/248 tests, workspace Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin contract changed.
- Data/migration impact: none; the test-only synchronization assertion does not alter production
  worker behavior or migration data.
- Security/data-loss impact: no waiver or test suppression was added; the correction prevents a
  no-op follow-up pass from being misreported as a failed repair while retaining the durable-state
  assertion.
- Performance/operational impact: no production polling interval or worker implementation changed;
  the test remains bounded and validates graceful stop after a real scheduled pass.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: scheduled/nightly candidate-SHA URLs and repeated clean-run evidence
  remain release-owner/CI-owned; production topology and external R4 gates remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.5 workflow-complete client evidence schema — 2026-08-20

- Exact files changed: `scripts/validate_r4_client_acceptance.py`,
  `tests/python/test_r4_client_acceptance.py`,
  `compatibility/clients/r4/acceptance-matrix.json`,
  `compatibility/clients/r4/README.md`, the R4 report, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the current-client artifact now requires an explicit result for every workflow
  named by its row, preventing a partial checklist from being reported as a full physical-client
  pass. The artifact remains `pending-external`; R4.5 and R5.3/R5.5 remain non-terminal.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the focused validator suite passed 8/8, Python compilation passed, and the committed
  schema-v3 matrix validated with 0 pass, 0 fail, and 5 pending-external rows. The subsequent
  exact local `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 216
  Python tests, Admin UI 44 files/248 unit tests, the complete workspace/PostgreSQL Rust and
  doctest matrix, formatting, workspace Clippy with `-D warnings`, and cargo-deny.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin wire contract changed.
- Data/migration impact: none; no database, source snapshot, canonical media, or client endpoint
  was contacted.
- Security/data-loss impact: positive fail-closed evidence validation; missing workflow results,
  failed workflows without notes, secret-shaped values, and non-repository evidence paths remain
  rejected.
- Performance/operational impact: bounded local JSON validation only.
- Documentation impact: Updated — the R4 report, this report, the R4 client README, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because runtime behavior is
  unchanged.
- Remaining risks and owner: release owner/operator still owns current migrated physical-client
  execution and sanitized workflow evidence.
- External input still required: current client binaries/devices, disposable migrated target, and
  operator participation for all five matrix rows.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.3/R5.1 migrated-catalog search probe non-vacuity — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, this report, the R4 report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: existing-snapshot search probes now derive representatives from migrated
  PostgreSQL Artist authority, require all six search classes to return authoritative values, and
  redact snapshot-derived catalog values from machine-readable evidence. No requirement became
  terminal; R4.3 and R5.1 remain Partial.
- ADRs: accepted ADR-0148 remains applicable; no ADR was accepted or applied. ADR-0149,
  ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v` passed 21/21;
  `python3 -m py_compile scripts/legacy_rehearsal.py` passed; the isolated five-song rehearsal
  passed migration verification, search rebuild, all six query classes, bounded autocomplete,
  backup/restore comparison, freeze, and rollback. No production snapshot was accessed.
- Compatibility impact: none; all three wire products and their public contracts remain unchanged
  and separate.
- Data/migration impact: synthetic disposable databases only; existing-snapshot mode performs a
  read-only authority query after import and does not modify source or canonical media.
- Security/data-loss impact: positive fail-closed validation; missing Unicode, punctuation, or
  low-selectivity representatives stop the rehearsal instead of producing a false green result,
  and snapshot evidence does not retain catalog text.
- Performance/operational impact: one bounded authority query plus the existing bounded search
  measurements; no runtime server path changed.
- Documentation impact: Updated — this report, the R4 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed.
- Remaining risks and owner: production-sized search sizing, headroom, and operator diagnostics
  still require the release owner/operator and an approved isolated snapshot.
- External input still required: approved snapshot URL, backup, canonical mapping, and rehearsal
  authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.3 search diagnostic and query-matrix rehearsal — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, the R4 report, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: local R4.3/R5.3 evidence now validates the activated search generation against
  migrated counts and schema, bounded prefix autocomplete, six query classes, result limits, index
  size, and latency percentiles. R4.3 remains Partial; no requirement was marked terminal.
- ADRs: accepted ADR-0148 remains applicable; no ADR was accepted or applied. ADR-0149, ADR-0150,
  and ADR-0151 remain Proposed.
- Tests: focused rehearsal tests passed 19/19. The five-song isolated drill passed against the
  rebuilt target with an activated 50/100/5 generation, schema 3, 135,246-byte index, bounded
  prefix results, and six query classes. The subsequent exact local
  `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 209 Python tests,
  Admin UI 44 files/248 unit tests, the complete workspace/PostgreSQL Rust and doctest matrix,
  formatting, workspace Clippy with `-D warnings`, and cargo-deny. This remains synthetic local
  evidence only.
- Compatibility impact: none; all three compatibility products retain their separate wire
  contracts and no client behavior changed.
- Data/migration impact: disposable fixture-only Unicode/punctuation search values; no production
  source, target, backup, or canonical media was accessed.
- Security/data-loss impact: positive fail-closed diagnostic and query-limit checks; no destructive
  migration behavior changed.
- Performance/operational impact: bounded diagnostic, autocomplete, query, index-size, and timing
  measurements; no production workload changed.
- Documentation impact: Updated — the R4 report, this report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal tooling.
- Remaining risks and owner: release owner/operator must provide the production-sized migrated
  target, search headroom measurements, and operator-triggered rebuild/status/retry evidence.
- External input still required: approved isolated production snapshot/canonical mapping and
  explicit authorization for production-sized rehearsal operations.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.1/R5.1 baseline and path-map fail-closed validation — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, the R4 report, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: malformed `--library-path-map` values now fail before target creation using rules
  aligned with the Rust parser. The rehearsal validates the stable CLI baseline artifact's frozen
  52-table coverage, identity/schema linkage, v2 marker, read-only facts, media totals, and
  redacted configuration fingerprint. R4.1/R5.1 remain Partial and no terminal requirement
  status was inferred.
- ADRs: accepted ADR-0148 remains applicable; no ADR was accepted or applied. ADR-0149,
  ADR-0150, and ADR-0151 remain Proposed.
- Tests: focused rehearsal tests passed 18/18. The five-track isolated drill passed against the
  actual CLI baseline JSON, including zero verification mismatch gaps, full restored catalog/media
  equality, freeze, failed-target retention, and canonical-media immutability. This is synthetic
  local evidence only. The subsequent exact local `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh`
  exited 0 with 208 Python tests, Admin UI 44 files/248 unit tests, the complete
  workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with `-D warnings`,
  and cargo-deny.
- Compatibility impact: none; all three compatibility products retain their existing contracts.
- Data/migration impact: malformed mappings fail before any rehearsal target is created; no
  production source, target, backup, or canonical media was accessed.
- Security/data-loss impact: positive fail-closed correction for partial-target creation and false
  or improperly redacted baseline evidence.
- Performance/operational impact: bounded preflight string/JSON checks; no production migration
  algorithm changed.
- Documentation impact: Updated — the R4 report, this report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal tooling.
- Remaining risks and owner: release owner/operator must supply the approved production snapshot,
  backup, canonical mapping, topology, restore target, and authorization.
- External input still required: approved isolated production snapshot/backup/canonical mapping and
  explicit authorization for production-equivalent rehearsal operations.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.8/R5.1 synthetic restore-content and fixture closure — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, the R4 report, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the synthetic rehearsal now validates its generated backup before restore and
  compares restored Artist, Release, Track, and user counts plus media file count and byte totals
  against the target. Bounded synthetic playlist and user-state seeding now guarantees unique
  source identity pairs, preventing fixture duplicates from appearing as migration loss.
  R4.8/R5.1 remain Partial; no terminal status was inferred from synthetic evidence.
- ADRs: accepted ADR-0148 remains the migration tooling authority; no ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v` passed 16/16. The isolated
  `python3 scripts/legacy_rehearsal.py drill --songs 5` passed end to end with zero verification
  mismatch gaps, full restored catalog/media equality, backup validation, freeze, failed-target
  retention, and canonical-media immutability. The post-change exact
  `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 208 Python tests,
  Admin UI 44 files/248 unit tests, the complete workspace/PostgreSQL Rust and doctest matrix,
  formatting, workspace Clippy with `-D warnings`, and cargo-deny.
- Compatibility impact: none; all three compatibility products retain their separate existing
  wire contracts and no client behavior changed.
- Data/migration impact: only disposable local rehearsal databases were used; no production source,
  target, backup, or canonical media was accessed. The production snapshot and restore-target
  evidence remain outstanding.
- Security/data-loss impact: positive; restore verification covers complete measured content shape,
  while unique synthetic identity pairs prevent false source/target mismatch reports.
- Performance/operational impact: bounded aggregate restore checks and deterministic fixture
  generation; no production migration path changed.
- Documentation impact: Updated — the R4 report, this report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal tooling.
- Remaining risks and owner: release owner/operator must run the approved real snapshot, backup,
  topology, restore, traffic-switch, and rollback rehearsals.
- External input still required: isolated production snapshot/backup/canonical mapping and explicit
  authorization for production-equivalent restore/cutover operations.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 podcast publication root descriptor reuse — 2026-08-20

- Exact files changed: `apps/melodee-server/src/podcast_worker.rs`, this report, the R5
  release-gate report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: podcast temporary-file creation, no-replace publication, and owned-file cleanup
  now reuse the component-by-component `O_DIRECTORY|O_NOFOLLOW` root opener. Opening only the
  final root component with `O_NOFOLLOW` was insufficient because an ancestor could still be
  replaced; publication and cleanup leaves are now relative to the held descriptor. No
  requirement was marked terminal; R5.1 remains Partial pending real topology and restore-target
  review.
- ADRs: accepted ADR-0134 remains applicable; no ADR was accepted or applied. ADR-0149,
  ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; the focused podcast-worker subset passed 5/5, including the
  new ancestor-symlink regression; and the exact current-tree
  `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` passed with 204 Python tests,
  248 Admin UI unit tests, the complete workspace/PostgreSQL Rust and doctest matrix,
  formatting, workspace Clippy with `-D warnings`, and cargo-deny. No test was weakened or
  ignored for this change.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, range behavior, and intentional 410/501 behavior are
  unchanged.
- Data/migration impact: no database, migration source, canonical media, podcast content, or
  production target was modified; the change only narrows the existing owned-root boundary.
- Security/data-loss impact: positive fail-closed correction; ancestor replacement cannot redirect
  podcast publication, no-replace rename, or orphan cleanup into another tree on supported Unix
  targets. The regression proves the external target remains untouched.
- Performance/operational impact: each affected operation performs one bounded descriptor walk
  and then a descriptor-relative leaf operation; download limits, leases, polling, and API
  behavior are unchanged.
- Documentation impact: Updated — this internal R5 report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; public documentation unchanged because this is an internal
  safety-boundary correction with no user-visible contract change.
- Remaining risks and owner: the broader R5.1 review still requires the real approximately-5-TB
  production topology and restore-target validation; release owner/operator owns that evidence.
- External input still required: approved isolated production snapshot/backup/canonical mapping
  and explicit authorization for production migration and restore drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 settings config-file read fencing — 2026-08-20

- Exact files changed: `crates/melodee-persistence/Cargo.toml`,
  `crates/melodee-persistence/src/settings.rs`,
  `crates/melodee-persistence/tests/settings.rs`, this report, the R5 release-gate report,
  `docs/src/content/docs/installation/configuration.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the settings config-file boundary now reads at most the bounded document size
  through a held no-follow parent descriptor, checks the opened leaf is a regular file, and
  rejects symlink replacement. No requirement was marked terminal; R5.1 remains Partial.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo test -p melodee-persistence --test settings --all-features` passed 7/7 and
  `cargo clippy -p melodee-persistence --all-targets --all-features -- -D warnings` passed;
  `cargo fmt --all`, the smoke gate, and the exact full gate also passed after the change.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin wire contract changed. The
  internal `MELODEE__CONFIG_FILE` input now fails closed for symlink or special-file replacement
  on supported Unix targets; valid regular JSON config files retain their precedence and format.
- Data/migration impact: no database, migration source, canonical media, or target data was
  modified.
- Security/data-loss impact: positive fail-closed correction; a config-file pathname race cannot
  redirect the settings resolver to an unintended file, and oversized input remains bounded.
- Performance/operational impact: one bounded descriptor walk and one regular-file check per
  config-file load; no steady-state request or worker path changed.
- Documentation impact: Updated — `docs/src/content/docs/installation/configuration.md`, this
  report, the R5 release-gate report, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: the broader R5.1 review still requires real approximately-5-TB
  topology and restore-target validation; release owner/operator owns that evidence.
- External input still required: approved isolated production snapshot/backup/canonical mapping
  and explicit authorization for production migration and restore drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 CLI input-file fencing — 2026-08-20

- Exact files changed: `apps/melodee-cli/src/main.rs`, `apps/melodee-cli/tests/config.rs`, this
  report, the R5 release-gate report, `docs/src/content/docs/installation/configuration.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: chart CSV, config import, scripting context, and ad-hoc script inputs now use one
  bounded regular-file reader that walks parent components without following symlinks and holds
  the parent while opening the leaf. No requirement was marked terminal; R5.1 remains Partial.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo test -p melodee-cli --test config --all-features` passed 4/4, including refusal of
  a symlinked config-import input; `cargo fmt --all`, Clippy, the smoke gate, and exact full gate
  passed after this slice.
- Compatibility impact: native v1, OpenSubsonic, and Jellyfin contracts are unchanged. Valid
  regular CLI input files retain their existing formats, bounds, and command semantics.
- Data/migration impact: no database, migration source, canonical media, or target data was
  modified by the implementation or tests.
- Security/data-loss impact: positive fail-closed correction; a path replacement cannot redirect a
  chart import, configuration apply, or script evaluation to an unintended file, and all reads
  remain bounded before parsing.
- Performance/operational impact: one descriptor walk and regular-file check per operator input;
  no server request or background-worker path changed.
- Documentation impact: Updated — `docs/src/content/docs/installation/configuration.md`, this
  report, the R5 release-gate report, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: the broader R5.1 review still requires real approximately-5-TB
  topology and restore-target validation; release owner/operator owns that evidence.
- External input still required: approved isolated production snapshot/backup/canonical mapping
  and explicit authorization for production migration and restore drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 export and unsupported-filesystem fencing — 2026-08-20

- Exact files changed: `apps/melodee-cli/Cargo.toml`, `Cargo.lock`,
  `apps/melodee-cli/src/main.rs`, `apps/melodee-cli/tests/config.rs`,
  `crates/melodee-search/src/generation.rs`, `crates/melodee-search/src/secure_fs.rs`,
  `crates/melodee-media/src/secure_fs.rs`, `crates/melodee-ingestion/src/secure_fs.rs`,
  `docs/src/content/docs/installation/configuration.md`, this report, the R5 release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the R5.1 audit now covers CLI config export and unsupported-platform fallbacks.
  Config exports are written to a new regular file through a held no-follow parent descriptor,
  atomically installed with no-replace rename, and refuse existing destinations. Search, media,
  and ingestion secure filesystem fallbacks now return `Unsupported` instead of silently using
  path-based operations where the Unix no-follow guarantee is unavailable. The redundant search
  generation path-based permission rewrite was removed because descriptor-relative creation
  already applies owner-only mode. No requirement was marked terminal; R5.1 remains Partial.
- ADRs: accepted ADR-0134 and ADR-0121 remain applicable; no ADR was accepted or applied in this
  slice. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; CLI config integration tests passed 4/4 including existing
  destination and symlink-target export regressions; search passed 4 library, 1 benchmark-smoke,
  6 rebuild-integration, 28 search, and 0 doctest tests; media passed 40/40 with 1 documented
  performance test ignored; ingestion passed 32/32; and Clippy with `-D warnings` passed for
  CLI, search, media, and ingestion.
- Compatibility impact: CLI config export now refuses an existing output path and documents the
  behavior; API wire contracts and native v1, OpenSubsonic, and Jellyfin compatibility products
  are unchanged. Unsupported platforms fail closed for affected filesystem operations rather than
  risking an unsafe fallback.
- Data/migration impact: no database, migration source, canonical media, or production file was
  modified. Export writes are isolated to the explicitly requested new path; no existing file is
  replaced.
- Security/data-loss impact: positive fail-closed correction; config export cannot redirect through
  a parent/leaf symlink or overwrite an existing destination, and unavailable no-follow primitives
  no longer degrade to path-based search/media/ingestion writes or cleanup.
- Performance/operational impact: export performs one bounded parent descriptor walk, one temporary
  file write/fsync, and one atomic rename; normal Unix search/media/ingestion behavior is unchanged.
- Documentation impact: Updated — `docs/src/content/docs/installation/configuration.md`, this
  internal R5 report, the R5 release-gate report, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: the complete R5.1 review still requires real production-topology and
  restore-target validation; release owner/operator owns that evidence. Existing Windows-specific
  podcast publication paths remain separately bounded and require platform acceptance if Windows
  is a release target.
- External input still required: approved isolated production snapshot/backup/canonical mapping,
  platform target confirmation where non-Unix support is required, and authorization for migration
  and restore drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 search-root creation race hardening — 2026-08-20

- Exact files changed: `crates/melodee-search/src/config.rs`,
  `crates/melodee-search/src/secure_fs.rs`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the R5.1 filesystem audit now covers search-root startup creation. Missing
  components are created one at a time through descriptor-relative no-follow `mkdirat`; existing
  components must be directories; the canonical path is checked against the originally opened
  directory identity; and a regular file cannot be accepted as a search root. No requirement was
  marked terminal; R5.1 remains Partial pending real topology and restore review.
- ADRs: ADR-0121 remains the accepted search-root authority. No ADR was accepted or applied;
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-search --all-features` passed 4
  library, 1 benchmark-smoke, 6 rebuild-integration, 28 search, and 0 doctest tests; the new
  regular-file-root regression and existing symlink-root regression passed; and
  `cargo clippy -p melodee-search --all-targets --all-features -- -D warnings` passed.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, range behavior, and intentional 410/501 behavior are
  unchanged.
- Data/migration impact: no database, migration source, canonical media, or search data was
  modified; this only hardens rebuildable projection-root startup validation.
- Security/data-loss impact: positive fail-closed correction; a path replacement between root
  validation and creation/canonicalization cannot redirect the search root through an external
  symlink, and regular-file roots are rejected before generation writes.
- Performance/operational impact: one bounded descriptor walk for each missing component and two
  held-directory identity checks at startup; no query, rebuild, lease, or worker limits changed.
- Documentation impact: Updated — this internal R5 report and `IMPLMENTATION_OUTSTANDING.md`;
  no public documentation changed because search-root behavior remains fail-closed and internal.
- Remaining risks and owner: real production topology and restore-target review remain with the
  release owner/operator; unsupported platforms retain a fail-closed startup result for this
  identity check.
- External input still required: approved isolated production snapshot/backup/canonical mapping
  and authorization for the real rehearsal and restore.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6/R5.1 topology fact reuse regression — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, this report,
  `design/evidence/r4/README.md`, `design/evidence/r5/release-gate-report.md`,
  and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the integration probe's storage-topology calculation now
  compares the canonical-root device recorded from its held no-follow
  directory descriptor with the backup device recorded from its held backup
  descriptor. It performs no second pathname lookup after those facts are
  validated. R5.1 remains Partial pending real-topology and restore-target
  review.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151
  remain Proposed.
- Tests: `python3 -m unittest tests/python/test_r4_integration_probe.py -v`
  passed 10/10. The topology regression patches `Path.stat` to fail, proving
  that the result uses descriptor facts rather than a reopened path.
- Compatibility impact: none; all three compatibility products retain their
  existing wire contracts.
- Data/migration impact: no source, target, canonical media, backup, or
  restore target was modified.
- Security/data-loss impact: positive; a path replacement cannot make the
  topology result disagree with the validated objects.
- Performance/operational impact: one fewer pathname lookup and no change to
  application workload or migration throughput.
- Documentation impact: Updated — this R5 report, the R4 report, the release
  gate report, and `IMPLMENTATION_OUTSTANDING.md`; no public documentation
  changed because this is internal validation tooling.
- Remaining risks and owner: release owner/operator must validate the actual
  production storage and restore topology and preserve sanitized evidence.
- External input still required: approved isolated snapshot/backup paths,
  canonical mapping, and authorization for the external rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 media normalization source/output fencing — 2026-08-20

- Exact files changed: `crates/melodee-media/Cargo.toml`, `Cargo.lock`,
  `crates/melodee-media/src/lib.rs`, `crates/melodee-media/src/normalize.rs`,
  `crates/melodee-media/src/secure_fs.rs`, `crates/melodee-ingestion/src/secure_fs.rs`, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the media inspection and normalization read/copy/output boundary now uses
  descriptor-relative no-follow operations on supported Unix targets; the initial inspection
  metadata snapshot and image-header read now use the same boundary; source and destination
  leaves cannot be symlinks at open time, generated output parents are walked without following
  links, retained copies use exclusive destination creation, unique attempt directories use
  verified-parent `mkdirat`, and FFmpeg output collisions fail closed with `-n`. No requirement
  was marked terminal; R5.1 remains Partial pending the real topology and restore-target review.
- ADRs: no new ADR was accepted or applied. ADR-0145 remains the accepted acquisition/staging
  authority; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-media --all-features` passed 40 unit
  tests, 13 real-format normalization tests, and 0 doctests, with the pre-existing performance
  harness as the single ignored test; the two new focused symlink/output regressions passed 1/1
  each; `cargo clippy -p melodee-media --all-targets --all-features -- -D warnings` passed;
  `./scripts/check.sh` passed from the current tree with 201 Python tests, the Admin UI static
  gate and 248 unit tests, the full workspace/PostgreSQL Rust and doctest matrix, formatting,
  workspace Clippy with warnings denied, and cargo-deny; and `git diff --check` passed. The media
  source/destination/parent-symlink/FIFO tests preserved external sentinels, returned promptly on
  a special-file input, and no test weakened or masked a correctness path.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, wire DTOs,
  properties, identifiers, statuses, auth, pagination, range behavior, and intentional error
  behavior are unchanged.
- Data/migration impact: no schema or migration mapping changed; canonical source media remains
  read-only, retained normalization copies bytes into the isolated attempt area, and no source,
  target database, or canonical media root was accessed by the tests.
- Security/data-loss impact: positive fail-closed correction for check-to-open source races,
  symlinked directory components, destination redirection, and accidental output overwrite. The
  Unix implementation uses `O_NOFOLLOW`, `O_EXCL`, and descriptor-relative directory operations;
  the existing non-Unix fallback remains platform-limited and does not claim stronger race
  resistance than its standard-library primitives.
- Performance/operational impact: one descriptor-relative parent walk/open per protected media
  leaf and one bounded copy stream for retained outputs; no unbounded retries, concurrency, or
  public runtime contract changed. A production-sized performance/capacity claim is not made.
- Documentation impact: Updated — this internal report, `design/evidence/r5/release-gate-report.md`,
  and `IMPLMENTATION_OUTSTANDING.md`; no public `/docs` change was needed because behavior remains
  internal and fail-closed.
- Remaining risks and owner: external tool processes still receive stable source paths rather than
  inherited descriptors, so operator-controlled source mutation during a tool invocation remains
  bounded by the existing before/after hash and verification checks; the release owner/operator
  still must review the approximately 5-TB topology, restore target, and production drill.
- External input still required: approved isolated production snapshot, backup, canonical-root
  mapping, and operator authorization for production-sized rehearsal and rollback review.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

The follow-up source-opener audit added `O_NONBLOCK` and an `fstat` regular-file check to the
Unix media and ingestion no-follow openers. A raced FIFO or device is now rejected before a worker
can block or consume special-file input. Retained media copies also call `sync_all` after the
exclusive copy stream. The focused media suite, the complete `cargo test -p melodee-ingestion
--all-features` suite, and both crate Clippy commands with `-D warnings` passed; the external
source/destination sentinels remained unchanged. No public contract or migration data changed.

### R5.2/R5.6 runtime-base authority correction and guarded Bookworm smoke — 2026-08-20

- Exact files changed: `Dockerfile`, `design/adr/ADR-0151-use-debian-trixie-runtime-base-for-release-images.md`,
  this report, `design/evidence/r5/adr-0151-decision-package.md`,
  `design/evidence/r5/release-gate-report.md`, `IMPLMENTATION_OUTSTANDING.md`,
  `design/evidence/r5/artifacts/local-20260820/bookworm-guarded-amd64-metadata.json`,
  `design/evidence/r5/artifacts/local-20260820/bookworm-guarded-amd64-trivy-full.json`,
  `design/evidence/r5/artifacts/local-20260820/bookworm-guarded-amd64-sbom.spdx.json`, and
  `design/evidence/r5/artifacts/local-20260820/bookworm-guarded-SHA256SUMS`.
- Requirements: the source image definition now faithfully retains the accepted Debian Bookworm
  runtime while ADR-0151 is Proposed. No requirement was marked terminal; R5.2 and R5.6 remain
  Partial.
- ADRs: no ADR was accepted or applied. ADR-0151 remains Proposed; the earlier Trixie images and
  scans remain historical decision-package triage and are not treated as candidates.
- Tests: `docker build --tag melodee:r5-bookworm-guarded-local --build-arg VERSION=0.1.0
  --build-arg REVISION=2877e2c-dirty .` passed. The resulting local image ID was
  `sha256:2eb48219e152cd959b96e897a71d0d6fa3ef98bc53aba99df9e4da64746b649a` and its OCI labels
  identified version `0.1.0` and revision `2877e2c-dirty`. The disposable
  `SMOKE_RUN_ID=local-bookworm-trace-20260820 SMOKE_PORT=18130 ./scripts/container_smoke.sh`
  run passed the non-root UID, media-tool, CLI-version, labels, PostgreSQL-backed health and
  readiness, Admin SPA, and graceful SIGTERM checks. An archive-isolated Trivy 0.72.0 scan of
  this exact local image reported 11 critical, 193 high, 296 medium, 231 low, and 110 unknown
  findings; Syft produced an SPDX 2.3 SBOM with 301 packages. The metadata, report, SBOM, and
  checksums are `bookworm-guarded-amd64-metadata.json`,
  `bookworm-guarded-amd64-trivy-full.json`, `bookworm-guarded-amd64-sbom.spdx.json`, and
  `bookworm-guarded-SHA256SUMS` under `design/evidence/r5/artifacts/local-20260820/`. The image
  was not published or presented as an RC, and the findings were not suppressed.
  The exact-tree `./scripts/check.sh` then passed with 201 Python tests, Admin UI typecheck,
  lint, i18n, build, and 248 unit tests, the complete PostgreSQL/workspace Rust and doctest
  matrix, formatting, workspace Clippy with warnings denied, and cargo-deny.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, or range behavior changed.
- Data/migration impact: none; the build and smoke test used disposable local containers and did
  not access a migration source, production database, or canonical media root.
- Security/data-loss impact: positive authority correction; the source no longer implements the
  disputed ADR-0151 runtime-base proposal before decision-owner acceptance. No vulnerability
  finding was suppressed, and the historical Trixie findings remain visible.
- Performance/operational impact: local amd64 build and bounded smoke duration only; no production
  capacity or multi-architecture claim is made.
- Documentation impact: Updated — this report, the ADR-0151 decision package, the release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed.
- Remaining risks and owner: the release owner must dispose ADR-0151 and the accepted severity
  policy, after which a fresh candidate-SHA amd64/arm64 build, scan, SBOM, provenance, and smoke
  run are required.
- External input still required: ADR-0151 disposition, candidate builder/scanner/provenance,
  release-owner acceptance, and all outstanding R4 production/client evidence.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.5 requirement override structural audit — 2026-08-20

- Exact files changed: `scripts/audit_requirement_overrides.py`,
  `tests/python/test_requirement_override_audit.py`, `scripts/check.sh`,
  `scripts/check-smoke.sh`, `scripts/bootstrap.sh`,
  `.github/workflows/release-container.yml`,
  `tests/python/test_container_smoke.py`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the normal audit now verifies that all 239 PRD requirements
  have exactly one known structured override, allowed status vocabulary, and
  list-shaped implementation/test evidence. Strict release mode fails closed
  on the 12 current non-terminal requirements; no requirement was falsely
  promoted.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151
  remain Proposed.
- Tests: `python3 scripts/audit_requirement_overrides.py` passed with
  239/239 structural coverage and counts of 211 tested, 9 implemented,
  12 in-progress, and 7 deferred; `--require-terminal` correctly failed on
  the named 12 requirements; the focused regression suite passed 2/2;
  `./scripts/check-smoke.sh` passed; and the subsequent `./scripts/check.sh`
  passed with 200 Python tests, the Admin UI gate, the full Rust/PostgreSQL
  and doctest matrix, formatting, Clippy, and cargo-deny.
- Compatibility impact: none; no public route, DTO, property, identifier,
  status, auth, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: none; the validator reads requirement metadata only
  and does not access databases, migration sources, or media roots.
- Security/data-loss impact: positive fail-closed release evidence; missing,
  unknown, malformed, or non-terminal requirement states cannot be concealed
  by the aggregate check.
- Performance/operational impact: one bounded JSON/PRD audit in the local
  gates; the published-release quality gate now invokes strict mode after
  `check.sh`, before either architecture can build or publish.
- Documentation impact: Updated — this R5 report and
  `IMPLMENTATION_OUTSTANDING.md`; public documentation unchanged.
- Remaining risks and owner: release owner must provide truthful terminal
  dispositions for the 12 in-progress requirements after external evidence
  and decisions arrive.
- External input still required: production snapshot/client/provider evidence,
  ADR dispositions, scheduled candidate-SHA gates, and release-owner acceptance.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.5 release workflow terminal-status enforcement — 2026-08-20

- Exact files changed: `.github/workflows/release-container.yml`,
  `tests/python/test_container_smoke.py`, this report,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the release workflow now runs
  `audit_requirement_overrides.py --require-terminal` after the full repository
  gate and before the architecture build matrix. The current 12 in-progress
  requirements therefore fail closed instead of being silently carried into a
  published release; no status was changed.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain
  Proposed.
- Tests: static workflow regression assertions pass; `actionlint` passes when
  available; repository validation, requirement-index validation, diff checks,
  normal override audit, and the current smoke gate pass. Strict mode was
  separately verified to fail on the named 12 nonterminal requirements.
- Compatibility impact: none; no public route, DTO, property, identifier,
  status, auth, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: none; this is release-workflow validation only.
- Security/data-loss impact: positive fail-closed release control; an attempted
  publication cannot proceed with untruthful requirement closure metadata.
- Performance/operational impact: one bounded Python audit in the release job;
  the architecture build and publication jobs remain downstream of the gate.
- Documentation impact: Updated — this internal R5 report, the release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`; public documentation unchanged.
- Remaining risks and owner: release owner/CI administrator must provide a
  successful candidate-SHA run after the external decisions and evidence are
  available.
- External input still required: ADR-0149/0150/0151 dispositions, production
  migration and integration evidence, physical-client evidence, successful
  scheduled/candidate-SHA workflows, and release-owner publication authority.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 race-resistant partial cleanup — 2026-08-20

- Exact files changed: `crates/melodee-ingestion/src/secure_fs.rs`,
  `crates/melodee-ingestion/src/acquisition.rs`, this report,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: partial-ingestion cleanup and delayed inbound cleanup now
  remove only through verified directory descriptors with `O_NOFOLLOW`,
  `SYMLINK_NOFOLLOW`, and descriptor-relative unlink operations on supported
  Unix targets. Symlinked partial roots and internal symlinks fail closed; the
  outside target remains untouched. No requirement was marked terminal.
- ADRs: no ADR was accepted or applied. The accepted ADR-0145 acquisition
  handoff boundary remains in force; ADR-0149, ADR-0150, and ADR-0151 remain
  Proposed.
- Tests: focused `cargo test -p melodee-ingestion acquisition::tests::partial_cleanup --lib`
  passed 2/2; the full `melodee-ingestion` package matrix passed; and the
  subsequent smoke gate passed after the delayed inbound cleanup change. The
  preceding full gate covered 201 Python tests, the Admin UI gate, all
  workspace/PostgreSQL Rust tests and doctests, formatting, Clippy, and
  cargo-deny.
- Compatibility impact: none; no public route, DTO, property, identifier,
  status, auth, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: positive data-loss protection; only disposable or
  explicitly owned partial staging trees are eligible, and legacy source/media
  paths are not touched.
- Security/data-loss impact: positive race and symlink containment; suspicious
  entries are retained for operator diagnosis rather than followed or removed.
- Performance/operational impact: partial cleanup performs descriptor-relative
  directory traversal and bounded directory-entry deletion; unsupported
  platforms fail closed instead of using an unsafe recursive fallback.
- Documentation impact: Updated — this internal R5 report, the release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`; public documentation unchanged.
- Remaining risks and owner: the complete R5.1 review still requires the real
  production topology and restore-target operator review; owner is the release
  owner with storage/operator support.
- External input still required: production snapshot/topology, restore target,
  and operator authorization for production migration drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 owned podcast cleanup fencing — 2026-08-20

- Exact files changed: `apps/melodee-server/src/podcast_worker.rs`, this
  report, `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: orphan, failed-download, and retention cleanup of owned
  podcast files now opens the owned root with `NOFOLLOW`, inspects the direct
  child with `SYMLINK_NOFOLLOW`, and unlinks only a regular file through the
  root descriptor. Missing files remain idempotently clean; symlinks and
  special entries fail closed.
- ADRs: no ADR was accepted or applied. Existing podcast-retention and
  ownership authority remains in force; ADR-0149, ADR-0150, and ADR-0151
  remain Proposed.
- Tests: focused `cargo test -p melodee-server
  owned_cleanup_rejects_symlink_without_touching_the_target --lib` passed 1/1;
  focused server Clippy passed with warnings denied; the outside sentinel
  regression remained byte-identical.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire
  contracts are unchanged.
- Data/migration impact: positive loss prevention for owned podcast storage;
  canonical media and migration sources are not involved.
- Security/data-loss impact: positive race/symlink containment; cleanup cannot
  follow or overwrite an external target through a replaced podcast path.
- Performance/operational impact: one descriptor-relative `statat`/`unlinkat`
  per owned file; missing files remain a successful idempotent outcome.
- Documentation impact: Updated — this internal R5 report, the release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`; public documentation unchanged.
- Remaining risks and owner: full R5.1 still requires production-topology and
  restore-target review; release owner with storage/operator support owns it.
- External input still required: production snapshot/topology, restore target,
  and operator authorization for migration drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.2 isolated worktree secret scan — 2026-08-20

- Exact files changed: `IMPLMENTATION_OUTSTANDING.md`, this report, and the release-gate report.
- Requirements: local R5.2 secret-scan preparation now has direct Gitleaks evidence for the
  current worktree. No requirement was marked terminal; candidate-SHA security evidence remains
  required.
- ADRs: no ADR was accepted or applied. ADR-0151 remains Proposed.
- Tests: Gitleaks `v8.30.0` scanned a 175.22 MB archive of the current worktree through an
  isolated disposable container without a host bind mount; it reported `no leaks found` and exit
  code 0. The archive excluded only generated build/dependency trees (`target`, UI dependency and
  build directories, and `.git`); repository source, documentation, evidence, and untracked
  closure files were included.
- Compatibility impact: none; no server or adapter behavior changed.
- Data/migration impact: none; the scan was read-only and used no database or media root.
- Security/data-loss impact: positive local evidence; no secret finding was suppressed and no
  credentials were printed. This does not replace the candidate-SHA/GitHub security gate.
- Performance/operational impact: bounded disposable scan completed in 5.36 seconds and examined
  approximately 175.22 MB.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed.
- Remaining risks and owner: final candidate-SHA secret scan, deployment review, image policy
  disposition, and release-owner acceptance remain open.
- External input still required: candidate SHA and successful scheduled/security workflow evidence;
  no registry or production endpoint was contacted.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 current native migrated replay confirmation — 2026-08-20

- Exact files changed: `design/evidence/r4/dual-run-report-native.json`,
  `design/evidence/r4/README.md`, this report, and the release-gate report.
- Requirements: the current native migrated differential was rerun and remains fail-closed at
  267 classified and 185 unclassified field differences. No requirement was marked terminal.
- ADRs: no ADR was accepted or applied. ADR-0150 remains Proposed; its reference-only metadata
  boundary and the separate catalog ordering decision remain unresolved.
- Tests: `python3 scripts/run_r4_dual_run.py --only native --skip-build` exited 1 because
  unexplained differences remain, which is expected evidence behavior. No comparator exclusions
  were widened.
- Compatibility impact: none; no public contract or compatibility DTO changed.
- Data/migration impact: none; only disposable migrated replay databases and roots were used.
- Security/data-loss impact: positive fail-closed evidence; no semantic difference was hidden.
- Performance/operational impact: replay remained bounded and reproducible; no production-sized
  performance claim is made.
- Documentation impact: Updated — this R5 evidence report, the R4 report, and the release-gate
  report; no public documentation changed.

### R4.2 second isolated existing-snapshot rehearsal — 2026-08-20

- Exact files changed: `design/evidence/r4/rehearsal-drill-existing-snapshot-r4dbg-second.json`,
  `design/evidence/r4/README.md`, this report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.2 rehearsal repeatability preparation now has a second no-seed run against an
  already populated isolated source, with backup validation, source/target counts, search rebuild,
  restore equality, canonical content hashing, and rollback retention recorded. No requirement
  was marked terminal; the production-sized rehearsal remains open.
- ADRs: ADR-0148 remains accepted migration authority; no ADR was accepted or applied. ADR-0149,
  ADR-0150, and ADR-0151 remain Proposed.
- Tests: the artifact records `result=pass`, zero verification mismatch gaps, zero duplicate
  public IDs, matching 10/10/19/1 source and target counts, a 549,673-byte validated backup,
  20 unchanged canonical files totaling 4,093,382 bytes with content hashes, and
  `source_mutated_by_harness=false`.
- Compatibility impact: none; all three compatibility products retain their existing contracts.
- Data/migration impact: the existing-snapshot source was not seeded, written, or dropped by the
  drill; only isolated target and restore databases were created for the rehearsal.
- Security/data-loss impact: positive bounded evidence for read-only source handling, backup
  restore equality, canonical media immutability, and failed-target preservation.
- Performance/operational impact: fixture-only import RSS was 30,027,776 bytes; no production
  scale or capacity claim is made.
- Documentation impact: Updated — the R4/R5 evidence reports and ledger; no public documentation
  changed because the operator procedure is unchanged.
- Remaining risks and owner: release owner/operator approval and the real approximately-5-TB
  snapshot/topology remain required.
- External input still required: approved production snapshot, backup, canonical mapping, and
  production-topology authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.
- Remaining risks and owner: compatibility/release owners must resolve the residuals and ADR
  dispositions before R4 exit and R5 entry.
- External input still required: ADR-0149/0150/0151 decisions, approved production snapshot and
  topology, physical clients, providers, and candidate-SHA evidence.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 current migrated differential rerun — 2026-08-19

- Exact files changed: `design/evidence/r4/dual-run-report-native.json`,
  `design/evidence/r4/dual-run-report-opensubsonic.json`,
  `design/evidence/r4/dual-run-report-jellyfin.json`,
  `design/evidence/r4/README.md`, `design/evidence/r5/README.md`,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the isolated migrated replay was rerun from the current working tree. Native
  completed all 12 groups and retained 185 unclassified differences; OpenSubsonic completed 11
  groups and retained 333; Jellyfin completed eight groups and retained 17. No stale count was
  promoted to zero and no comparator rule was widened.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed;
  metadata, ordering, user-state, provider, playlist, and production decisions remain open.
- Tests: `python3 scripts/run_r4_dual_run.py --only native --skip-build`,
  `python3 scripts/run_r4_dual_run.py --only opensubsonic --skip-build`, and
  `python3 scripts/run_r4_dual_run.py --only jellyfin --skip-build` all executed against
  disposable migrated PostgreSQL targets and wrote the committed surface reports. Each command
  exited 1 because the report still contains unexplained differences; this is expected fail-closed
  evidence, not a passing differential gate. The native report contains 267 classified/185
  unclassified differences; the OpenSubsonic report contains 318 classified/333 unclassified;
  the Jellyfin report contains 149 classified/17 unclassified.
- Compatibility impact: no route, DTO family, identifier, status, auth, pagination, range, or
  intentional 410/501 contract changed.
- Data/migration impact: both runs created and dropped only the explicitly named disposable
  `r4_dual_source`/`r4_dual_target` databases and isolated temporary roots; canonical media was
  generated under `/tmp` and the source snapshot was read-only during import.
- Security/data-loss impact: the harness remained fail-closed on verification and differential
  residuals; no production endpoint, credential, registry, or canonical media root was used.
- Performance/operational impact: the current replay remains reproducible but is intentionally
  not release-grade until the unclassified residuals reach zero.
- Documentation impact: Updated — `design/evidence/r4/README.md`, the two committed reports,
  and this R5 evidence report; no public documentation changed.
- Remaining risks and owner: release owner/compatibility owner must resolve the residual
  metadata/order/state/provider/playlist semantics and provide approved production-sized and
  physical-client evidence.
- External input still required: ADR-0149/0150 dispositions, approved production snapshot and
  topology, and owner-authorized final differential acceptance.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 final migration projection writes — 2026-08-19

- Exact files changed: `apps/melodee-cli/src/legacy/import.rs`,
  `apps/melodee-cli/src/legacy/import_state4.rs`, `design/evidence/r5/README.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: final catalog projection rebuilding and excluded-table issue summarization now
  refresh and verify the owning lease inside their target transactions before writing. R5.1
  remains Partial because the real production topology and complete threat-model gate remain
  external.
- ADRs: ADR-0145 and ADR-0148 remain the accepted acquisition/migration authorities; no new ADR
  was applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-cli --test legacy --test legacy_faults
  --test legacy_freeze` passed 22/22 with 0 failures or ignored tests, including the
  production-shaped bounded import suite.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire contracts are unchanged.
- Data/migration impact: no schema or source mapping changed; legacy source and canonical media
  remain read-only, while final target projection writes are atomic and owner-fenced.
- Security/data-loss impact: a worker that loses its lease cannot append excluded-table audit
  issues or rebuild target participation/aggregate projections after fencing loss.
- Performance/operational impact: one additional lease refresh per final projection transaction
  and one per excluded-summary transaction; both operations remain bounded by import completion.
- Documentation impact: Updated — this internal R5 report and `IMPLMENTATION_OUTSTANDING.md`; no
  public behavior documentation changed.
- Remaining risks and owner: full adversarial race coverage and review against the approved
  approximately 5-TB topology remain with the release owner/operator.
- External input still required: approved production snapshot/topology and release-owner R4/R5
  decisions before formal phase promotion.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 verification issue writes — 2026-08-19

- Exact files changed: `apps/melodee-cli/src/legacy/verify.rs`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: verification mismatch issues now refresh and verify the owning migration lease in
  the same target transaction before recording a durable mismatch. R5.1 remains Partial because
  the complete threat-model review and production-topology evidence remain external.
- ADRs: ADR-0145 and ADR-0148 remain accepted; no new ADR was applied. ADR-0149, ADR-0150, and
  ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; the complete legacy/fault/freeze suite remains 22/22 passed after
  this correction, and `./scripts/check-smoke.sh` passed.
- Compatibility impact: none; no compatibility-product contract changed.
- Data/migration impact: no schema or source-data change; verification remains non-destructive and
  a stale worker cannot append a mismatch issue after lease loss.
- Security/data-loss impact: closes the remaining un-fenced verification audit write identified in
  the local migration-path review.
- Performance/operational impact: one lease refresh per mismatch-issue transaction; clean verify
  completion was already fenced.
- Documentation impact: Updated — this internal R5 report and `IMPLMENTATION_OUTSTANDING.md`; no
  public behavior documentation changed.
- Remaining risks and owner: adversarial lease-expiry race coverage and real-topology review remain
  with the release owner/operator.
- External input still required: approved isolated snapshot/topology and release-owner decisions.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 current-tree full-gate rerun — 2026-08-19

- Exact files changed: no source or workflow files; this report and
  `design/evidence/r5/release-gate-report.md` record the rerun.
- Requirements: the full local gate remains green, but no requirement was marked terminal because
  external R4 acceptance, scheduled candidate-SHA evidence, accepted ADR dispositions, and
  release-owner acceptance remain open.
- ADRs: none applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `./scripts/check.sh` passed with exit code 0, including 181 Python tests, Admin UI 44
  files/248 tests, all workspace/PostgreSQL tests, formatting, Clippy, and cargo-deny. The
  command's existing non-failing Fast Refresh and duplicate/unmatched-license warnings remain.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire contracts are unchanged.
- Data/migration impact: none; this was a local verification run only.
- Security/data-loss impact: no production endpoint, migration source, canonical media, registry,
  or release publication was accessed.
- Performance/operational impact: the complete local gate remains reproducible from the current
  dirty tree; scheduled and multi-architecture execution remain unverified.
- Documentation impact: Updated — this internal R5 report and the release-gate report.
- Remaining risks and owner: external R4 evidence, ADR decisions, scheduled workflows, native
  arm64 build/scan, vulnerability policy, and release-owner acceptance remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.4 scheduled workflow evidence audit — 2026-08-19

- Exact files changed: `design/evidence/r5/README.md`,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the repository already contains a scheduled Full validation workflow, but R5.4
  remains Not started because no successful run from a release-candidate SHA is recorded. The
  latest scheduled run, [32214289862](https://github.com/melodee-project/melodee-rs/actions/runs/32214289862),
  ran at older SHA `207054be6fe37e84fdb32066ddae98987ce14bb2` and failed the CLI search-root
  safety assertion; this is recorded as failure history, not a waiver or candidate evidence.
- ADRs: none applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo test -p melodee-cli --test search
  search_status_rejects_overlap_with_an_inbound_source_without_leaking_paths -- --nocapture`
  passed 1/1 on current HEAD/working tree. Read-only `gh run view 32214289862 --log-failed`
  confirmed the remote failure and its exact assertion.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin contract changed.
- Data/migration impact: none; no database, media, migration, or workflow execution was mutated.
- Security/data-loss impact: positive evidence clarification; the failure remains visible and no
  scheduled result is promoted to a green release gate.
- Performance/operational impact: none locally; a successful candidate-SHA scheduled run is still
  required to establish repeatability on the release candidate.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: release owner/CI owner must provide a candidate SHA and successful
  scheduled workflow URL, then investigate any candidate-specific flakes without suppressing
  correctness assertions.
- External input still required: release-candidate SHA/acceptance and CI execution authority.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 exact scanned-image publication hardening — 2026-08-19

- Exact files changed: `.github/workflows/release-container.yml`,
  `scripts/validate_release_evidence.py`, `tests/python/test_container_smoke.py`,
  `tests/python/test_release_evidence.py`, `design/evidence/r5/README.md`,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R5.6 publication preparation now preserves the exact image digest that passed
  per-architecture smoke and vulnerability scans. Each matrix leg archives `image.tar.gz` and
  includes it in the validated manifest/checksum bundle; the publication job verifies the bundle,
  loads the exact archives, pushes unique staging references, and creates final multi-architecture
  manifest tags from those references without rebuilding an unscanned image. R5.6 remains Partial;
  no accepted candidate, passing vulnerability policy, final provenance, or release-owner approval
  exists; local amd64/arm64 artifacts are recorded separately below.
- ADRs: no ADR was accepted or applied. ADR-0151 remains Proposed; this change does not select a
  runtime base or suppress a vulnerability finding.
- Tests: `python3 -m unittest tests.python.test_release_evidence
  tests.python.test_container_smoke -v` passed 11/11; `actionlint
  .github/workflows/release-container.yml` passed. The archive is required by both the evidence
  validator and workflow completeness check; checksum and symlink regression tests remain green.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, identifiers,
  status codes, auth, pagination, and range behavior are unchanged.
- Data/migration impact: none; this changes only release artifact transport and verification. No
  database, migration source, canonical media, or production registry was accessed.
- Security/data-loss impact: positive release-integrity change; publication cannot silently rebuild
  an image after its scan, and a missing/tampered archive or evidence file fails closed. Staging
  references are unique to the authorized release workflow run.
- Performance/operational impact: publication adds artifact download, image load, and staging
  pushes; it avoids an extra release rebuild and makes final manifest composition deterministic
  from scanned inputs. Native arm64 builder performance remains unverified.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because no runtime behavior
  changed.
- Remaining risks and owner: release owner/CI owner must execute the workflow on an accepted
  candidate, resolve the exact image vulnerability policy, verify both architecture scans and
  provenance, and authorize publication. Staging cleanup policy after a failed manifest push
  remains an operational release-run concern.
- External input still required: accepted ADR-0151 disposition, release-candidate acceptance,
  GitHub Actions execution, and explicit publication authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 machine-readable evidence package report — 2026-08-19

- Exact files changed: `scripts/validate_release_evidence.py`,
  `tests/python/test_release_evidence.py`, `.github/workflows/release-container.yml`,
  `tests/python/test_container_smoke.py`, `design/evidence/r5/release-gate-report.md`,
  `IMPLMENTATION_OUTSTANDING.md`, and this report.
- Requirements: the per-architecture prepublication bundle now has a stable schema-v1 manifest
  containing image identity, architecture, runtime user, release labels, report sizes/hashes, and
  checksum coverage. The validator runs before and after SHA256SUMS creation and never contacts a
  registry; R5.6 remains Partial because no accepted candidate or external multi-architecture run
  exists.
- ADRs: ADR-0144 remains the accepted publication authority. ADR-0151 remains Proposed and was
  not accepted or applied.
- Tests: `python3 -m unittest tests/python/test_release_evidence.py
  tests/python/test_container_smoke.py` passed 9/9; `actionlint
  .github/workflows/release-container.yml`, `python3 -m py_compile
  scripts/validate_release_evidence.py`, repository validation, requirements-index validation, and
  `git diff --check` passed. The subsequent `./scripts/check.sh` passed with 181 Python tests,
  Admin UI 44 files/248 tests, all workspace/PostgreSQL tests, formatting, Clippy, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire contracts are unchanged.
- Data/migration impact: none; the validator reads only CI-produced evidence files and does not
  access a database, migration source, or canonical media root.
- Security/data-loss impact: missing, empty, mismatched, or tampered image/scan/SBOM/checksum files
  fail closed; no vulnerability is suppressed and no publication is authorized.
- Performance/operational impact: validation is bounded by streaming SHA-256 reads and small JSON
  metadata; the artifact is self-describing while the existing 14-day retention and arm64 build
  path remain unchanged.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: GitHub Actions execution, arm64 build/scan, vulnerability policy,
  provenance, scheduled gates, and release-owner acceptance remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 operator-help audit — 2026-08-19

- Exact files changed: `scripts/container_smoke.sh`,
  `tests/python/test_container_smoke.py`, `design/evidence/r5/README.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the bounded release-image smoke command now exposes a dependency-free
  `--help`/`-h` path before environment validation, and rejects unexpected arguments. The
  operator command audit also executed the documented rehearsal, differential, client, probe,
  evidence-validator, and `melodee-cli legacy --help` paths successfully. R5.6 remains Partial;
  no candidate artifact or external multi-architecture run exists.
- ADRs: ADR-0144 remains the accepted publication authority; ADR-0151 remains Proposed and was
  not accepted or applied.
- Tests: `python3 -m unittest tests/python/test_container_smoke.py -v` passed 7/7;
  `bash scripts/container_smoke.sh --help`, the documented Python `--help` commands,
  `melodee-cli legacy --help`, `bash -n scripts/container_smoke.sh`, and `git diff --check`
  passed without Docker access or secret-bearing configuration.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire contracts are unchanged.
- Data/migration impact: none; help and argument validation do not access databases, migration
  sources, canonical media, or registries.
- Security/data-loss impact: the help path fails before image or Docker evaluation, and unknown
  arguments fail closed; no publication or registry mutation is possible through this change.
- Performance/operational impact: operators can verify the smoke command interface without
  provisioning its dependencies; runtime smoke behavior is unchanged.
- Documentation impact: Updated — this internal R5 report and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: CI execution, arm64 build/scan, vulnerability policy, provenance,
  scheduled gates, and release-owner acceptance remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 arm64 local-build attempt — 2026-08-19

- Exact files changed: `design/evidence/r5/README.md` and
  `IMPLMENTATION_OUTSTANDING.md`; no image or registry artifact was committed.
- Requirements: a fresh local `docker buildx build --platform linux/arm64 --load` attempt was
  allowed to run for approximately 1,663 seconds against the uniquely named
  `melodee:r5-arm64-local` image. It installed the arm64 Debian runtime and reached the Rust
  server compilation stage, but was canceled before image load because emulation remained too
  slow. R5.6 remains Partial and no arm64 digest, smoke, SBOM, or scan result is claimed.
- ADRs: ADR-0144 remains the accepted publication authority; ADR-0151 remains Proposed and was
  not accepted or applied.
- Tests: the build command was locally bounded and canceled with no image produced; subsequent
  `docker image inspect melodee:r5-arm64-local` confirmed the image is absent. The existing
  amd64 smoke/evidence validator and full local gates remain the qualifying local results.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin contract changed.
- Data/migration impact: none; the build accessed only repository build inputs and did not access
  a database, migration source, canonical media, or production endpoint.
- Security/data-loss impact: no artifact was published or used as release evidence; the absence
  of an arm64 result remains fail-closed rather than being represented as a pass.
- Performance/operational impact: the QEMU attempt demonstrates that a native arm64 builder is
  required for practical candidate production; the release workflow's arm64 matrix leg remains
  unverified until CI or native hardware runs it.
- Documentation impact: Updated — this internal R5 report and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: native arm64 build/scan, exact-candidate vulnerability policy,
  provenance, scheduled gates, and release-owner acceptance remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 search-generation manifest hardening — 2026-08-19

- Exact files changed: `crates/melodee-search/src/generation.rs`,
  `crates/melodee-search/tests/search_tests.rs`, `design/evidence/r5/README.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: search-generation manifest reads now reject symlink/non-regular files and
  oversized manifests; sealing replaces only a stale regular pending file, creates the pending
  file with `create_new`, syncs it before activation, and rejects a pre-existing pending symlink.
  R5.1 remains Partial because the real production topology and restore-target review are absent.
- ADRs: no new ADR was applied; the accepted search-generation authority remains in force and
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-search --test search_tests
  --all-features` passed 27/27, including external-target preservation and stale-pending retry
  regressions. `./scripts/check-smoke.sh` and the final current `./scripts/check.sh` also passed;
  the full gate included 174 Python tests, Admin UI 44 files/248 tests, workspace
  Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire contracts are
  unchanged.
- Data/migration impact: none; only rebuildable search projection metadata is affected, and no
  legacy database or canonical media is accessed.
- Security/data-loss impact: a replaced manifest or pending symlink can no longer be used as a
  read/write path through the normal seal operation; stale regular pending state remains
  recoverable without following links.
- Performance/operational impact: manifest sealing adds bounded metadata validation and a file
  sync before atomic rename; no catalog query or worker interval changes.
- Documentation impact: Updated — this internal R5 evidence report and
  `IMPLMENTATION_OUTSTANDING.md`; no public behavior documentation changed.
- Remaining risks and owner: full race coverage across the approved production topology and
  restore-target review remain with the release owner/operator; R4 external evidence still gates
  formal R5 entry.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 local prepublication matrix hardening — 2026-08-19

- Exact files changed: `.github/workflows/release-container.yml`,
  `tests/python/test_container_smoke.py`, `design/evidence/r5/release-gate-report.md`, and this
  report.
- Requirements: both-architecture prepublication build/smoke/scan preparation is strengthened;
  no release requirement was marked terminal because no candidate artifact, accepted severity
  disposition, SBOM/provenance bundle, or release-owner acceptance exists.
- ADRs: ADR-0144 remains the accepted publication authority; ADR-0151 remains Proposed and was
  not accepted or applied by this change.
- Tests: `python3 -m unittest tests/python/test_container_smoke.py -v` passed 6/6;
  `bash -n scripts/container_smoke.sh`, `actionlint .github/workflows/release-container.yml`,
  `git diff --check`, `./scripts/check-smoke.sh`, and the final `./scripts/check.sh` passed. The
  final full gate included 174 Python tests, Admin UI 44 files/248 tests, workspace
  Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin contracts are unchanged.
- Data/migration impact: none; the workflow and static assertions do not access migration sources
  or canonical media.
- Security/data-loss impact: the publication dependency now requires both architecture-specific
  smoke and exact-image vulnerability-scan legs; no finding is suppressed and no publication is
  authorized.
- Performance/operational impact: arm64 uses conditional QEMU setup in CI and a distinct smoke
  port/run identifier; native arm64 builder performance remains unproven.
- Documentation impact: Updated — this internal R5 evidence report and the release-gate report.
- Remaining risks and owner: GitHub Actions execution, arm64 artifact/scan, vulnerability policy,
  SBOM/provenance, scheduled gates, and release-owner acceptance remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 per-architecture evidence capture — 2026-08-19

- Exact files changed: `.github/workflows/release-container.yml`,
  `tests/python/test_container_smoke.py`, `design/evidence/r5/release-gate-report.md`,
  `IMPLMENTATION_OUTSTANDING.md`, and this report.
- Requirements: pre-publication release evidence now has a bounded capture path for each matrix
  architecture: image ID/labels/architecture, SPDX SBOM, JSON vulnerability reports, scan-status
  metadata, and checksums. No requirement was marked terminal because the workflow has not run for
  an accepted candidate and the release-owner decision remains absent.
- ADRs: ADR-0144 remains the accepted publication authority. ADR-0151 remains Proposed; this
  change does not select or change a runtime base or severity policy.
- Tests: `python3 -m unittest tests/python/test_container_smoke.py -v` passed 6/6;
  `bash -n scripts/container_smoke.sh`, `actionlint .github/workflows/release-container.yml`,
  `python3 scripts/validate_repository.py`, `python3 scripts/extract_requirements.py --check`,
  `git diff --check`, `./scripts/check-smoke.sh`, and the final `./scripts/check.sh` all passed;
  the final full gate included 174 Python tests, Admin UI 44 files/248 tests, workspace
  Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin contracts are unchanged.
- Data/migration impact: none; the workflow inspects only the newly built local CI image and does
  not access migration sources or canonical media.
- Security/data-loss impact: the existing title/source/license/version/revision label checks remain
  fail-closed; architecture is checked against the matrix; scan failures still block publication;
  no vulnerability is suppressed.
- Performance/operational impact: each architecture emits an independently verifiable artifact
  bundle; artifact retention is 14 days and arm64 still depends on QEMU in the current workflow.
- Documentation impact: Updated — this internal R5 evidence report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: GitHub Actions execution, arm64 build/scan, exact candidate severity
  disposition, provenance attestation, scheduled gates, and release-owner acceptance remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 fail-closed evidence upload — 2026-08-19

- Exact files changed: `.github/workflows/release-container.yml`,
  `tests/python/test_container_smoke.py`, `design/evidence/r5/release-gate-report.md`,
  `IMPLMENTATION_OUTSTANDING.md`, and this report.
- Requirements: the multi-architecture pre-publication workflow now treats missing evidence
  artifacts as a matrix failure instead of a warning, and explicitly checks every required
  metadata, SBOM, scan, status, and checksum file. No requirement was marked terminal because no
  accepted candidate or GitHub Actions run exists.
- ADRs: ADR-0144 remains the accepted publication authority; ADR-0151 remains Proposed and was
  not accepted or applied.
- Tests: focused container workflow tests passed 6/6; `bash -n scripts/container_smoke.sh`,
  `actionlint .github/workflows/release-container.yml`, repository validation, requirements
  check, `git diff --check`, `./scripts/check-smoke.sh`, and the final current
  `./scripts/check.sh` all passed. The final full gate included 174 Python tests, Admin UI 44
  files/248 tests, workspace Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin contracts are unchanged.
- Data/migration impact: none; no migration source, database, or canonical media is accessed.
- Security/data-loss impact: absent image metadata, SBOM, scan reports, status, or checksums can no
  longer be silently accepted by the upload step or by a finalizer-generated status file; no
  findings are suppressed and publication is still gated on the complete matrix.
- Performance/operational impact: none beyond failing a release matrix leg when its evidence
  directory is absent; the existing 14-day artifact retention and arm64 QEMU path are unchanged.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: GitHub Actions execution, arm64 build/scan, vulnerability policy,
  provenance, scheduled gates, and release-owner acceptance remain open.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 preparatory package report — 2026-08-19

- Exact files changed: `scripts/container_smoke.sh`, `tests/python/test_container_smoke.py`,
  `.github/workflows/release-container.yml`, and this report.
- Requirements: R5.6 local artifact-smoke preparation is strengthened; no release requirement was
  marked terminal because the candidate, arm64 artifact, SBOM/provenance, and release-owner
  acceptance are still absent.
- ADRs: ADR-0144 remains the accepted publication authority; ADR-0151 remains Proposed and was
  not accepted or applied by this change.
- Tests: `bash -n scripts/container_smoke.sh` passed; the focused container-smoke suite passed
  4/4; the disposable container smoke passed and reported image ID
  `sha256:1a8551faf7b973aa78f6cbdcef3be7364eae07c98935a971ef977359e9eea552`, version `0.1.0`,
  and exact OCI label validation.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin contract changed.
- Data/migration impact: none; the smoke harness creates and removes only uniquely named disposable
  Docker objects and does not access migration or canonical-media data.
- Security/data-loss impact: the reusable smoke path now fails closed on a non-`sha256` image ID,
  incorrect title/source/version/license labels, or an incorrect supplied revision; no findings
  are suppressed.
- Performance/operational impact: the workflow uses the same reusable label/image-ID verifier as
  local smoke, removing duplicated shell validation while preserving the existing disposable
  PostgreSQL/readiness/SIGTERM checks.
- Documentation impact: Updated — this R5 evidence report.
- Remaining risks and owner: exact candidate vulnerability policy, arm64 build/scan, SBOM,
  provenance, scheduled gates, and release-owner acceptance remain owned by the release owner.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 local hardening package report — 2026-08-19

- Exact files changed: `crates/melodee-ingestion/src/acquisition.rs`,
  `design/evidence/r5/README.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R5.1 local no-follow cleanup coverage is strengthened; no requirement was marked
  terminal because the real production topology and restore-target review remain external.
- ADRs: no new ADR was applied; ADR-0145 remains the accepted acquisition authority and ADR-0151,
  ADR-0149, and ADR-0150 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-ingestion --test acquisition
  --all-features` passed 13/13; the new focused unit test
  `acquisition::tests::delayed_cleanup_never_follows_a_replaced_inbound_symlink` passed 1/1.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, or range contract changed.
- Data/migration impact: cleanup now classifies a replaced inbound symlink/non-directory as a new
  occurrence and retains it; no legacy database, canonical media, or migration schema is changed.
- Security/data-loss impact: recursive cleanup no longer relies on a path-following existence check
  before removing inbound or partial staging trees; the regression proves an external symlink target
  survives unchanged.
- Performance/operational impact: one metadata syscall is added before bounded cleanup; replacement
  is recorded for reconciliation instead of risking deletion.
- Documentation impact: Updated — this internal R5 evidence report and
  `IMPLMENTATION_OUTSTANDING.md`; no public behavior documentation changed.
- Remaining risks and owner: full operation review against the approved approximately 5 TB topology
  and restore target remains with the release owner/operator.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 evidence-bundle path containment — 2026-08-19

- Exact files changed: `scripts/validate_release_evidence.py`,
  `tests/python/test_release_evidence.py`, and this report.
- Requirements: R5.6 local evidence validation now rejects a symlinked evidence directory,
  symlinked parent component, symlinked required report, non-regular required report, or
  symlinked `SHA256SUMS` before reading or hashing it. No release requirement was marked
  terminal because the candidate, arm64 artifact, accepted severity disposition, provenance,
  and release-owner acceptance remain absent.
- ADRs: ADR-0144 remains the accepted publication authority; ADR-0151 remains Proposed and was
  not accepted or applied.
- Tests: `python3 -m unittest tests/python/test_release_evidence.py -v` passed 4/4;
  `python3 scripts/validate_release_evidence.py --help`, Python compilation, and
  `git diff --check` passed.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin contracts are unchanged.
- Data/migration impact: none; the validator only inspects a supplied evidence bundle and does
  not access migration sources, PostgreSQL, or canonical media.
- Security/data-loss impact: evidence validation cannot follow a link outside the bundle while
  loading or hashing required metadata, SBOM, scan reports, or checksums; no vulnerability or
  scan result is suppressed.
- Performance/operational impact: one bounded `lstat` validation pass is added before evidence
  reads; normal bundle validation remains content-hash based.
- Documentation impact: Updated — this internal R5 evidence report; no public documentation
  changed because this is release-engineering validation behavior.
- Remaining risks and owner: exact candidate GitHub Actions execution, arm64 build/scan,
  vulnerability policy, provenance, scheduled gates, and release-owner acceptance remain open.
- External input still required: release-owner disposition of ADR-0151 and an accepted R5
  candidate before any R6 work or publication.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 disabled-podcast phase replay correction — 2026-08-20

- Exact files changed: `scripts/run_r4_dual_run.py`,
  `tests/python/test_scripts.py`,
  `design/evidence/r4/dual-run-report-opensubsonic.json`,
  `design/evidence/r4/README.md`, `design/evidence/r5/README.md`,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the R4 dual-run harness now includes the separate
  `opensubsonic-conditional-off.json` phase that the legacy oracle captures
  outside its main scenario list. The three disabled-podcast captures now
  replay and compare successfully; no requirement was marked terminal because
  333 other OpenSubsonic differences remain.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151
  remain Proposed; the correction changes only replay coverage.
- Tests: `python3 -m unittest tests/python/test_scripts.py -q` passed 33/33;
  `python3 -m py_compile scripts/run_r4_dual_run.py` passed;
  `python3 scripts/run_r4_dual_run.py --help` passed; the migrated
  OpenSubsonic replay exited 1 fail-closed with 318 classified and 333
  unclassified differences, and all three conditional-off scenarios had no
  unexplained differences.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO,
  property, status, auth, pagination, or range behavior changed. OpenSubsonic
  differential coverage increased by three required conditional captures.
- Data/migration impact: none; the replay continued to use disposable
  migrated databases and isolated roots.
- Security/data-loss impact: positive evidence correction; a required phase can
  no longer be silently omitted from the migrated differential.
- Performance/operational impact: one additional isolated disabled-podcast
  replay phase per OpenSubsonic run; no production runtime path changed.
- Documentation impact: Updated — this report, the R4 report, the release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: release owner/compatibility owner must resolve
  the remaining 333 OpenSubsonic differences and provide external client and
  production evidence.
- External input still required: accepted ADR-0150 disposition and approved
  migrated snapshot/topology for final differential closure.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.2/R5.6 exact multi-architecture scan and SBOM evidence — 2026-08-20

- Exact files changed: `design/evidence/r5/artifacts/local-20260820/` (four generated
  machine-readable reports and `SHA256SUMS`), `design/evidence/r5/adr-0151-decision-package.md`,
  `design/evidence/r5/release-gate-report.md`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: local R5.2/R5.6 evidence now covers both architecture vulnerability scans,
  exact-image SBOM generation, and checksums. No requirement was marked terminal; candidate-SHA
  provenance, severity-policy acceptance, RC acceptance, and publication remain open.
- ADRs: no ADR was accepted or applied; ADR-0151 remains Proposed.
- Tests/artifacts: Trivy 0.72.0 scanned the exact amd64 image
  `sha256:1a8551faf7b973aa78f6cbdcef3be7364eae07c98935a971ef977359e9eea552` and exact arm64 image
  `sha256:73ab8baaffaa79c4986873df0f6ce35ae770408f9fc41db9d297af2d88f3204b` through a disposable
  container/archive transfer. Both reported 8 critical, 187 high, 203 medium, 183 low, and 87
  unknown findings. Syft 1.51.0 generated SPDX 2.3 SBOMs with 299 amd64 and 296 arm64 packages.
  The committed `SHA256SUMS` covers both Trivy reports and both SBOMs; both exact images also passed
  the previously recorded container smoke runs.
- Compatibility: none; no native v1, OpenSubsonic, or Jellyfin route, DTO, identifier, status,
  authentication, pagination, or range behavior changed.
- Data/migration: none; scanning and SBOM generation used disposable local image archives and no
  migration source, target database, or canonical media.
- Security/data-loss: positive evidence improvement with no suppression or comparator masking;
  both images fail the current critical/high policy and remain release-blocked.
- Performance/operational impact: archive transfer scanned 242 MB amd64 and 237 MB arm64 images;
  no production builder or registry performance claim is made.
- Documentation impact: Updated — `IMPLMENTATION_OUTSTANDING.md`, the ADR-0151 decision package,
  the release-gate report, this report, and generated local scan/SBOM evidence; no public docs.
- Remaining risks and owner: release owner must decide ADR-0151 and the accepted severity policy,
  then obtain exact candidate-SHA scans, signed provenance, scheduled gates, RC acceptance, and all
  outstanding R4 evidence.
- External input still required: ADR-0151 disposition, candidate-SHA builder/scanner/provenance,
  scheduled workflow success, release-owner RC acceptance, and the outstanding production/client/
  migration evidence.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.2/R5.6 local arm64 artifact retry — 2026-08-20

- Exact files changed: `IMPLMENTATION_OUTSTANDING.md`,
  `design/evidence/r5/README.md`, and
  `design/evidence/r5/release-gate-report.md`.
- Requirements: local multi-architecture preparation now includes a successful Linux arm64
  BuildKit/QEMU build and exact-image smoke test; no requirement was marked terminal. Candidate-SHA
  scanning, provenance, and release-owner acceptance remain open.
- ADRs: no ADR was accepted or applied; ADR-0151 remains Proposed.
- Tests/artifact: `docker buildx build --platform linux/arm64 --load --tag
  melodee:r5-arm64-local --build-arg VERSION=0.1.0 --build-arg REVISION=2877e2c-dirty
  --progress=plain .` completed with the Rust release build taking 36m08s. The exact image ID is
  `sha256:73ab8baaffaa79c4986873df0f6ce35ae770408f9fc41db9d297af2d88f3204b`, with Linux arm64,
  version 0.1.0, and revision `2877e2c-dirty`. `SMOKE_RUN_ID=local-arm64-20260820
  SMOKE_PORT=18093 ./scripts/container_smoke.sh` passed. A disposable Trivy 0.72.0 container
  scanned both exact architecture archives through `docker cp`; each reported 8 critical, 187 high,
  203 medium, 183 low, and 87 unknown findings. Syft 1.51.0 generated SPDX 2.3 SBOMs with 299
  amd64 and 296 arm64 packages. The complete reports, SBOMs, and checksums are committed under
  `design/evidence/r5/artifacts/local-20260820/`.
- Compatibility: no native v1, OpenSubsonic, or Jellyfin wire, route, DTO, identifier, status,
  auth, pagination, or range changes.
- Data/migration: none; the build, smoke test, and failed scan attempts used disposable local
  resources and no migration source, target database, or canonical media.
- Security/data-loss: both architecture scans are now recorded without suppression; the critical
  and high findings remain a release blocker and require policy/owner disposition.
- Performance/operational impact: the arm64 cross-build took 36m08s under local QEMU; this is not
  a production-builder performance claim.
- Documentation impact: Updated — `IMPLMENTATION_OUTSTANDING.md`, this report, and
  `design/evidence/r5/release-gate-report.md`; no public documentation changed.
- Remaining risks and owner: ADR-0151, exact candidate-SHA scanners/provenance, RC acceptance, and
  all outstanding R4 gates remain with the
  release owner/operator.
- External input still required: ADR-0151 disposition, approved candidate-SHA scan/SBOM/provenance,
  candidate-SHA scheduled validation, RC acceptance, and the outstanding R4 snapshot,
  client, provider, production-topology, and ADR decisions.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.4 scheduled-run environment isolation — 2026-08-20

- Exact files changed: `apps/melodee-cli/tests/search.rs`,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the CLI search-root safety regression now supplies its own inbound
  staging and normalization work-area roots, so it does not depend on a developer
  `.env` file. No requirement was marked terminal; R5.4 still requires a successful
  scheduled run from the exact release-candidate SHA.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain
  Proposed.
- Tests: `cargo fmt --all --check` passed; the focused search test passed 10/10
  consecutive runs. The latest observed scheduled run
  [`32330365945`](https://github.com/melodee-project/melodee-rs/actions/runs/32330365945)
  failed before this correction because the runner had no `.env`; no new workflow
  run was dispatched.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire
  contracts are unchanged.
- Data/migration impact: none; the test uses a disposable temporary root and does
  not access migration sources, target databases, or canonical media.
- Security/data-loss impact: positive test determinism and preserved fail-closed
  overlap validation; no error-path masking or comparator suppression was added.
- Performance/operational impact: no runtime path changed; the test adds only
  deterministic child-root configuration for its subprocess.
- Documentation impact: Updated — this internal R5 report,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed.
- Remaining risks and owner: a successful scheduled/nightly run from the exact
  candidate SHA remains owned by CI/release operations, along with external R4
  acceptance and release-owner decisions.
- External input still required: candidate SHA available to CI and authorization
  to run or wait for the scheduled workflow; ADR-0149/0150/0151 dispositions and
  production/client evidence remain required.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 current OpenSubsonic fixture-isolation rerun — 2026-08-20

- Exact files changed: `scripts/legacy_oracle.py`, `scripts/run_r4_dual_run.py`,
  `tests/python/test_native_v1_capture.py`, `design/evidence/r4/README.md`,
  `design/evidence/r4/dual-run-report-opensubsonic.json`, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: local R4.4 differential reproducibility is improved; three clean-target
  OpenSubsonic share-count fixture differences were removed, but 330 unclassified differences
  remain. No R5 requirement was marked terminal and R5 is not formally active.
- ADRs: none applied or accepted; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the 39-test native capture fixture suite passed; both modified Python scripts compiled;
  CLI help checks passed; the isolated migrated OpenSubsonic replay completed all 11 groups and
  exited 1 fail-closed with 330 unclassified differences.
- Compatibility impact: no runtime compatibility product or public contract changed.
- Data/migration impact: only disposable migrated source/target databases and temporary roots were
  used; native-only fixture state is now excluded from clean OpenSubsonic/Jellyfin targets.
- Security/data-loss impact: positive fixture isolation; no broad comparator mask or waiver was
  added.
- Performance/operational impact: no runtime impact; one deterministic fixture branch per replay.
- Documentation impact: Updated — R4/R5 evidence reports, release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: release owner/compatibility owner must resolve the remaining
  differential residuals and provide external R4/R5 evidence.
- External input still required: accepted ADR dispositions, approved production snapshot/topology,
  physical clients, production integration endpoints, scheduled CI, and candidate acceptance.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 current amd64 container smoke rerun — 2026-08-20

- Exact files changed: `design/evidence/r5/README.md` and
  `design/evidence/r5/release-gate-report.md`.
- Requirements: R5.6 local container-artifact preparation has a second exact-image smoke
  observation from the pre-restoration Trixie prototype. No requirement was marked terminal; the
  image remains historical triage rather than an accepted release candidate.
- ADRs: no ADR was accepted or applied. ADR-0144 remains the accepted publication authority;
  ADR-0151 remains Proposed and still governs any runtime-base change.
- Tests: `SMOKE_IMAGE=melodee:r5-iterative-local RELEASE_VERSION=0.1.0
  RELEASE_REVISION=2877e2c-dirty SMOKE_RUN_ID=local-current-20260820-0716 SMOKE_PORT=18128
  ./scripts/container_smoke.sh` passed for the historical Trixie prototype. The exact image ID was
  `sha256:1a8551faf7b973aa78f6cbdcef3be7364eae07c98935a971ef977359e9eea552`.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin contract changed.
- Data/migration impact: none; the smoke run used disposable Docker objects and a disposable
  PostgreSQL container, with no migration source or canonical media root.
- Security/data-loss impact: the harness again proved non-root runtime and bounded shutdown/cleanup;
  the recorded exact-image scan reports 8 critical and 187 high findings, so no security gate is
  claimed and no finding was suppressed.
- Performance/operational impact: startup, readiness, CLI, SPA, media-tool availability, and
  SIGTERM behavior were rechecked; no production performance claim is inferred.
- Documentation impact: Updated — this internal R5 report and the release-gate report; no public
  documentation changed because the runtime behavior was not changed.
- Remaining risks and owner: release owner/ADR-0151 decision owner and CI/security owners retain
  the arm64 build, exact candidate scan, provenance, scheduled validation, and RC acceptance risks.
- External input still required: ADR-0151 disposition, reliable amd64/arm64 candidate builders,
  release-owner candidate acceptance, and explicit publication authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### Current-tree full-gate rerun after fixture isolation — 2026-08-20

- Exact files changed: no additional implementation files; the gate covered the current dirty
  tree after the `legacy_oracle.py` and `run_r4_dual_run.py` fixture-lifecycle correction.
- Requirements: the local correctness gate remains green; no requirement was marked terminal and
  no external R4/R5 evidence was inferred.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `./scripts/check.sh` completed with exit code 0, including 193 Python tests, Admin UI
  static checks with 44 files and 248 unit tests, all workspace/PostgreSQL Rust tests and doctests,
  formatting, Clippy with warnings denied, and cargo-deny. The separate smoke gate, repository
  validation, requirements-index check, and client-matrix validator also passed.
- Compatibility impact: none; the fixture correction changes only disposable differential setup,
  not native v1, OpenSubsonic, or Jellyfin routes, DTOs, properties, identifiers, statuses, auth,
  pagination, range, or intentional 410/501 behavior.
- Data/migration impact: none outside disposable source/target databases and temporary roots used
  by the replay; no canonical media or migration source was modified.
- Security/data-loss impact: no finding was suppressed; cargo-deny remained green with its recorded
  non-failing warnings.
- Performance/operational impact: no production runtime path changed. The full gate completed its
  existing production-shaped local integration suites.
- Documentation impact: Updated — this report, the release-gate report, the R4 report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: release owner/compatibility owner still own differential closure,
  ADR dispositions, and all external R4/R5 acceptance evidence.
- External input still required: accepted ADR-0149/0150/0151 dispositions, approved production
  snapshot/topology, physical clients, deployment integration endpoints, scheduled CI, and
  candidate acceptance.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.2/R5.1 composite user-state and existing-snapshot rehearsal — 2026-08-20

- Exact files changed: `apps/melodee-cli/src/legacy/import_state2.rs`,
  `apps/melodee-cli/src/legacy/verify.rs`, `apps/melodee-cli/tests/legacy.rs`,
  `scripts/legacy_rehearsal.py`, `tests/python/test_legacy_rehearsal.py`,
  `design/evidence/r4/rehearsal-drill-existing-snapshot.json`, this report,
  `design/evidence/r4/README.md`, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: production-shaped repeated user-state facts now use their composite
  `(user, catalog entity)` identity and no longer collide in the public-entity mapping
  table. The existing-snapshot rehearsal path has a committed passing artifact. No
  requirement was marked terminal and R5 remains formally inactive.
- ADRs: ADR-0148 remains applied; no schema change or new ADR was accepted. ADR-0149,
  ADR-0150, and ADR-0151 remain Proposed.
- Tests: the CLI legacy suite passed 11/11; the shared-track multi-user regression passed
  1/1; `python3 -m unittest tests.python.test_legacy_rehearsal -q` passed 14/14; and the
  existing-snapshot drill passed with zero verification gaps, zero duplicate public IDs,
  successful search rebuild and restore-content validation, source read-only facts,
  `source_mutated_by_harness=false`, and unchanged canonical media.
- Compatibility impact: none; all three compatibility products and their public contracts
  are unchanged.
- Data/migration impact: fixes migration state idempotency/verification only; it does not
  modify source data or canonical media and introduces no schema migration.
- Security/data-loss impact: positive fail-closed correction; a multi-user source no longer
  aborts on an invalid uniqueness assumption, and no source deletion or comparator waiver was
  added.
- Performance/operational impact: bounded composite-key checks and batch verification; the
  101-track existing-snapshot run recorded 2.72 seconds import wall time and 30,527,488 bytes
  peak import RSS.
- Documentation impact: Updated — this report, the R4 report, the release-gate report,
  `IMPLMENTATION_OUTSTANDING.md`, and the committed rehearsal artifact.
- Remaining risks and owner: approximately-5-TB production evidence, external compatibility,
  physical-client, provider, scheduled, security, and release-owner acceptance remain open.
- External input still required: approved isolated production snapshot/backup/canonical mapping,
  ADR dispositions, client/operator access, deployment endpoints, and candidate acceptance.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.3/R5.4 Admin UI and public-docs gate closure — 2026-08-20

- Exact files changed: `admin-ui/e2e/mockApi.ts`, `admin-ui/e2e/overview.spec.ts`,
  `admin-ui/e2e/smoke.spec.ts`, `admin-ui/e2e/system-workers.spec.ts`, the three
  `admin-ui/e2e/visual.spec.ts-snapshots/system-workers-*.png` baselines, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: local Admin UI correctness evidence now covers the current `SystemHealth` DTO
  shape, including podcast storage, disk, and worker verdicts. The stale fixtures had omitted
  those required fields and caused a deterministic `undefined.status` route-render failure on
  Overview, Workers, and Diagnostics; all affected fixtures now match the current contract. No
  requirement was marked terminal because candidate-SHA and external release gates remain open.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the isolated desktop regression suite passed 12/12. The complete
  `cd admin-ui && pnpm test:e2e` matrix passed 289 tests with 62 intentional skips and zero
  failures across accessibility, keyboard, functional, localization/RTL, responsive, and visual
  coverage. The docs production gate passed:
  `cd docs && pnpm install --frozen-lockfile && pnpm check && pnpm check:links && pnpm build`,
  with 0 Astro diagnostics, valid relative/internal links, and 42 pages built. The generated
  System Workers visual baselines were rerun and passed 3/3.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, or range behavior changed. The UI fixtures and visual
  baselines now represent the already-defined health response contract.
- Data/migration impact: none; the changes are isolated to disposable browser fixtures and
  screenshots, and do not access PostgreSQL, migration sources, or canonical media.
- Security/data-loss impact: positive test-coverage correction; stale payloads can no longer hide
  a route-render crash, and no comparator, secret, or security finding was suppressed.
- Performance/operational impact: no runtime service behavior changed. The full browser matrix
  remains bounded and repeatable; the only visual changes are the three required health rows and
  their expected panel reflow.
- Documentation impact: Updated — this internal R5 report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation content changed because the fix aligns
  tests with an existing API contract.
- Remaining risks and owner: candidate-SHA execution, scheduled/nightly workflow success,
  physical-client acceptance, production integration/topology evidence, differential closure,
  vulnerability policy, and release-owner ADR/candidate decisions remain open with the release
  owner/operator/CI owners.
- External input still required: approved production snapshot and canonical-root mapping,
  deployment/provider/client access, ADR-0149/0150/0151 decision-owner dispositions, and
  release-owner acceptance before R5/R6 promotion or publication.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.4 repeated current-tree full-gate confirmation — 2026-08-20

- Exact files changed: `IMPLMENTATION_OUTSTANDING.md`, this report, and the release-gate report.
- Requirements: the complete local gate was run twice consecutively from the same current dirty
  worktree after the second rehearsal evidence update. No requirement was marked terminal; R5.4
  remains Not started because scheduled/nightly candidate-SHA evidence is still required.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: both `./scripts/check.sh` runs exited 0. Each covered repository and requirement
  validation, 195 Python tests, the Admin UI static gate with 248 unit tests, the complete
  workspace/PostgreSQL Rust and doctest matrix, `cargo fmt --check`, workspace Clippy with
  `-D warnings`, and cargo-deny. The separate `./scripts/check-smoke.sh` rerun also exited 0.
  Known non-failing diagnostics remained limited to two Admin UI Fast Refresh warnings and
  cargo-deny duplicate-transitive/unmatched-optional-license warnings.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin contract changed.
- Data/migration impact: none; the gates used isolated test databases and did not access a
  production database or canonical media root.
- Security/data-loss impact: positive repeatability evidence; no tests were skipped or weakened,
  and no comparator exclusions or security suppressions were added.
- Performance/operational impact: repeated full-gate wall time was dominated by the existing
  PostgreSQL integration matrix; no production capacity claim is made.
- Documentation impact: Updated — this report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed.
- Remaining risks and owner: scheduled/nightly workflow success from an accepted candidate SHA,
  plus external production and client gates, remain release-owner/CI-owned.
- External input still required: successful scheduled Full validation URL for the accepted
  candidate SHA and release-owner acceptance; no workflow dispatch, push, tag, or publication was
  performed.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.3/R5.4 repeat UI and docs gate — 2026-08-20

- Exact files changed: `IMPLMENTATION_OUTSTANDING.md`, this report, and
  `design/evidence/r5/release-gate-report.md`; no application source changed during this
  repeatable gate run.
- Requirements: strengthens local R5.3 correctness and R5.4 repeatability evidence; no
  requirement was marked terminal because the accepted candidate SHA, scheduled workflow, and
  external gates remain outstanding.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cd admin-ui && pnpm test:e2e` scheduled 351 tests, passed all 289 executed tests, and
  reported 62 intentional configuration skips with zero failures. The executed matrix covered
  accessibility, keyboard focus, catalog/review/settings/workers flows, localization, RTL,
  responsive tablet/mobile behavior, and desktop visual baselines. The docs commands
  `cd docs && pnpm check`, `pnpm check:links`, and `pnpm build` passed with 0 errors, warnings,
  or hints, valid relative/internal links, and 42 pages built.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior were unchanged.
- Data/migration impact: none; all UI/docs checks used disposable fixtures and did not access
  migration sources, PostgreSQL production data, or canonical media.
- Security/data-loss impact: positive evidence only; no correctness tests were disabled and no
  comparator, secret, or vulnerability finding was suppressed.
- Performance/operational impact: local browser matrix completed in 1.6 minutes on four workers;
  no production capacity claim is made.
- Documentation impact: Updated — this internal report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public docs content changed in this repeat run.
- Remaining risks and owner: accepted candidate-SHA execution, scheduled/nightly success,
  physical-client and production integration, differential closure, vulnerability policy, and
  release-owner decisions remain open with the release owner/operator/CI owners.
- External input still required: ADR-0149/0150/0151 dispositions, approved isolated production
  snapshot/backup/canonical mapping, deployment/provider/client access, and candidate acceptance.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 delta-deletion lease fencing — 2026-08-19

- Exact files changed: `apps/melodee-cli/src/legacy/delta.rs`,
  `design/evidence/r5/README.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the delta reconciliation path now refreshes and verifies the migration lease in
  the same target transaction before recording `source.row_deleted` issues. A stale worker can no
  longer append deletion evidence after losing run ownership; the target row remains retained.
  R5.1 remains Partial because the real production topology and restore-target review are absent.
- ADRs: ADR-0148 and ADR-0145 remain the accepted migration/acquisition authorities; no new ADR was
  applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-cli --test legacy
  verify_then_delta_is_convergent -- --exact` passed 1/1. The test continues to prove a deleted
  source row is retained and represented by a durable `source.row_deleted` issue.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin wire contracts are unchanged.
- Data/migration impact: no schema or source-data change; source deletions remain non-destructive,
  and only an owner-fenced reconciliation issue is written.
- Security/data-loss impact: stale migration workers are fenced before deletion-reconciliation
  issue commits, closing a silent-loss/audit-integrity path without deleting source or target media.
- Performance/operational impact: one bounded lease refresh per deletion-issue batch; batches with
  no missing source rows perform no additional target write.
- Documentation impact: Updated — this internal R5 report and `IMPLMENTATION_OUTSTANDING.md`; no
  public behavior documentation changed.
- Remaining risks and owner: full adversarial race coverage and review against the approved
  approximately 5-TB topology remain with the release owner/operator.
- External input still required: approved production snapshot/topology and release-owner R4/R5
  decisions before formal phase promotion.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.2 disk-budget preflight and current-tree gate rerun — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, `design/evidence/r4/README.md`,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.2 rehearsal preparation now has an explicit fail-closed disk
  budget preflight and direct no-target-created regression coverage. No
  requirement was marked terminal; external R4 acceptance and formal R5 entry
  remain open.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151
  remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -q` passed
  13/13; `./scripts/check-smoke.sh` passed; the subsequent `./scripts/check.sh`
  passed with 192 Python tests, Admin UI 44 files/248 tests, all
  workspace/PostgreSQL Rust tests, formatting, Clippy, and cargo-deny.
- Compatibility impact: none; the three compatibility products and all public
  routes, DTOs, identifiers, statuses, auth, pagination, and range behavior are
  unchanged.
- Data/migration impact: no migration source, target database, or canonical
  media is accessed by the new tests; the threshold applies to the disposable
  rehearsal workdir.
- Security/data-loss impact: insufficient-headroom behavior fails before target
  creation and invalid thresholds fail before filesystem inspection; no capacity
  or production-readiness claim is inferred.
- Performance/operational impact: one bounded disk-metric query precedes target
  creation and adds thresholds to machine-readable rehearsal evidence.
- Documentation impact: Updated — `design/evidence/r4/README.md`, this report,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`; no public behavior documentation changed.
- Remaining risks and owner: release owner/operator must provide the real
  snapshot/topology, physical-client/provider gates, candidate-SHA scheduled
  execution, and ADR dispositions.
- External input still required: approved isolated snapshot, backup,
  canonical mapping, deployment/client access, and release-owner decisions.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.6 release-evidence runtime-user contract correction — 2026-08-20

- Exact files changed: `scripts/validate_release_evidence.py`,
  `tests/python/test_release_evidence.py`, and this report.
- Requirements: the release-evidence validator now validates the runtime principal actually
  declared by the Dockerfile (`melodee:melodee`); the separate container smoke gate continues to
  prove the resolved UID is 10001. No requirement was marked terminal and R5.6 remains Partial.
- ADRs: no ADR was accepted or applied. ADR-0151 remains Proposed.
- Tests: `python3 -m unittest tests.python.test_release_evidence -v` passed 5/5, including root
  rejection, symlinked-file/directory rejection, tamper detection, and checksum validation. The
  validator was then exercised against exact local amd64 and arm64 image bundles containing image
  metadata, archives, SBOMs, Trivy reports, scan status, release-evidence manifests, and
  SHA256SUMS; both bundles validated and all checksums passed.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin contract changed.
- Data/migration impact: none; this is release-evidence validation only.
- Security/data-loss impact: positive fail-closed correction; the workflow no longer rejects the
  repository's intended named non-root runtime while still refusing an unapproved runtime user.
  No vulnerability was suppressed.
- Performance/operational impact: no production runtime change; validation remains bounded by the
  captured evidence files and exact image archives.
- Documentation impact: Updated — this internal evidence report; no public behavior changed.
- Remaining risks and owner: exact candidate-SHA image scans must still pass the accepted policy,
  and final provenance, scheduled gates, release-owner acceptance, and publication remain open.
- External input still required: ADR-0151 disposition and an accepted candidate artifact; no
  registry or production endpoint was contacted.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 acquisition source reopen fencing — 2026-08-20

- Exact files changed: `crates/melodee-ingestion/src/acquisition.rs`, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: source copy and tree hashing now reopen regular files through the descriptor-
  relative no-follow boundary after metadata checks, while staging directories and first-write
  files use no-follow creation and exclusive-open semantics; no requirement was marked terminal
  because R5.1 still needs production-topology and restore-target review.
- ADRs: ADR-0145 remains the accepted acquisition handoff authority; no new ADR was accepted or
  applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; the focused symlink-source regression passed 1/1; the complete
  `cargo test -p melodee-ingestion --all-features` run passed 32 library, 13 acquisition, 5
  environment-fault, 5 inspection, 13 normalization, 14 publication, 1 publication-resilience,
  and 15 reconciliation tests, with one documented performance harness ignored; ingestion
  Clippy with `-D warnings` passed; `./scripts/check-smoke.sh` passed.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior are unchanged.
- Data/migration impact: no source, target, or canonical media mutation; regular source bytes are
  read only through the no-follow boundary, and symlink replacement fails closed.
- Security/data-loss impact: positive; a check-to-open replacement cannot redirect acquisition
  copy or hash reads to an external symlink, and a pre-existing or raced destination symlink cannot
  be followed or overwritten, while existing source symlink skipping remains.
- Performance/operational impact: one descriptor-relative reopen per copied or hashed source file;
  no unbounded work or retry behavior changed.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because the safety boundary is
  internal and behavior remains fail-closed.
- Remaining risks and owner: concurrent mutation of an approved production source and the full
  approximately-5-TB topology/restore review remain with the release owner/operator.
- External input still required: approved isolated snapshot/backup/canonical mapping and operator
  authorization for production-sized rehearsal and rollback review.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 staging destination fencing and current-tree closure gate — 2026-08-20

- Exact files changed: `crates/melodee-ingestion/src/secure_fs.rs`,
  `crates/melodee-ingestion/src/acquisition.rs`, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: first-write staging files now use descriptor-relative `O_EXCL|O_NOFOLLOW`
  creation and staging directories use the same no-follow directory walk; no requirement was
  marked terminal because R5.1 still requires production-topology and restore-target review.
- ADRs: ADR-0145 remains the accepted acquisition handoff authority; no new ADR was accepted or
  applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; the focused `direct_copy_rejects_a_symlinked_source_at_open_time`
  regression passed 1/1 and preserved both external source and destination sentinels;
  `cargo clippy -p melodee-ingestion --all-targets --all-features -- -D warnings` passed;
  `cargo test -p melodee-ingestion --all-features` passed 32 library, 13 acquisition, 5
  environment-fault, 5 inspection, 13 normalization, 14 publication, 1 publication-resilience,
  and 15 reconciliation tests, with the documented performance harness ignored; and the current
  `./scripts/check.sh` passed with 201 Python tests, Admin UI typecheck/lint/i18n/build and 248
  Admin UI unit tests, the full workspace/PostgreSQL Rust and doctest matrix, formatting,
  workspace Clippy with warnings denied, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior are unchanged.
- Data/migration impact: no source, target, or canonical media mutation; destination symlinks and
  existing files are not followed or overwritten, and all source reads remain no-follow.
- Security/data-loss impact: positive fail-closed correction for check-to-open source races and
  destination redirection/overwrite races; no broad test exclusion or comparator mask was added.
- Performance/operational impact: one descriptor-relative parent walk/open per staging leaf and
  bounded recursive cleanup; no retry, concurrency, or public runtime contract changed.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is an internal
  safety boundary with unchanged user-visible behavior.
- Remaining risks and owner: the real approximately-5-TB topology, restore-target review,
  operator rollback decision, and external production evidence remain with the release owner and
  operator.
- External input still required: approved isolated snapshot/backup/canonical mapping and
  authorization for production-sized rehearsal and rollback review.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 search-generation manifest fencing — 2026-08-20

- Exact files changed: `crates/melodee-search/src/secure_fs.rs`,
  `crates/melodee-search/src/generation.rs`, `crates/melodee-search/src/lib.rs`,
  `crates/melodee-search/tests/search_tests.rs`, this report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: search-root sentinel reads, generation manifest reads, pending-manifest writes,
  and atomic activation now use descriptor-relative no-follow filesystem operations on supported
  Unix targets. The parent directory is held open during replacement and synced after activation;
  non-regular manifests and pending entries are rejected. No requirement was marked terminal
  because R5.1 still requires the real production topology and restore-target review.
- ADRs: ADR-0121 remains the accepted search-root authority; no new ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-search --all-features` passed 3 library,
  1 benchmark-smoke, 6 rebuild-integration, 28 search, and 0 doctests; the new parent-symlink
  regression passed and preserved the external sentinel; `cargo clippy -p melodee-search
  --all-targets --all-features -- -D warnings` passed; the subsequent exact-tree
  `./scripts/check.sh` passed with 201 Python tests, Admin UI typecheck/lint/i18n/build and 248
  unit tests, the full workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy
  with warnings denied, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior are unchanged.
- Data/migration impact: no catalog, source, target, or canonical-media mutation; Tantivy remains
  a rebuildable projection and stale generations retain the existing ownership checks.
- Security/data-loss impact: positive; a concurrent generation-parent replacement cannot redirect
  manifest reads or activation writes through an operator-managed symlink, and stale pending-file
  cleanup only removes a verified regular file.
- Performance/operational impact: one descriptor-relative parent walk for each manifest operation
  and one directory sync after activation; query behavior and result limits are unchanged.
- Documentation impact: Updated — this internal R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is an internal
  safety boundary with unchanged user-visible behavior.
- Remaining risks and owner: the real approximately-5-TB topology, restore-target review, and
  cross-platform validation remain with the release owner and operator.
- External input still required: approved isolated snapshot/backup/canonical mapping and
  authorization for production-sized rehearsal and rollback review.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6/R5.1 integration-probe artifact and parser hardening — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `scripts/check_command_help.py`, `tests/python/test_r4_integration_probe.py`,
  `tests/python/test_command_help.py`, the R4 report, this report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: local R4.6/R5.1 evidence now validates the probe's stable redacted schema, rejects
  malformed OIDC/XML input as controlled failures, enforces bounded range metadata, and audits the
  integration probe's help path with a PATH-only environment. No requirement was marked terminal;
  production integration and restore-topology evidence remain open.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the focused probe and command-help suites passed 15/15; compilation, probe help, and the
  command-help audit passed. The subsequent exact `cargo fmt --all &&
  MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 212 Python tests,
  Admin UI 44 files/248 unit tests, the complete workspace/PostgreSQL Rust and doctest matrix,
  formatting, workspace Clippy with warnings denied, and cargo-deny.
- Compatibility impact: none; all three compatibility products retain their existing wire
  contracts.
- Data/migration impact: none; no source, target, canonical media, backup, or external endpoint
  was accessed or modified.
- Security/data-loss impact: positive fail-closed correction; evidence cannot contain secret-shaped
  values, XML entity declarations are rejected, and response-shape errors cannot bypass the probe's
  controlled failure path.
- Performance/operational impact: bounded in-memory validation only; no production runtime change.
- Documentation impact: Updated — the R4 report, this R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed.
- Remaining risks and owner: release owner/operator must provide and run approved production
  endpoints and restore targets; R4.6 and R5.1 remain Partial.
- External input still required: approved endpoint/test-account set, backup/restore targets, and
  authorization for external validation.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 bounded backup-probe descriptor fencing — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, this report,
  `design/evidence/r4/README.md`, `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R5.1 review now covers the local backup-integrity probe's check-to-use boundary.
  It walks absolute path components through `O_DIRECTORY|O_NOFOLLOW` descriptors, opens the leaf
  with `O_NOFOLLOW|O_NONBLOCK`, verifies a regular file with `fstat`, hashes the held descriptor,
  and gives the same descriptor to `pg_restore --list`; no requirement was marked terminal because
  the real backup/restore topology review remains open.
- ADRs: no new ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed;
  the probe does not change the accepted migration or backup contracts.
- Tests: `python3 -m unittest tests/python/test_r4_integration_probe.py -v` passed 9/9,
  including leaf-symlink and FIFO regressions; Python compilation, `--help`, `git diff --check`,
  repository validation, requirements-index validation, override audit, and the current
  `./scripts/check-smoke.sh` passed. The subsequent exact-tree `./scripts/check.sh` also passed
  with 203 Python tests, Admin UI typecheck/lint/i18n/build and 248 unit tests, the full
  workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with warnings
  denied, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior are unchanged.
- Data/migration impact: no source database, target database, canonical media, or backup content
  was mutated; the probe remains explicitly read-only and only inspects a supplied backup.
- Security/data-loss impact: positive fail-closed correction for parent/leaf replacement races,
  special-file blocking, and descriptor/path disagreement between hashing and `pg_restore`.
- Performance/operational impact: one bounded descriptor walk and one held descriptor per backup
  inspection; `pg_restore --list` remains the only external validation command and receives no
  credentials or database connection.
- Documentation impact: Updated — the R4 evidence report, this R5 report, the release-gate report,
  and `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal
  validation tooling.
- Remaining risks and owner: release owner/operator must validate the actual backup and restore
  targets, including production filesystem topology and content equality, and retain sanitized
  evidence; R5.1 remains Partial.
- External input still required: approved isolated production snapshot/backup paths and operator
  authorization for the real restore/cutover rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 descriptor-bound topology evidence — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, this report,
  `design/evidence/r4/README.md`, `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the R5.1 threat-model review now covers the topology evidence path as well as
  backup content validation. Canonical-root capacity/device facts come from a held no-follow
  directory descriptor, and same/cross-filesystem comparison reuses the backup descriptor's
  device fact instead of re-opening the supplied pathname. R5.1 remains Partial pending the
  real approximately-5-TB topology and restore-target review.
- ADRs: no new ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the focused probe suite passed 9/9; the exact-tree `./scripts/check.sh` passed with 203
  Python tests, Admin UI typecheck/lint/i18n/build and 248 unit tests, the full PostgreSQL/
  workspace Rust and doctest matrix, formatting, workspace Clippy with warnings denied, and
  cargo-deny.
- Compatibility impact: none; all three compatibility products retain their existing wire
  contracts and no client behavior changed.
- Data/migration impact: no source, target, canonical media, backup, or restore target was
  modified; only local probe code and evidence were changed.
- Security/data-loss impact: positive; path replacement cannot cause the topology artifact to pair
  a newly resolved pathname with an earlier validated backup, and special files remain rejected
  before reads.
- Performance/operational impact: bounded descriptor walks and `fstatvfs` replace path-based stats;
  no application workload or migration throughput changed.
- Documentation impact: Updated — this R5 report, the R4 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal tooling.
- Remaining risks and owner: release owner/operator must validate real production storage and
  restore topology, content equality, and deployment integration behavior.
- External input still required: approved isolated production snapshot/backup/canonical mapping
  and explicit authorization for the external rehearsal and restore.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4/R5.3 current-surface replay and isolated full-gate refresh — 2026-08-20

- Exact files changed: `design/evidence/r4/dual-run-report-native.json`,
  `design/evidence/r4/dual-run-report-opensubsonic.json`,
  `design/evidence/r4/dual-run-report-jellyfin.json`,
  `design/evidence/r4/catalog-ordering-decision-package.md`, this report,
  `design/evidence/r5/release-gate-report.md`, and the corresponding ledger/test expectation
  updates. No application source changed in this refresh.
- Requirements: current isolated migrated replays are now synchronized at native 267 classified /
  185 unclassified, OpenSubsonic 318 / 330, and Jellyfin 149 / 17. All three commands exited
  fail-closed because unexplained differences remain; no requirement was marked terminal.
  The local correctness gate was rerun against a fresh isolated PostgreSQL container after the
  repository development container exhausted stale DSM files: `./scripts/check.sh` exited 0.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed;
  metadata-dependent mappings, ordering, and the aggregate play-count residual remain decision
  gated.
- Tests: the exact full gate passed 204 Python tests, Admin UI static checks with 44 files and
  248 unit tests, the complete workspace/PostgreSQL Rust and doctest matrix including all 18
  `jellyfin_media_api` tests, `cargo fmt --check`, workspace Clippy with `-D warnings`, and
  cargo-deny. The initial run's 17 Jellyfin failures were PostgreSQL DSM allocation failures
  (`No space left on device`), not test assertions; a fresh 8 GiB-shm isolated database passed
  the affected suite 18/18 and the exact full gate afterward. The separate smoke gate is rerun
  after this evidence update.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO, property, identifier,
  status, auth, pagination, range, or intentional 410/501 behavior changed. Differential
  reports remain authoritative and fail closed.
- Data/migration impact: replay databases and the fresh test database were isolated; no production
  database, migration source, canonical media, or restore target was accessed. The seven visible
  aggregate play-count residuals remain uninvented because the source has counters without
  user-history facts.
- Security/data-loss impact: no comparator exclusion, secret suppression, test weakening, or
  finding waiver was added. The fresh test database used no repository or production volume.
- Performance/operational impact: the full gate completed its existing production-shaped local
  integration suites; the isolated database was used solely to avoid the local dev container's
  exhausted shared-memory state. No production capacity claim is made.
- Documentation impact: Updated — this internal R5 report, the R5 release-gate report, and the
  R4 ordering decision package; no public documentation changed.
- Remaining risks and owner: release owner/compatibility owner still own the 185/330/17
  differential closure, ADR dispositions, and all external R4/R5 acceptance evidence. The local
  development Postgres recovered healthy after the scoped restart; its existing persistent volume
  was not reset, and the temporary isolated test container was stopped without a repository or
  production volume.
- External input still required: ADR-0149/0150/0151 dispositions, approved isolated production
  snapshot/backup/canonical mapping, physical clients, deployment integration endpoints,
  candidate-SHA scheduled validation, release-owner candidate acceptance, and publication
  authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 canonical-media adoption descriptor fencing — 2026-08-20

- Exact files changed: `apps/melodee-cli/src/legacy/media.rs`,
  `apps/melodee-cli/src/legacy/import_tracks.rs`,
  `docs/src/content/docs/installation/legacy-migration.md`, this report, the R5 release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: canonical-media validation now uses a component-by-component no-follow walk from
  the canonical root, opens relative parent directories through held descriptors, opens the leaf
  with `O_NOFOLLOW|O_NONBLOCK`, and checks regular-file type and size from `fstat`. Absolute,
  traversal, symlink, and special-file paths fail closed as `media.unadoptable`; no requirement
  was marked terminal and R5.1 remains Partial pending real topology and restore-target review.
- ADRs: ADR-0148 remains the accepted migration authority; no ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; `cargo clippy -p melodee-cli --all-targets --all-features
  -- -D warnings` passed; `cargo test -p melodee-cli --test legacy --all-features` passed
  12/12, including the existing canonical-media symlink-escape and source-immutability checks.
  The post-change exact current-tree `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh`
  then exited 0 with 204 Python tests, 248 Admin UI unit tests, the complete workspace/PostgreSQL
  Rust and doctest matrix, formatting, workspace Clippy with `-D warnings`, and cargo-deny.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: canonical media remains read-only and no database, source snapshot,
  target, or media bytes were modified; valid regular files retain metadata-only size validation.
- Security/data-loss impact: positive fail-closed correction; pathname replacement cannot redirect
  validation to an unintended file, block on a FIFO, or admit an absolute/traversal path.
- Performance/operational impact: one bounded descriptor walk and regular-file `fstat` per
  adopted-media check; media bytes are still not read or re-encoded.
- Documentation impact: Updated — `docs/src/content/docs/installation/legacy-migration.md`, this
  report, the R5 release-gate report, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: the real approximately-5-TB canonical-root validation, production
  topology, and restore-target review remain with the release owner/operator.
- External input still required: approved isolated production snapshot/backup/canonical mapping
  and authorization for production migration and restore drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 rehearsal backup descriptor fencing — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, this report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.1/R4.8 and R5.1 local rehearsal tooling now validates an operator-supplied
  backup through a component-by-component descriptor-relative `O_NOFOLLOW` walk, rejects FIFOs and
  other non-regular leaves without reading them, hashes the held descriptor, and passes the same
  descriptor to `pg_restore --list`. Existing-snapshot canonical roots now reject symlinked
  ancestors before target creation. No requirement was marked terminal; R4.1, R4.8, and R5.1
  remain Partial pending real snapshot, topology, and restore evidence.
- ADRs: accepted ADR-0148 remains the migration tooling authority; no ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v` passed 16/16; the
  command-help audit passed for 78 CLI paths plus the rehearsal drill. The post-change exact
  `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 206 Python tests,
  248 Admin UI unit tests, the complete workspace/PostgreSQL Rust and doctest matrix, formatting,
  workspace Clippy with `-D warnings`, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin contracts are unchanged.
- Data/migration impact: the source backup and canonical root remain read-only inputs; validation
  fails before target creation when the configured paths are unsafe.
- Security/data-loss impact: positive fail-closed correction; pathname replacement cannot change
  which backup bytes are hashed or inspected by `pg_restore --list`, and special files cannot block
  the validator.
- Performance/operational impact: one bounded descriptor walk and one bounded sequential hash per
  validated backup; no production endpoint or database was contacted.
- Documentation impact: None — the operator command and valid input contract are unchanged; the
  internal evidence and implementation ledger were updated.
- Remaining risks and owner: release owner/operator still owns the approved production snapshot,
  backup integrity, real topology, and isolated restore drill.
- External input still required: approved isolated production snapshot/backup/canonical mapping and
  authorization for production migration and restore rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R5.1 podcast-root and onboarding probe fencing — 2026-08-20

- Exact files changed: `apps/melodee-server/src/podcast_worker.rs`,
  `apps/melodee-server/src/onboarding.rs`, this report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the R5.1 filesystem audit now covers podcast ownership initialization and the
  onboarding writable probe. Podcast roots are created and inspected one component at a time
  with descriptor-relative `O_DIRECTORY|O_NOFOLLOW` walks; ownership markers are checked or
  created relative to the held root descriptor; and the writable probe creates and unlinks its
  unique file relative to one held parent descriptor. No requirement was marked terminal; R5.1
  remains Partial pending real production-topology and restore-target review.
- ADRs: accepted ADR-0134 remains the podcast ownership authority; no ADR was accepted or
  applied in this slice. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; the podcast worker unit subset passed 4/4; the onboarding unit
  subset passed 5/5; and `cargo clippy -p melodee-server --all-targets --all-features -- -D warnings`
  passed. Existing ancestor-symlink, invalid-marker, owned-publication, and external-sentinel
  regressions remain green.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, range behavior, and intentional 410/501 behavior are
  unchanged.
- Data/migration impact: no database, migration source, canonical media, or podcast content was
  modified; ownership initialization only creates the explicitly configured empty podcast root
  and its marker, while onboarding probes leave no file behind.
- Security/data-loss impact: positive fail-closed correction; a local path replacement cannot
  redirect podcast-root creation, marker creation, or onboarding probe cleanup through a symlink
  into another tree on supported Unix targets. Unsupported targets retain conservative failure for
  the onboarding probe, while the existing platform-specific podcast behavior remains bounded.
- Performance/operational impact: one descriptor walk per configured root/probe and one bounded
  marker operation; no worker lease, download limit, polling interval, or API behavior changed.
- Documentation impact: Updated — this internal R5 report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; public documentation unchanged because this is internal safety
  behavior.
- Remaining risks and owner: the full R5.1 review still requires the real approximately-5-TB
  production topology and restore-target validation; release owner/operator owns that evidence.
- External input still required: approved isolated production snapshot/backup/canonical mapping
  and explicit authorization for the production migration and restore drills.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.
