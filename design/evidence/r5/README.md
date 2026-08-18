# R5 — Release-candidate hardening and complete gate

Status: Formal R5 entry is not started because R4 remains the active closure phase. Local
preparatory correctness, repository, UI, dependency, migration, container-configuration, and
documentation-adjacent checks pass from the current working tree. External scheduled-run, scanner,
physical-client, production-provider, and release-owner gates remain open.

Date: 2026-08-18

## Scope and authority

R5 is the release-candidate hardening portion of the release-closure plan. Its authority is the
PRD Release Gates A–I, the repository Definition of Done, ADR-0144, and the R5 work packages in
`IMPLMENTATION_OUTSTANDING.md` §10. R4 remains the active phase because its production-shaped snapshot,
physical-client, provider, traffic-switch, and rollback acceptance legs are environment-owned.

No version, tag, image, release, or public compatibility contract was changed. R6 therefore
remains not started.

## Local gate evidence

The following checks passed on 2026-08-18:

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
cd docs && pnpm install --frozen-lockfile && pnpm check && pnpm check:links && pnpm build
```

The focused migration/freeze suite passed 14/14 tests. The full gate passed repository and
requirements validation, 140 Python tests, Admin UI typecheck/lint/i18n/unit/build (248 unit
tests), workspace formatting, Clippy, the complete workspace test suite including PostgreSQL
integration, and dependency checks. The Admin UI lint reported two existing non-fatal Fast Refresh
warnings; it reported zero errors. `cargo deny check` reported advisories, bans, licenses, and
sources as OK; duplicate transitive packages and an unmatched optional license allowance remain
warnings already accepted by the repository gate.

The Compose checks used synthetic values from `compose.env.example`; they did not contact a
deployment or create persistent volumes. The smoke and full gates were rerun after the proposed
runtime-base change, and the docs site built 42 pages with link validation passing.

The Compose configuration was also validated with the required variables supplied as synthetic
local values:

```text
docker compose -f compose.dev.yaml config
docker compose config
```

The production-shaped R4 rehearsal remains the source for migration/rollback measurements:
`design/evidence/r4/rehearsal-drill-12000.json` and
`design/evidence/r4/dual-run-report.json`.

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
| Inbound acquisition cleanup | Direct-child containment, source generation and tree-hash fences, durable receipt, current lease, directory sync, and delayed path-reuse tests in `acquisition.rs` |
| Partial staging cleanup | Worker-owned direct child of the staging `releases/` root; unexpected paths fail closed |
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

The exact-candidate container scans are still open. A current local prototype was built and
smoke-tested after proposing ADR-0151:

```text
docker build --tag melodee:r5-trixie-local \
  --build-arg VERSION=0.1.0 \
  --build-arg REVISION=<current-head> .
```

The prototype uses Debian Trixie 13.6, FFmpeg `7.1.5-0+deb13u1`, and Chromaprint `1.5.1-7`.
It passed the release workflow's local image smoke path: UID/GID 10001, CLI version, FFmpeg,
FFprobe, fpcalc, PostgreSQL-backed health/readiness, Admin SPA, and graceful SIGTERM. Trivy
0.72.0 was used to scan the exact saved prototype
image and reported 8 critical and 160 high findings. This is an improvement over the stale
Bookworm image scan, but it still fails the release workflow's critical-finding policy and is not
release evidence. The unresolved findings require package/vendor triage and release-owner
acceptance; no suppression was added.

For the same saved image tar, Syft 1.51.0 generated an SPDX 2.3 SBOM containing 299 packages.
The local image content ID was `sha256:7236faeda9024f7adef47736d16fbaf5c59976c6ca96e0dabbedbb00b6f710d5`.
The SBOM is a temporary triage artifact and has no release provenance or signed attestation.

ADR-0151 proposes the Trixie runtime-base change because the current Bookworm image had 11
critical and 185 high findings. The proposal remains unaccepted until the candidate scan,
media/client compatibility, both architecture builds, SBOM, and provenance requirements pass.

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

An arm64 `docker buildx build --platform linux/arm64 --load` attempt was run
with a 1,800-second timeout. It reached the complete `melodee-server` release
compile and began `melodee-cli` compilation without an architecture-specific
error, but the emulated build was canceled at the timeout before an image was
loaded. No arm64 digest or scan result is claimed. A native arm64 builder is
required for an efficient reproducible two-architecture release build.

## R5.3–R5.5 — Correctness, flake, and requirement closure

The full local gate completed without failures, including the long-running 200-release,
restart/recovery, production-shaped catalog-query, media normalization, publication, search,
native, OpenSubsonic, Jellyfin, and Admin API suites. No correctness test was changed to ignored
or bypassed as part of this work. The current native differential replay is documented in the R4
report with 257 classified and 213 unclassified differences; the unclassified set still requires
external compatibility classification or implementation fixes and is not silently waived. The
earlier OpenSubsonic replay recorded 389 differences. Proposed ADR-0150 records the remaining
legacy media metadata boundary.

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
requires the release-owner authorization specified by the plan and ADR-0144.

## Remaining R5/R6 acceptance

- Repeat the R4 rehearsal against the real production-shaped snapshot and restore environment.
- Complete the external differential classification and physical native/Android Auto/MeloAmp/
  OpenSubsonic/Jellyfin client matrix.
- Run scheduled/nightly checks from the candidate SHA.
- Run Trivy, Gitleaks, Syft, image smoke, provenance, and vulnerability gates.
- Complete a native arm64 build and scan; the local QEMU attempt reached the
  application crates but timed out before producing an image.
- Complete production OIDC, Last.fm, SMTP, podcast, proxy/TLS, storage, backup/restore, and
  operator cutover/rollback evidence.
- Obtain release-owner acceptance before R6 versioning, tagging, image publication, or GitHub
  release execution.

Documentation impact:

- Updated — this internal R5 evidence report, `design/evidence/r4/README.md`,
  `IMPLMENTATION_OUTSTANDING.md`, and the public migration guide
  `docs/src/content/docs/installation/legacy-migration.md`.
