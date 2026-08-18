# ADR-0151: Use Debian Trixie runtime base for release images

- **Date:** 2026-08-18
- **Status:** Proposed
- **Decision owners:** Release owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-DEPLOY-001, PRD-DEPLOY-002, PRD-DEPLOY-003, PRD-DEPLOY-004,
  PRD-DEPLOY-005, PRD-DEPLOY-006, PRD-PRINCIPLE-007

## Context

The release image currently uses `debian:bookworm-slim` and installs the Debian-packaged
`ffmpeg`, `ffprobe`, and `libchromaprint-tools` required by ADR-0108. A current Trivy scan of the
image built from the release-closure worktree reports 11 critical and 185 high findings in the
Bookworm runtime, including findings with no fixed version in the selected repository. The image
cannot pass the release workflow's critical-finding gate in that state.

Debian Trixie is the current stable Debian release in the local release environment and provides
the same named FOSS media packages. Changing the runtime base is a release-distribution decision,
not an application dependency change, so it must be explicit and validated against the image
smoke contract and supported media tools.

## Decision

Use `debian:trixie-slim` as the final runtime stage base in `Dockerfile`. Continue installing the
same named packages (`ca-certificates`, `curl`, `ffmpeg`, `libchromaprint-tools`, and `tzdata`)
through Debian's package repositories, keep the non-root UID/GID 10001 contract, and retain the
same OCI labels, entrypoint, healthcheck, exposed port, and persistent-path behavior.

The builder stages and application source contracts do not change. The release workflow must scan
the exact candidate image, and both amd64 and arm64 builds must pass the existing tool, startup,
readiness, Admin SPA, and graceful-shutdown smoke checks before this ADR can be accepted.

## Alternatives considered

### Alternative A — Keep Bookworm and suppress findings

Rejected. Suppression would conceal an image-level release blocker and would not satisfy the
workflow's critical-vulnerability policy.

### Alternative B — Build media tools independently

Rejected for this change. Replacing Debian's FOSS packages with a separately maintained media
toolchain would expand the supply-chain and architecture matrix without evidence that it reduces
the image's vulnerability surface.

### Alternative C — Use another runtime distribution

Deferred. Alpine or a distroless image would require a separate compatibility and operational
decision for FFmpeg, Chromaprint, certificates, diagnostics, and libc behavior.

## Rationale

Trixie keeps the accepted Debian-package model and the existing runtime contract while moving the
image to the current stable security baseline. It is the smallest distribution-level change that
can be tested with the current release workflow and does not alter application wire contracts.

## Consequences

### Positive

- The official image runtime distribution changes from Debian Bookworm to Debian Trixie.
- Debian package versions, FFmpeg behavior, and image vulnerability results must be recorded for
  each release candidate.

### Negative / trade-offs

- A release-owner acceptance is required before publication; this proposal does not authorize a
  release, tag, or GHCR push.
- Trixie's FFmpeg/Chromaprint versions must be checked against the accepted media matrix and
  representative client/transcoding evidence.

### Follow-up work

- Build and smoke-test the amd64 candidate locally and the amd64/arm64 candidates in the release
  workflow.
- Run Trivy against the exact candidate image with the release workflow's critical/high policy.
- Record the image digest, package versions, scan result, SBOM, and provenance at R5/R6 release
  closure.

## Validation

The local prototype must pass the release image smoke path: UID 10001, CLI version, FFmpeg,
FFprobe, fpcalc, PostgreSQL-backed health/readiness, Admin SPA, and graceful SIGTERM. The release
candidate must additionally pass both architecture builds, the full repository gate, image scan,
SBOM, provenance, and the accepted media/client gates.

## References

- [PRD.md](../../PRD.md) §54
- [SPEC.md](../../SPEC.md) §6 and §27
- [ADR-0144: Publish official releases as GHCR OCI images](ADR-0144-publish-official-releases-as-ghcr-oci-images.md)
- [ADR-0108: Use FFmpeg, ffprobe, and Chromaprint tools](ADR-0108-use-ffmpeg-ffprobe-and-chromaprint-tools.md)
