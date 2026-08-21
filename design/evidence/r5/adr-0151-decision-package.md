# ADR-0151 decision package — release runtime base

Status: Decision required. ADR-0151 remains **Proposed** and has not been
accepted by this work.

Date: 2026-08-19  
Decision owner: Release owner  
Working tree: `2877e2c` plus uncommitted release-closure changes

## Current candidate evidence

The Dockerfile currently retains the accepted Debian Bookworm runtime because
ADR-0151 is still Proposed. A Debian Trixie amd64 image was built earlier as
isolated decision-package triage only; it was not published and was never a
release candidate:

```text
docker build --tag melodee:r5-iterative-local \
  --build-arg VERSION=0.1.0 \
  --build-arg REVISION=2877e2c-dirty .
```

Image content ID:

```text
sha256:1a8551faf7b973aa78f6cbdcef3be7364eae07c98935a971ef977359e9eea552
```

Trivy artifact digest: `sha256:f270191810618e5ebbdf315503358e75a5b02ed98849ad8ab72ae416f26ea547`.

The release-workflow-equivalent local smoke passed for UID/GID 10001, CLI
version, FFmpeg, FFprobe, fpcalc, required OCI labels, PostgreSQL-backed
health/readiness, the Admin SPA, and graceful SIGTERM. The runtime reports
That historical prototype reported Debian Trixie 13.6, FFmpeg
`7.1.5-0+deb13u1`, and Chromaprint `1.5.1-7`.

Syft 1.51.0 generated an SPDX 2.3 SBOM containing 299 packages. It is a
temporary local triage result with no signed provenance or release attestation.

Trivy 0.72.0 scanned the exact amd64 image with unfixed findings included:

| Severity | Count |
|---|---:|
| Critical | 8 |
| High | 187 |
| Medium | 203 |
| Low | 183 |
| Unknown | 87 |

The eight critical findings are all runtime-package findings and currently
have no fixed version in the scanner result:

| Package | Installed version | Vulnerability IDs |
|---|---|---|
| `libglib2.0-0t64` | `2.84.4-3~deb13u3` | CVE-2026-58016 |
| `libmbedcrypto16` | `3.6.5-0.1~deb13u1` | CVE-2026-34873, CVE-2026-34875 |
| `libxml2` | `2.12.7+dfsg+really2.9.14-2.1+deb13u3` | CVE-2026-6653 |
| `perl-base` | `5.40.1-6` | CVE-2026-13221, CVE-2026-42496, CVE-2026-57433, CVE-2026-8376 |

No vulnerability was suppressed or masked. The image therefore fails the
release workflow's critical-finding policy and cannot be accepted as the RC.

The exact arm64 image was then scanned through the same Trivy 0.72.0 container
and vulnerability database. It reported 8 critical, 187 high, 203 medium, 183
low, and 87 unknown findings. Syft 1.51.0 generated SPDX 2.3 SBOMs containing
299 amd64 packages and 296 arm64 packages. The complete machine-readable
reports and SBOMs, with SHA256 checksums, are committed under
`artifacts/local-20260820/`:

| Architecture | Image ID | Trivy report SHA256 | SBOM SHA256 | Packages |
|---|---|---|---|---:|
| amd64 | `sha256:1a8551faf7b973aa78f6cbdcef3be7364eae07c98935a971ef977359e9eea552` | `a902f344c84cb43e468232e67a702cfeb703b64999d2d3bf7ae66bebf0a4255a` | `d06d8553673057e4c09cd2690b98108250e52e9ee2c02e0d169bb77452aa7f81` | 299 |
| arm64 | `sha256:73ab8baaffaa79c4986873df0f6ce35ae770408f9fc41db9d297af2d88f3204b` | `9a0d3dbc232893edc1c017fe2dcaf0d7190303d752eb520510bd755fcabd52af` | `82291d6493ac3681c3fe48bfa19d7820eecf50b4ea841ec70ad8547d6f452c65` | 296 |

## Decision alternatives

1. Accept ADR-0151 after the exact release candidate is rebuilt and scanned,
   both architectures pass, the media/client matrix passes, and the release
   owner accepts the residual severity policy. This preserves Debian's FOSS
   package model and the existing runtime contract.
2. Hold the Trixie change and remediate the Bookworm package findings when
   Debian publishes fixed versions. This leaves the current image blocked and
   does not provide a release path today.
3. Select another runtime distribution or independently build media tools.
   This requires a new or superseding ADR plus a separate architecture,
   licensing, media, and client-compatibility plan.

Suppressing the findings is not an available alternative: the workflow
explicitly fails critical findings and the repository rules prohibit hiding
security blockers without decision-owner authority.

## Disposition requested

The release owner must explicitly accept, reject, or revise ADR-0151. Acceptance
must name the final runtime base and severity policy and authorize the required
amd64/arm64 candidate scans. Until then, this package is evidence for the
decision only; it does not change ADR status, versioning, publication, or
release eligibility.

## Guarded current-source verification

On 2026-08-20 the source Dockerfile was restored to the accepted Debian Bookworm
runtime while this ADR remains Proposed. The guarded local command

```text
docker build --tag melodee:r5-bookworm-guarded-local \
  --build-arg VERSION=0.1.0 \
  --build-arg REVISION=2877e2c-dirty .
```

passed with image ID
`sha256:2eb48219e152cd959b96e897a71d0d6fa3ef98bc53aba99df9e4da64746b649a`.
The disposable `scripts/container_smoke.sh` run passed the non-root runtime,
media tools, labels, CLI version, PostgreSQL-backed health/readiness, Admin SPA,
and graceful shutdown checks. This confirms the accepted source baseline remains
buildable. An archive-isolated Trivy 0.72.0 scan of the exact image reported 11
critical, 193 high, 296 medium, 231 low, and 110 unknown findings; Syft produced
an SPDX 2.3 SBOM with 301 packages. The machine-readable metadata, scan, SBOM,
and checksums are under `design/evidence/r5/artifacts/local-20260820/` with the
`bookworm-guarded-amd64-*` names. This is not an RC, was not published, and has
no provenance; no findings were suppressed.

## Remaining external evidence

Even after the ADR decision, the following remain required: final candidate
SBOM/provenance, scheduled candidate-SHA gates,
and current physical-client and production-topology acceptance. No production
endpoint, registry, tag, or GitHub Release was contacted by this work.

---

> **Disposition (2026-08-21):** ADR-0151 was **Accepted** by the release owner in
> commit `c8cfbd0`, which changed the `Dockerfile` runtime stage to
> `debian:trixie-slim` per the accepted decision. The scan findings recorded
> above (8 critical / 187 high on the prototype, all unfixed runtime-package
> findings) remain open release blockers: the release workflow still fails on
> any critical finding (`ignore-unfixed: false`), and the accepted-ADR follow-up
> work — exact amd64/arm64 candidate rebuild, smoke, scan, SBOM, and provenance
> at the release commit — remains R5.2/R5.6 scope. The text above is the
> historical decision record and is preserved as written.
