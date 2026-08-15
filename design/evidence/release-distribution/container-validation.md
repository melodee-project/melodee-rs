# Container release-distribution validation

This evidence records the local pre-publication validation of the production
image defined by the container-only release policy in ADR-0144. It is not a
published GHCR image and does not represent a GitHub Release.

## Build

- Date: 2026-08-15
- Source revision used as the build label: `55fc3eb651e97f9d411f1af4559356799aa948b2`
- Local image: `melodee:release-distribution-local`
- Build arguments: `VERSION=0.1.0`, the source revision above, and a UTC OCI
  creation timestamp
- Runtime base: `debian:bookworm-slim`
- Local runtime architecture: `linux/amd64`

The image was built from a clean Docker build context. The multi-platform
release configuration is validated statically in
`.github/workflows/release-container.yml`; its `linux/arm64` image cannot be
runtime-tested on this amd64 workstation without a release publication.

## Runtime contents

The image runs as UID/GID `10001` (`melodee`) and contains:

| Component | Observed version |
| --- | --- |
| Melodee server/CLI package | `0.1.0` |
| FFmpeg | `5.1.9-0+deb12u1` |
| FFprobe | `5.1.9-0+deb12u1` |
| fpcalc / Chromaprint | `1.5.1` |

The inspected image carried the required OCI title, description, source,
revision, version, creation-time, and MIT license labels. It contains neither
`/src`, Rust toolchain directories, nor a copied `.env` file.

## Integration smoke test

Against an ephemeral PostgreSQL 18.4 container, the production image:

1. applied its migrations and reached both `/healthz` and `/readyz`;
2. served the compiled admin SPA at `/admin/`;
3. exposed `ffmpeg`, `ffprobe`, `fpcalc`, and `melodee-cli --version`;
4. exited successfully on Docker `SIGTERM`; and
5. rejected writes through a read-only inbound bind mount.

The official `compose.yml` was also started with the same image retagged only
for local use. Its PostgreSQL, storage-initialization, and Melodee services
became ready; the inbound bind was read-only; and a marker in the named work
volume persisted after recreating the Melodee service. All temporary
containers, networks, volumes, and bind directories were removed after the
test.

The documented PostgreSQL backup validation was also exercised against the
Compose PostgreSQL service: `pg_dump --format=custom` produced an archive and
`pg_restore --list` parsed it successfully before the disposable service and
archive were removed.

## Release workflow boundary

`release-container.yml` uses the published GitHub Release event only. Before
GHCR publication it runs `./scripts/check.sh`, builds and tests the native
amd64 image, scans it with Trivy, and validates required OCI labels. The final
Buildx publish step creates one `linux/amd64`/`linux/arm64` manifest with SBOM
metadata and a GitHub OIDC-backed attestation. No image was pushed during this
local validation.
