# ADR-0144: Publish official releases as GHCR OCI images

- **Date:** 2026-08-15
- **Status:** Accepted
- **Decision owners:** Melodee project owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-DEPLOY-001, PRD-DEPLOY-002, PRD-DEPLOY-003,
  PRD-DEPLOY-004, PRD-DEPLOY-005, PRD-DEPLOY-006, PRD-PRINCIPLE-007

## Context

Melodee is a self-hosted server whose production behavior depends on its Rust executables, the
admin SPA, PostgreSQL, and the FFmpeg, ffprobe, and Chromaprint `fpcalc` tools required by
ADR-0108. Distributing host-native executables would create a separate production support matrix
for each operating system, architecture, C runtime, external-tool package, and installer format.
That matrix is neither the product's deployment model nor a reliable way to validate the complete
runtime package.

The owner has approved a single official runtime-distribution model. This decision does not remove
the open-source source-build workflow: developers and advanced users may still compile a native
executable for their own environment.

## Decision

1. Official Melodee runtime releases are OCI container images published only to
   `ghcr.io/melodee-project/melodee`.
2. Official image platforms are `linux/amd64` and `linux/arm64`. Other platforms remain
   community/source-build territory unless a later ADR expands the tested matrix.
3. A GitHub Release remains the official version and release-note record. GitHub-generated source
   archives remain available with the Release, but it receives no deployable native artifacts.
4. The project does **not** publish Rust executables, ZIP/TAR native bundles, Windows installers,
   macOS packages, `.deb`/`.rpm` packages, AppImages, Homebrew artifacts, or any other
   precompiled native package to GitHub Releases.
5. Users who require a native executable clone the repository and build it from source for their
   own target. This workflow is supported for development and community use, but is not an
   officially tested production release-distribution artifact.
6. The official release workflow triggers only when a GitHub Release is published. It runs the
   full repository gate, builds and smoke-tests the image, scans it, then publishes the signed
   release provenance and multi-platform image. Ordinary pushes, pull requests, tag pushes, and
   schedules do not publish an image.
7. Stable releases publish immutable `X.Y.Z`, floating `X.Y`, and `latest` tags. A GitHub
   prerelease publishes only its exact prerelease tag and never moves `latest` or a stable floating
   tag. The GitHub Release tag is authoritative and must match the server and CLI Cargo versions.
8. The image is OCI-compatible and is documented for FOSS Docker Engine/Moby plus
   Docker Compose-compatible tooling on Linux. Docker Desktop is not a Melodee requirement.
9. The production image is a non-root, multi-stage package containing the server, administrative
   CLI, admin SPA, and supported FOSS media tools. It carries OCI metadata, an SPDX SBOM, and a
   keyless GitHub OIDC build-provenance attestation.

## Alternatives considered

### Attach per-platform native artifacts to GitHub Releases

Rejected. This would multiply the supported production matrix and leave operators responsible for
matching system media-tool packages and runtime libraries. It also makes it harder to validate the
same complete artifact that users deploy.

### Make Docker Desktop the supported host requirement

Rejected. The runtime is an OCI image and runs with compatible FOSS Linux container engines. A
desktop product is neither necessary nor suitable as a server deployment requirement.

### Publish images on every main commit or tag push

Rejected. An official deployable artifact must correspond to the release record and pass the
release-quality gates. Preview builds would blur that boundary and require broader package-write
credentials in ordinary CI.

## Rationale

One OCI image keeps the executable, admin bundle, and media-runtime dependencies together in a
reproducible, testable package. It makes two Linux architectures a deliberate and visible support
commitment while keeping native compilation accessible to the FOSS community. GHCR integrates with
the repository's release record, `GITHUB_TOKEN`, provenance attestations, and image SBOM support
without a proprietary registry or long-lived publishing secret.

## Consequences

### Positive

- Operators deploy a single, coherent runtime package through a familiar Compose reference.
- The release workflow validates the same production image form it publishes.
- Image labels, SBOM, vulnerability scanning, and provenance are associated with the deployable
  artifact rather than a detached native archive.
- Historical note: ADR-0145 later changes inbound mount permission. Inbound media bytes remain
  immutable, but the runtime must be able to remove successfully acquired candidate directories;
  PostgreSQL, staging, canonical storage, work, and search
  remain explicit persistent paths.

### Negative / trade-offs

- The official production matrix is limited to Linux containers on amd64 and arm64.
- The first GHCR publication may require an organization administrator to make the package public
  and confirm that it is linked to this repository; workflow YAML cannot safely force that policy.
- OS package versions for FFmpeg, ffprobe, and fpcalc follow the selected supported Debian base
  image (`ffmpeg` and `libchromaprint-tools`) rather than a developer workstation. Release workflow
  logs record the actual versions.

### Follow-up work

- Phase 11 migration, cutover, and rollback rehearsal remains independent and is still required
  before replacing a production legacy installation.
- Re-evaluate supported image platforms, base distribution, or native artifact policy only through
  a new ADR and a documented release-matrix change.

## Validation

- `Dockerfile` builds server/CLI/admin assets in separate build stages and runs as UID 10001.
- `compose.yml` uses `ghcr.io/melodee-project/melodee`, mounts inbound media writable only for
  verified candidate handoff cleanup under ADR-0145, and makes
  owned persistent paths explicit.
- `.github/workflows/release-container.yml` is release-published-only, validates SemVer/Cargo
  versions, runs `make check`, smoke-tests a PostgreSQL-backed image, scans it, builds a
  linux/amd64+linux/arm64 manifest, publishes an SPDX SBOM, and adds GitHub provenance.
- Public documentation describes image tags, upgrades, source builds, attestation verification,
  persistent state, and the first-time GHCR package-visibility requirement.

## References

- [PRD.md](../../PRD.md) §54
- [SPEC.md](../../SPEC.md) §6 and §27
- [ADR-0103](ADR-0103-require-fully-foss-dependencies.md)
- [ADR-0104](ADR-0104-start-as-a-modular-monolith.md)
- [ADR-0108](ADR-0108-use-ffmpeg-ffprobe-and-chromaprint-tools.md)
- [Docker deployment documentation](../../docs/src/content/docs/installation/docker.md)
