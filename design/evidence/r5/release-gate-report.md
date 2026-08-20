# Release Gates A–I — current closure report

Status: **Partial**. This is a current-tree evidence report, not a release
approval or waiver.

Date: 2026-08-20  
HEAD: `2877e2c` (`Add R4 integration probe and associated tests`)  
Working tree: intentionally dirty with R4/R5 closure changes listed by `git status --short`  
Toolchain: Rust `1.97.1`, Docker `29.7.2`, BuildKit `0.32.2`, PostgreSQL `18.4-alpine`

## Commands and direct evidence

The following local gates passed from this working tree:

```text
./scripts/check-smoke.sh
./scripts/check.sh
SMOKE_IMAGE=melodee:r5-iterative-local RELEASE_VERSION=0.1.0 \
  RELEASE_REVISION=2877e2c-dirty SMOKE_RUN_ID=local-runtime-validator-20260820 \
  SMOKE_PORT=18102 ./scripts/container_smoke.sh
python3 scripts/validate_repository.py
python3 scripts/extract_requirements.py --check
cd docs && pnpm check && pnpm check:links && pnpm build
```

Failure and rerun history is part of this report. The first full-gate attempt after the local R5.1
hardening failed at
`apps/melodee-server/tests/discovery_worker.rs::scheduler_runs_periodically_and_stops_gracefully`
because a second no-op scheduler pass replaced the correction statistics returned by `stop()`.
The test was corrected to assert the observed durable repair and a completed pass; the exact test
then passed 10/10 consecutive runs. The final full gate passed with exit code 0. It included 174
Python tests, the Admin UI typecheck/lint/i18n/unit/build path, the
workspace Rust tests, and cargo-deny. Cargo-deny reported advisories, bans, licenses, and sources
as OK; duplicate transitive packages and an unmatched optional-license allowance remained
non-failing warnings.

The latest current-tree rerun after R4.6 path-containment hardening also passed with exit code 0:
it ran 177 Python tests, Admin UI 44 files/248 tests, all workspace Rust/PostgreSQL tests, and
cargo-deny. This rerun does not change any requirement to terminal or provide the missing external
R4/R5 evidence.

The subsequent current-tree rerun after R4.5 launcher readiness and path-containment hardening
also passed with exit code 0: it ran 178 Python tests, Admin UI 44 files/248 tests, all workspace
Rust/PostgreSQL tests, and cargo-deny. It likewise leaves the external R4 gates and all R5 entry
conditions open.

The latest current-tree rerun after machine-readable release-evidence validation also passed with
exit code 0: it ran 180 Python tests, Admin UI 44 files/248 tests, all workspace/PostgreSQL tests,
formatting, Clippy, and cargo-deny. Cargo-deny again reported advisories, bans, licenses, and
sources as OK; duplicate transitive packages and the unmatched optional-license allowance remain
non-failing warnings. This rerun does not provide a candidate-SHA, scheduled, arm64, production,
physical-client, ADR, or publication result.

The latest current-tree rerun after the operator-help and evidence updates also passed with exit
code 0: it ran 181 Python tests, Admin UI 44 files/248 tests, all workspace/PostgreSQL tests,
formatting, Clippy, and cargo-deny. The same duplicate-transitive-package and unmatched optional-
license warnings remained non-failing. This rerun does not provide a candidate-SHA, scheduled,
arm64, production, physical-client, ADR, or publication result.

The latest current-tree rerun after R4.1 source-backup validation also passed with exit code 0: it
ran 184 Python tests, Admin UI 44 files/248 tests, all workspace/PostgreSQL tests, formatting,
Clippy, and cargo-deny. The separate docs checks, link validation, and 42-page production build
also passed. This rerun does not provide a candidate-SHA, scheduled, arm64, production,
physical-client, ADR, or publication result.

The most recent current-tree rerun after release-evidence bundle path-containment hardening also
passed with exit code 0: it ran 186 Python tests, Admin UI 44 files/248 tests, all
workspace/PostgreSQL tests, formatting, Clippy, and cargo-deny. The focused release-evidence
suite passed 5/5, including root-runtime rejection, symlinked-root and symlinked-file rejection.
This rerun does not
provide a candidate-SHA, scheduled, arm64, production, physical-client, ADR, or publication
result.

The release-evidence validator was then corrected to validate the Dockerfile's named runtime
principal `melodee:melodee`; `scripts/container_smoke.sh` separately verifies its resolved UID/GID
10001. A local workflow-equivalent bundle was validated for each exact current-tree architecture,
including image metadata, archive, SBOM, vulnerability reports, scan status, schema-v1 manifest,
and SHA256SUMS. Both bundles passed validation and checksum verification. This remains local
dirty-tree evidence only.

The latest `./scripts/check.sh` rerun after this correction passed with exit code 0. It ran 195
Python tests, the Admin UI gate with 44 files and 248 unit tests, all workspace/PostgreSQL Rust
tests and doctests, formatting, workspace Clippy with warnings denied, and cargo-deny. The only
cargo-deny output remained the previously recorded non-failing duplicate-transitive-package and
unmatched optional-license warnings. This remains dirty-tree evidence and does not provide
candidate-SHA, scheduled, arm64 policy-pass, production, physical-client, ADR, or publication
evidence.

The latest current-tree rerun after migration lease-fencing hardening also passed with exit code
0. It ran 186 Python tests in 110.388 seconds, Admin UI 44 files/248 tests, all workspace and
PostgreSQL tests, formatting, Clippy, and cargo-deny. The migration/fault/freeze suite passed
22/22 after the final projection, excluded-summary, and verification-issue write fences were
added. This remains local dirty-tree evidence; it does not provide a candidate-SHA, scheduled,
arm64, production, physical-client, ADR, or publication result.

The latest current-tree rerun after CLI search-test environment isolation also passed with exit
code 0. It ran 194 Python tests, Admin UI 44 files/248 tests, all workspace and PostgreSQL Rust
tests and doctests, formatting, workspace Clippy with warnings denied, and cargo-deny. The
12-test legacy migration suite passed, including the shared-track multi-user state regression;
the focused search-root safety test passed ten consecutive runs. Cargo-deny again reported
advisories, bans, licenses, and sources as OK, with only the existing duplicate-transitive and
unmatched-optional-license warnings. This remains local dirty-tree evidence; it does not provide
candidate-SHA, scheduled, arm64, production, physical-client, ADR, or publication evidence.

The latest current-tree rerun after R4.4 differential comparator exactness hardening also passed
with exit code 0. It ran 187 Python tests, Admin UI 44 files/248 tests, all workspace and
PostgreSQL tests, formatting, Clippy, and cargo-deny. The focused differential-classification
suite passed 32/32 and the client-launcher safety suite passed 8/8. This remains local
dirty-tree evidence; it does not provide zero unexplained differentials, a candidate-SHA,
scheduled, arm64, production, physical-client, ADR, or publication result.

The release workflow was hardened after review found that its publication job rebuilt a
multi-platform image after scanning separate architecture images. The matrix now archives the
exact scanned images, includes `image.tar.gz` in the signed-by-checksum evidence bundle, verifies
and loads both archives in the publication job, pushes unique staging references only after the
quality/build/scan matrix succeeds, and creates final manifest tags from those staged digests
without rebuilding. `actionlint .github/workflows/release-container.yml` and the focused release
evidence/workflow tests passed. This is workflow/evidence preparation only; no registry was
contacted and no image was published.

The scheduled Full validation workflow is present at `.github/workflows/full-validation.yml`.
The latest observed run was
[32330365945](https://github.com/melodee-project/melodee-rs/actions/runs/32330365945) at current
HEAD `2877e2c`; it failed in
`apps/melodee-cli/tests/search.rs::search_status_rejects_overlap_with_an_inbound_source_without_leaking_paths`
because the test relied on the CLI's local `.env` staging value, which is absent on the runner,
so ingestion configuration failed before the intended search-root overlap check. The test now
sets isolated staging and normalization work-area roots explicitly and passes ten consecutive
local runs. The earlier run
[32214289862](https://github.com/melodee-project/melodee-rs/actions/runs/32214289862) at SHA
`207054be6fe37e84fdb32066ddae98987ce14bb2` remains failure history. These are not candidate-SHA
scheduled evidence; R5.4 remains Not started and no waiver is claimed.

The release workflow was additionally checked with `actionlint` and static Python assertions.
Its pre-publication job now builds, smoke-tests, and scans separate `linux/amd64` and
`linux/arm64` images, and publication depends on the complete architecture matrix. Each successful
matrix leg captures image identity/labels, an SPDX SBOM, JSON Trivy reports, scan status, a
schema-v1 `release-evidence.json` manifest, and a SHA256SUMS manifest as a required workflow
artifact; `scripts/validate_release_evidence.py` checks the bundle before and after checksum
creation, an explicit completeness step requires every file, and missing or mismatched artifact
output fails the matrix leg.
No GitHub Actions run, candidate artifact, or
publication is claimed from this local check.

The latest current-tree `./scripts/check.sh` rerun after the exact scanned-image archive and
publication changes also passed with exit code 0. It ran 187 Python tests, Admin UI 44
files/248 tests, all workspace/PostgreSQL Rust tests, formatting, Clippy, and cargo-deny.
`python3 -m unittest tests.python.test_release_evidence tests.python.test_container_smoke -v`
passed 11/11 and `actionlint .github/workflows/release-container.yml` passed. The publication
job no longer initializes QEMU because it loads and assembles already-built architecture images;
this is a workflow-only reduction and does not claim arm64 execution evidence.

The final current-tree `./scripts/check.sh` rerun on 2026-08-20 also passed with exit code 0. It ran
188 Python tests, Admin UI 44 files/248 tests, all workspace/PostgreSQL Rust tests, formatting,
Clippy, and cargo-deny; the local smoke gate passed separately with 116 server-library tests.
This remains dirty-tree evidence and does not provide candidate-SHA, scheduled, arm64, production,
physical-client, ADR, or publication evidence.

After the bounded R4.6 proxy/TLS header probe was added, the current-tree `./scripts/check.sh`
was rerun again on 2026-08-20 and passed with exit code 0. It ran 189 Python tests, Admin UI
44 files/248 tests, all workspace/PostgreSQL Rust tests, formatting, Clippy, and cargo-deny.
The local smoke gate also passed separately. This is still dirty-tree evidence and does not
provide candidate-SHA, scheduled, arm64, production, physical-client, ADR, or publication
evidence.

After the R4.2 disk-budget preflight and its fail-closed regression tests were added, the
current-tree `./scripts/check.sh` was rerun again on 2026-08-20 and passed with exit code 0.
It ran 192 Python tests, Admin UI 44 files/248 tests, all workspace/PostgreSQL Rust tests,
formatting, Clippy, and cargo-deny. The local smoke gate also passed separately. This remains
dirty-tree evidence and does not provide candidate-SHA, scheduled, arm64, production,
physical-client, ADR, or publication evidence.

The separate Admin UI Playwright gate was then run from the current tree with
`cd admin-ui && pnpm test:e2e`: 289 tests passed and 62 intentional tests were skipped, with zero
failures across accessibility, keyboard, functional, localization/RTL, responsive, and visual
coverage. The preceding focused desktop regression passed 12/12, and the three updated System
Workers visual baselines passed 3/3 after correcting stale health fixtures that omitted the
required podcast-storage, disk, and worker fields. The public documentation production gate also
passed from the current tree:
`cd docs && pnpm install --frozen-lockfile && pnpm check && pnpm check:links && pnpm build`;
Astro reported 0 errors/warnings/hints, all relative/internal links were valid, and 42 pages were
built. These are local dirty-tree R5.3/R5.4 preparation results, not candidate-SHA, scheduled,
physical-client, production, ADR, or publication evidence.

The UI/docs gate was repeated on the current tree after the subsequent local hardening. Playwright
scheduled 351 tests, passed all 289 executed tests, and reported 62 intentional configuration
skips with zero failures; the executed coverage included accessibility, keyboard, functional,
localization/RTL, responsive/tablet/mobile, and desktop visual checks. `cd docs && pnpm check`,
`pnpm check:links`, and `pnpm build` again passed with zero diagnostics, valid links, and 42 pages
built. This repeat remains dirty-tree evidence only and does not provide candidate-SHA,
scheduled-workflow, physical-client, production, ADR, or publication evidence.

## Remote workflow observations

Read-only inspection of the repository Actions history on 2026-08-19 found no scheduled or
candidate-SHA closure run. The current base commit `2877e2c8f8702f4ca98a2246ed4ce0b8cdcc1a4b`
has successful Docs and Security scanning runs, but CI smoke was cancelled and CodeQL failed at
SARIF upload for repository configuration, not source findings:

- Docs: <https://github.com/melodee-project/melodee-rs/actions/runs/32262883904> — success.
- Security scanning: <https://github.com/melodee-project/melodee-rs/actions/runs/32262883704> — success.
- CI smoke: <https://github.com/melodee-project/melodee-rs/actions/runs/32262883970> — cancelled.
- CodeQL: <https://github.com/melodee-project/melodee-rs/actions/runs/32262883668> — failed because
  GitHub reported that Advanced Security must be enabled to use code scanning; all three analysis
  jobs reached analysis/SARIF upload and no code finding was reported in the log.

The CodeQL failure is an external repository-admin blocker. Enabling Advanced Security requires
repository-owner authority and is not performed by this task. It is not a waiver, and R5.2/R5.4
remain Partial/Not started respectively until the owner enables the feature and the exact accepted
candidate SHA receives successful security and scheduled workflow evidence.

The historical local Trixie amd64 prototype is `melodee:r5-iterative-local`, image ID
`sha256:1a8551faf7b973aa78f6cbdcef3be7364eae07c98935a971ef977359e9eea552`. Its SPDX 2.3 SBOM
contains 299 packages. The historical local Trixie arm64 image is `melodee:r5-arm64-local`, image ID
`sha256:73ab8baaffaa79c4986873df0f6ce35ae770408f9fc41db9d297af2d88f3204b`, with an SPDX 2.3
SBOM containing 296 packages. The container smoke script passed for both. Trivy 0.72.0 reports
8 critical, 187 high, 203 medium, 183 low, and 87 unknown findings on each historical image, so
neither is release evidence under the current policy. The source Dockerfile now retains Debian
Bookworm until ADR-0151 is accepted; no current candidate image is claimed.

The same exact amd64 image was rerun through the disposable container smoke harness on
2026-08-20. It passed image-label/version checks, non-root runtime checks, ffmpeg/ffprobe/fpcalc
availability, startup/readiness, Admin SPA serving, CLI version output, bounded SIGTERM shutdown,
and disposable PostgreSQL cleanup. The run used `SMOKE_RUN_ID=local-current-20260820-0716` and did not
reuse or publish any Docker object. This is repeatable local evidence only; it does not change the
recorded critical/high vulnerability result or provide arm64, candidate-SHA, provenance, or
release-owner acceptance.

The post-fixture-isolation current-tree `./scripts/check.sh` rerun on 2026-08-20 completed with
exit code 0. It ran 193 Python tests, the Admin UI static gate (44 files and 248 unit tests), all
workspace/PostgreSQL Rust tests and doctests, formatting, workspace Clippy with warnings denied,
and cargo-deny. Cargo-deny again reported advisories, bans, licenses, and sources as OK; duplicate
transitive packages and the unmatched optional-license allowance remained non-failing warnings.
This is dirty-tree evidence only and does not provide candidate-SHA, scheduled, arm64, production,
physical-client, ADR, or publication evidence.

After the composite user-state import correction and existing-snapshot rehearsal, the current-tree
`./scripts/check.sh` was rerun on 2026-08-20 and completed with exit code 0. It ran 194 Python
tests, the Admin UI static gate (44 files and 248 unit tests), all workspace/PostgreSQL Rust tests
and doctests, formatting, workspace Clippy with warnings denied, and cargo-deny. The migration
regression and production-shaped bounded-batch suites passed inside the workspace run. Cargo-deny
again reported advisories, bans, licenses, and sources as OK; duplicate transitive packages and
the unmatched optional-license allowance remained non-failing warnings. This is dirty-tree
evidence only and does not provide candidate-SHA, scheduled, arm64, production, physical-client,
ADR, or publication evidence.

The subsequent local migration correction also passed its focused gates. The full CLI legacy
suite passed 11/11, the multi-user shared-track regression passed 1/1, and the existing-snapshot
rehearsal artifact passed with zero verification mismatch gaps, zero duplicate public IDs,
successful search rebuild and restore-content checks, source read-only facts, and unchanged
canonical media. This strengthens R4.2/R5.1 preparation only; it does not provide the real
approximately-5-TB rehearsal, external differential closure, or formal R5 entry.

Current requirement counts are 239 total: 211 `tested`, 9 `implemented`, 12 `in-progress`, and 7
`deferred`. No status was made terminal by this report. Current migrated differential reports are
committed per surface: native 192 unclassified differences, OpenSubsonic 330, and Jellyfin 17.

The final current-tree `./scripts/check.sh` rerun after expanding the documented operator-command
audit completed on 2026-08-20 with exit code 0. It ran 197 Python tests, the Admin UI static gate
(44 files and 248 unit tests), all workspace/PostgreSQL Rust tests and doctests, formatting,
workspace Clippy with warnings denied, and cargo-deny. A subsequent executable command-help audit
passed all 78 documented `melodee-cli` paths plus `legacy_rehearsal.py drill --help`; its focused
regression suite passed 2/2. Cargo-deny again reported advisories, bans, licenses, and sources as
OK; duplicate transitive packages and the unmatched optional-license allowance remained
non-failing warnings. This is dirty-tree evidence only and does not provide candidate-SHA,
scheduled, arm64 policy-pass, production, physical-client, ADR, or publication evidence.

The requirement override audit was then added to the smoke, full, and bootstrap gates. Its normal
mode passed with complete structural coverage for all 239 requirements: 211 tested, 9 implemented,
12 in-progress, and 7 deferred. Its strict `--require-terminal` mode failed closed on the named
12 in-progress requirements, as required for a truthful pre-release gate. The focused audit tests
passed 2/2, and the subsequent current-tree `./scripts/check.sh` passed with 200 Python tests,
the Admin UI gate, the full Rust/PostgreSQL and doctest matrix, formatting, Clippy, and cargo-deny.
This strengthens R5.5 evidence but does not make the non-terminal requirements or external gates
complete.

The published-release workflow now runs the strict terminal-status audit immediately after
`./scripts/check.sh` in its `quality-gate` job. A static regression assertion covers the workflow
step, and `actionlint` passed for the edited workflow. Because the current registry intentionally
contains 12 in-progress requirements, an attempted release currently stops before either the
amd64/arm64 build matrix or publication; this is fail-closed enforcement, not a waiver.

Partial-ingestion cleanup was then hardened to use descriptor-relative `NOFOLLOW` traversal and
unlink operations on supported Unix targets. Focused regressions passed 2/2 for an internal
symlink and a replaced root symlink; both preserve the outside sentinel. Suspicious partial trees
are retained rather than followed or removed. This closes a local R5.1 race/symlink gap without
changing the accepted ADR-0145 handoff boundary, but the real production-topology threat-model
review remains open.

The subsequent current-tree gate after this hardening passed with exit code 0. It ran 201 Python
tests, the Admin UI typecheck/lint/i18n/unit/build path, all workspace/PostgreSQL Rust tests and
doctests, formatting, workspace Clippy with warnings denied, and cargo-deny. This is repeatable
dirty-tree evidence only; it does not provide a candidate-SHA, scheduled, physical-client,
production-topology, ADR, or publication result.

Owned podcast orphan/failure/retention cleanup was then fenced to the validated root directory on
Unix: it rejects symlink and special-file children and uses descriptor-relative `unlinkat` for
regular files. The focused regression passed 1/1 and preserved the external sentinel. This is
local R5.1 hardening only; it does not replace the required production-topology review.

Podcast ownership initialization and onboarding writable probes were then fenced as well. On
supported Unix targets, podcast-root creation and marker inspection/creation walk absolute
components through `O_DIRECTORY|O_NOFOLLOW` descriptors, and onboarding creates and removes its
unique probe relative to one held parent descriptor. The podcast worker subset passed 4/4, the
onboarding subset passed 5/5, and server Clippy with `-D warnings` passed. This is local R5.1
hardening only; it does not provide production-topology, restore-target, or external integration
evidence.

Acquisition source copy and tree hashing were then changed to reopen regular files through the
descriptor-relative no-follow boundary after their metadata snapshots, while staging directories
and first-write files now use no-follow creation and exclusive-open semantics. The focused
regression `direct_copy_rejects_a_symlinked_source_at_open_time` passed, including the external
destination sentinel, the full `melodee-ingestion` all-features suite passed (32 library, 13
acquisition, 5 environment-fault, 5 inspection, 13 normalization, 14 publication, 1
publication-resilience, and 15 reconciliation tests, plus the documented performance test
remained ignored as an evidence harness), and ingestion Clippy with `-D warnings` passed. This
closes local check-to-open source-redirection and destination-overwrite gaps without changing the
migration source, canonical media, or public compatibility products; production-topology and
restore-target review remain external R5.1 requirements.

The destination side of that boundary is now implemented directly in `secure_fs.rs`: first-write
staging files use descriptor-relative `O_EXCL|O_NOFOLLOW` creation, and staging directories use
the verified no-follow directory walk. The focused source/destination symlink regression passed
1/1 with both external sentinels unchanged. The complete `cargo test -p melodee-ingestion
--all-features` suite passed 32 library, 13 acquisition, 5 environment-fault, 5 inspection, 13
normalization, 14 publication, 1 publication-resilience, and 15 reconciliation tests; the
documented performance harness remained ignored. Ingestion Clippy with `-D warnings` passed.
The current-tree `./scripts/check.sh` then passed with 201 Python tests, Admin UI typecheck,
lint, i18n, build, and 248 Admin UI unit tests, all workspace/PostgreSQL Rust tests and doctests,
formatting, workspace Clippy, and cargo-deny. This is local hardening evidence only: no source,
canonical media, production database, or external endpoint was touched, and the real topology and
restore-target review remain open.

The R5.1 audit then fenced CLI config export and unsupported filesystem fallbacks. CLI exports now
write a new file through a held no-follow parent descriptor and atomic no-replace rename; existing
destinations and symlink targets are rejected, with the focused CLI config suite passing 4/4.
Search, media, and ingestion secure filesystem fallbacks now fail closed when descriptor-relative
no-follow operations are unavailable, and search generation no longer performs a second path-based
permission rewrite after secure directory creation. Focused search/media/ingestion tests and
Clippy passed; this remains local hardening and does not provide production-topology or
restore-target evidence.

The media normalization boundary received the corresponding R5.1 hardening in the current tree.
`crates/melodee-media/src/secure_fs.rs` walks Unix directory components with
`O_DIRECTORY|O_NOFOLLOW`, opens source and output leaves without following symlinks, creates
retained-output copies with exclusive destination creation, and creates unique attempt directories
relative to verified parents. `hash_file`, Lofty metadata reads, artwork/sidecar preflight reads,
normalized-artwork verification, work-area creation, and output-parent creation now use that
boundary. Encoder destinations are rejected when already present and FFmpeg receives `-n`, so a
destination collision fails closed rather than overwriting a file. The focused tests
`media_filesystem_operations_fail_closed_on_symlink_components_and_leaves` and
`output_planning_rejects_symlinked_parent_and_existing_leaf` passed, including unchanged external
sentinels, and the source-opener regression also rejected a FIFO without blocking;
`cargo test -p melodee-media --all-features` passed 40 unit tests, 13 real-format tests,
and zero doctests (one pre-existing performance harness remains ignored), and media Clippy with
`-D warnings` passed. The subsequent current-tree `./scripts/check.sh` also passed with 201 Python
tests, the Admin UI static gate and 248 unit tests, the full workspace/PostgreSQL Rust and doctest
matrix, formatting, workspace Clippy with warnings denied, and cargo-deny. This is a local
source/staging safety improvement only: no public API,
compatibility product, migration source, target database, or canonical media was changed, and
the production-sized topology and restore-target review remain external R5.1 conditions.

The source-opener audit then added `O_NONBLOCK` plus an `fstat` regular-file check to the Unix
media and ingestion no-follow openers, preventing a raced FIFO or device from blocking a worker or
being consumed as source media. Retained media copies now sync the exclusive destination after
copy completion. The complete media and ingestion all-features suites and both crate Clippy
commands with `-D warnings` passed; no public contract, migration source, target database, or
canonical media was changed.

## Gate A — Data integrity

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Existing catalog counts reconcile within documented intentional transformations | Two independent 12,000-Track synthetic rehearsals restored matching 500 Artist / 1,000 Release / 12,000 Track / 50 user counts | Partial | Approved production snapshot and affected-count reconciliation |
| No unexplained missing Artists/Releases/Tracks | Local `legacy verify` reported zero mismatch gaps and zero duplicate public IDs | Partial | Real production-sized source and final differential classification |
| Public IDs preserved | Legacy import tests and synthetic verification preserve source public IDs | Partial | Real source comparison |
| User state preserved | Synthetic migration exercises user state; current differential still exposes play-count/state residuals | Partial | Release-owner compatibility classification and production snapshot evidence |
| Canonical media unchanged unless intentionally migrated | Two rehearsals report unchanged canonical 16-file / 16,000,000-byte tree | Partial | Read-only production-root rehearsal |

## Gate B — Ingestion resilience

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| 200+ Release scenario passes | Workspace acquisition, normalization, publication, and reconciliation suites pass their 200-release scenarios | Partial | Evidence from the release candidate SHA |
| Repeated kill/restart passes | Rust fault suites cover repeated termination and restart convergence; migration rehearsal crash/resume is covered synthetically | Partial | Production-sized rehearsal |
| No lost work | Fault suites and two synthetic migrations report no loss or duplicate IDs | Partial | Real topology and source-size evidence |
| No duplicate catalog inserts from retry | Import and ingestion tests prove idempotent retry and zero duplicate public IDs | Partial | Real source confirmation |
| No inbound candidate removal before verified durable staging receipt | Acquisition tests prove the ADR-0145 handoff boundary | Partial | Candidate-SHA and production-topology confirmation |
| No deletion of durable staged/canonical source without separately authorized decision, audit, and confirmation | Data-loss review and fault suites cover source retention and fail-closed removal | Partial | Operator review of real media and restore topology |

## Gate C — Native API/client

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Melodee API v1 differential suite passes | Current migrated replay ran all 12 native groups; 185 unclassified differences remain in `dual-run-report-native.json` after the current replay's narrow timestamp classifications; seven aggregate play-count fields remain visible and no disputed ADR-0150 mapping was added | Partial | Zero unexplained differences |
| Current Android phone and Android Auto smoke tests pass | Current-client checklist and launch preparation exist in `compatibility/clients/r4/`; no current migrated device run | Partial | Physical phone and Android Auto/DHU operator run |
| MeloAmp browse/search/playback/playlist/auth flows pass | Reproducible preparation exists; no current migrated MeloAmp run | Partial | Current MeloAmp binary/device and operator evidence |

## Gate D — OpenSubsonic

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Current compatibility matrix passes | Migrated replay ran 11 groups; 330 unclassified differences remain in `dual-run-report-opensubsonic.json` after the tested canonical-path projection correction, disabled-podcast phase coverage correction, and native-only share-fixture isolation | Partial | Zero unexplained differences and current matrix acceptance |
| Representative clients pass required workflows | Client matrix preparation exists; no current migrated client run | Partial | Representative current clients and operator evidence |

## Gate E — Jellyfin

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Current music compatibility route matrix passes | Migrated replay ran 8 groups; 17 unclassified differences remain in `dual-run-report-jellyfin.json` after tested media-container and primary-release-count corrections | Partial | Zero unexplained differences |
| Representative supported clients pass required workflows | Client matrix preparation exists; no current migrated Jellyfin-client run | Partial | Representative current client and operator evidence |

## Gate F — Search/performance

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Search p95 benchmark target passes | Synthetic migrated rebuild measured bounded queries at p95 under 350 ms and a default 100-hit limit | Partial | Production-sized migrated target measurements |
| API performance does not materially regress without approved exception | Full local performance suites pass; differential and production traffic evidence remain open | Partial | Candidate-SHA and production workload comparison |
| Large-catalog memory behavior remains bounded | 12,000-Track import/search RSS recorded; repeated local runs pass | Partial | Approximately 5-TB rehearsal and final headroom measurements |

## Gate G — Feature parity

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Every supported capability is Complete or has an approved ADR/release waiver | 211 requirements are tested and 7 are deferred, but 12 remain in-progress; ADR-0149/0150 are Proposed | Partial | Terminal truthful statuses and accepted decision-owner dispositions |

## Gate H — Operations

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Backup/restore tested | Synthetic target backup restored with matching catalog counts and content checks | Partial | Production backup and restore target |
| Migration/rollback tested | Synthetic freeze, failed-target retention, and rollback proof pass twice | Partial | Production-topology drill and operator authorization |
| Diagnostics usable | CLI/Admin diagnostics and full Rust/Admin UI suites pass | Partial | Release-candidate and deployed-topology checks |
| Search rebuild tested | Rebuild interruption/restart/lease/corruption tests and synthetic migrated rebuild pass | Partial | Production-sized migrated target and operator status path |
| Failed ingestion can be diagnosed/retried through Admin UI/CLI | Existing work-item diagnostics/retry suites pass | Partial | Candidate-SHA and production operational acceptance |

## Gate I — Security/FOSS

| PRD bullet | Local evidence | Status | Missing condition |
|---|---|---|---|
| Dependency license policy passes | `cargo deny check` licenses/sources pass; docs production audit passes | Partial | Final candidate scan |
| No required proprietary dependencies/services | Repository technology baseline and dependency boundary checks pass | Partial | Final candidate review |
| Security scanning/advisories pass agreed severity policy | Exact local amd64/arm64 image scans report 8 critical, 187 high, 203 medium, 183 low, and 87 unknown findings per image; no suppressions | Partial | Release-owner ADR-0151 disposition and a passing exact candidate scan |
| Secrets/logging review passes | Gitleaks clean-archive evidence, redaction tests, and diagnostic secrecy suites pass | Partial | Candidate-SHA secret scan and deployment review |

The local arm64 artifact retry then completed successfully using the cached BuildKit builder:
`docker buildx build --platform linux/arm64 --load --tag melodee:r5-arm64-local
--build-arg VERSION=0.1.0 --build-arg REVISION=2877e2c-dirty --progress=plain .` completed with
the release Rust build finishing in 36m08s. The resulting Linux arm64 image has ID
`sha256:73ab8baaffaa79c4986873df0f6ce35ae770408f9fc41db9d297af2d88f3204b`, and
`SMOKE_RUN_ID=local-arm64-20260820 SMOKE_PORT=18093 ./scripts/container_smoke.sh` passed the
non-root runtime, media-tool, labels, readiness, Admin SPA, CLI version, PostgreSQL, and bounded
SIGTERM checks. The exact images were scanned through a disposable Trivy 0.72.0 container using
Docker archive transfer rather than a host socket or bind mount. Both reports contain 8 critical,
187 high, 203 medium, 183 low, and 87 unknown findings. Syft 1.51.0 produced SPDX 2.3 SBOMs with
299 amd64 and 296 arm64 packages. The reports, SBOMs, and checksums are committed under
`design/evidence/r5/artifacts/local-20260820/`. This is local dirty-tree evidence only; it does
not provide a passing severity policy, candidate-SHA provenance, or release-owner acceptance.

The current native migrated replay was also rerun from this working tree with
`python3 scripts/run_r4_dual_run.py --only native --skip-build`. It completed the scenario set
and failed closed with 267 classified and 185 unclassified differences. The current report keeps
the catalog ordering/projection, reference-only metadata, user-state, and aggregate play-counter
residuals visible; no comparator mask or disputed ADR-0150 mapping was added.

An isolated local Gitleaks v8.30.0 scan also passed: a 175.22 MB archive containing the current
repository source, documentation, evidence, and untracked closure files (excluding only generated
build/dependency trees and `.git`) was scanned through a disposable container without a host bind
mount. It reported no leaks and exit code 0. This is dirty-tree preparation evidence only; the
candidate-SHA/GitHub security gate and deployment review remain open.

The existing-snapshot rehearsal was then repeated against a separately populated isolated source
without invoking the seed path. The second artifact,
`design/evidence/r4/rehearsal-drill-existing-snapshot-r4dbg-second.json`, records a passing
10-Artist/10-Release/19-Track/1-user run, zero verification mismatch gaps, zero duplicate public
IDs, a validated 549,673-byte source backup, restore counts matching the target, 20 canonical
files totaling 4,093,382 bytes unchanged with content hashes, and
`source_mutated_by_harness=false`. This improves repeatability evidence only; it is not the
required approximately-5-TB production rehearsal and does not change R4.2 from Partial.

The complete local gate was also run twice consecutively from the same current dirty worktree
after the rehearsal evidence update, and both runs exited 0. Each run covered 195 Python tests,
the Admin UI gate with 248 unit tests, the full workspace/PostgreSQL Rust and doctest matrix,
formatting, Clippy with warnings denied, and cargo-deny. A separate current-tree smoke-gate rerun
also exited 0. These are repeatability results only: R5.4 remains Not started until the accepted
candidate SHA has a successful scheduled/nightly workflow URL; no dispatch or publication was
performed.

The latest current-tree `./scripts/check.sh` rerun after the reproducible operator-command help
audit completed with exit code 0 on 2026-08-20. It ran 197 Python tests, the Admin UI static gate
(44 files and 248 unit tests), all workspace/PostgreSQL Rust tests and doctests, formatting,
workspace Clippy with warnings denied, and cargo-deny. The separate
`python3 scripts/check_command_help.py` audit passed all 11 documented `melodee-cli` paths and
`legacy_rehearsal.py drill --help`. This remains dirty-tree evidence only; it does not provide a
candidate-SHA, scheduled, arm64 policy-pass, production, physical-client, ADR, or publication
result.

The source runtime definition was then corrected to retain the accepted Debian Bookworm base
while ADR-0151 remains Proposed. A guarded local build using
`docker build --tag melodee:r5-bookworm-guarded-local --build-arg VERSION=0.1.0
--build-arg REVISION=2877e2c-dirty .` passed and produced image ID
`sha256:2eb48219e152cd959b96e897a71d0d6fa3ef98bc53aba99df9e4da64746b649a`. The disposable
`SMOKE_RUN_ID=local-bookworm-trace-20260820 SMOKE_PORT=18130 ./scripts/container_smoke.sh` run
passed the runtime-user, media-tool, labels, CLI, PostgreSQL-backed health/readiness, Admin SPA,
and graceful-shutdown checks. This is local Bookworm guard evidence only: it is not a release
candidate, does not supersede the historical Trixie decision-package scans, and has no publication,
provenance, or release-owner acceptance.

An archive-isolated Trivy 0.72.0 scan of that exact Bookworm image reported 11 critical, 193 high,
296 medium, 231 low, and 110 unknown findings; Syft produced an SPDX 2.3 SBOM with 301 packages.
The metadata, report, SBOM, and checksums are committed under
`design/evidence/r5/artifacts/local-20260820/` as the `bookworm-guarded-amd64-*` files. This
confirms the current Bookworm image also fails the current critical-finding policy. It remains
local decision evidence only, with no suppression, candidate status, provenance, or publication.

After those evidence updates, the exact current tree also passed `./scripts/check.sh` with 201
Python tests, the Admin UI typecheck/lint/i18n/build gate and 248 unit tests, the complete
PostgreSQL/workspace Rust and doctest matrix, formatting, workspace Clippy with warnings denied,
and cargo-deny. The pass remains local dirty-tree evidence and does not change any external gate
or ADR disposition.

The search-generation filesystem boundary was then tightened for R5.1. Root-sentinel and
generation-manifest reads, pending-manifest creation/removal, and manifest activation now reopen
their parent directories without following symlinks; activation uses a descriptor-relative atomic
rename followed by a directory sync. The search suite passed 3 library, 1 benchmark-smoke, 6
rebuild-integration, 28 search, and 0 doctest tests, including a parent-symlink regression that
preserved an external sentinel; search Clippy with `-D warnings` also passed. This is local
hardening evidence only. It does not replace the required real-topology review, production-sized
rebuild, or accepted release-candidate run.

The R4.6 integration probe was then hardened against endpoint redirection and backup path races.
Its HTTP opener no longer follows redirects for health, range, OIDC, podcast, Last.fm, or proxy/TLS
checks, URL validation rejects malformed ports before network access, and backup inspection holds
descriptor-relative no-follow path components through hashing and `pg_restore --list`, rejecting
FIFOs and other special files before reads. The focused suite passed 10/10, including redirect,
malformed-port, leaf-symlink, and FIFO regressions; the subsequent exact-tree `./scripts/check.sh`
passed with 203 Python tests, Admin UI typecheck/lint/i18n/build and 248 unit tests, the full
workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with warnings denied,
and cargo-deny. This is read-only local probe preparation; no production endpoint, credential,
provider account, backup, or restore target was contacted.

The follow-up topology correction now obtains canonical-root capacity, headroom, and device facts
from a held descriptor-relative no-follow directory, and compares the backup device using the
descriptor already validated by `pg_restore --list`, without reopening the canonical pathname.
The focused topology regression fails if `Path.stat()` is used for the result. This closes a local
evidence-assembly race; it does not provide production storage, restore, or deployment acceptance.

The current-tree surface differential refresh then reran all three isolated
migrated replays: native completed 12 groups and recorded 267 classified / 185
unclassified differences, OpenSubsonic completed 11 groups and recorded 318 /
330, and Jellyfin completed eight groups and recorded 149 / 17. Each command
exited 1 fail-closed because unexplained differences remain. The reports are
committed under `design/evidence/r4/dual-run-report-{native,opensubsonic,jellyfin}.json`;
no comparator rule, API contract, migration mapping, or ADR disposition was
changed by the refresh.

The settings config-file loader now uses a bounded descriptor-relative no-follow read on supported
Unix targets and rejects symlink or special-file replacement. The persistence regression suite
covers the rejected symlink case; valid JSON config-file precedence remains unchanged. This is a
local R5.1 safety correction and does not provide production-topology or restore-target evidence.

After that correction, the exact current-tree `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh`
run exited 0 against the dedicated local PostgreSQL service: 204 Python tests, 248 Admin UI unit
tests, the complete workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with
`-D warnings`, and cargo-deny passed. Credentials are intentionally represented by an ellipsis in
this report; no production database was contacted.

The CLI chart/config/script input paths now share the same bounded regular-file reader, with
descriptor-relative no-follow parent walks and a regression rejecting a symlinked config-import
input. This closes a local operator-input replacement race without changing valid input formats or
any compatibility surface; it does not provide production integration or restore evidence.

The podcast publication boundary was then tightened so temporary-file creation, no-replace
publication, and owned-file cleanup reuse the component-by-component no-follow descriptor for the
configured root. The focused podcast-worker subset passed 5/5, including an ancestor-symlink
regression that confirms no file is created through the linked parent. The exact current-tree
`MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` then exited 0 with 204 Python tests,
248 Admin UI unit tests, the full workspace/PostgreSQL Rust and doctest matrix, formatting,
workspace Clippy with warnings denied, and cargo-deny. This is local R5.1 hardening evidence only;
the real production topology and restore-target review remain open.

Canonical-media adoption validation was then moved from pathname metadata/canonicalization to a
held descriptor boundary. Relative parent components are opened without following symlinks, the
leaf is opened with `O_NOFOLLOW|O_NONBLOCK`, and `fstat` admits only regular files before size
comparison; absolute, traversal, symlink, and special-file paths become `media.unadoptable`.
The focused CLI legacy suite passed 12/12 and CLI Clippy passed with warnings denied. The exact
post-change `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 204
Python tests, 248 Admin UI unit tests, the complete workspace/PostgreSQL Rust and doctest matrix,
formatting, workspace Clippy with warnings denied, and cargo-deny. No production media or
migration target was accessed.

The exact full gate was then rerun against a fresh isolated PostgreSQL container after the
repository development container's `/dev/shm` reached 100% from stale DSM files. The isolated
run exited 0: 204 Python tests, Admin UI static checks with 44 files and 248 unit tests, the
complete workspace/PostgreSQL Rust and doctest matrix, including all 18 Jellyfin media tests,
`cargo fmt --check`, workspace Clippy with `-D warnings`, and cargo-deny passed. The preceding
17 Jellyfin failures were PostgreSQL shared-memory allocation failures, not behavioral assertion
failures; the affected suite then passed 18/18 with the isolated database. This remains local
dirty-tree evidence and does not provide candidate-SHA, scheduled, production, physical-client,
ADR, or publication evidence.

## Release decision

Gates A–I are not all passing. No release waiver is claimed. R4 remains formally active; R5 and
R6 remain blocked by the external conditions above, ADR-0149/0150/0151 dispositions, the current
differential residuals, and the unavailable physical-client, production-topology, scheduled-run,
candidate multi-architecture scan/provenance, and release-owner acceptance evidence.
