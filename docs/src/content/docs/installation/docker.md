---
title: Container deployment
description: Deploy Melodee with the official GHCR image and Docker Compose-compatible tooling.
sidebar:
  order: 1
---

Melodee's official production artifact is the OCI image:

```text
ghcr.io/melodee-project/melodee
```

Official images support Linux `amd64` and `arm64`. They work with FOSS Docker Engine/Moby and
Docker Compose-compatible tooling on Linux; Docker Desktop is not required. GitHub Releases are
the version and release-note record. They do **not** contain precompiled native executables,
installers, or binary archives. See [Build from source](./build-from-source/) if you specifically
need a host-native executable.

## Quick deployment with Compose

The repository's [`compose.yml`](../../../../../compose.yml) is the reference deployment. It uses
the released GHCR image, PostgreSQL 18, named volumes for Melodee-owned state, and a read-only bind
mount for inbound media.

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs
cp compose.env.example .env
# Edit .env: set distinct random secrets, a public origin, and an absolute inbound path.
docker compose pull
docker compose up -d
docker compose ps
```

Use `openssl rand -base64 48` for both Melodee secrets. Do not commit `.env`. If the PostgreSQL
password contains reserved URL characters, percent-encode it in `MELODEE_DATABASE_URL`.

The Compose example binds the HTTP port only to `127.0.0.1` by default. Put a TLS-terminating
reverse proxy in front of it and set `MELODEE__NATIVE__PUBLIC_ORIGIN` to that public HTTPS origin.
Only set `MELODEE_BIND_ADDRESS=0.0.0.0` for a deliberately firewalled direct deployment.

After startup, verify the server and create the first administrator:

```bash
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
docker compose exec -e MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD='a-long-unique-password' \
  melodee melodee-cli users bootstrap-admin --username admin --email admin@example.test
```

The password is supplied through the process environment rather than a command-line argument.
Open `https://your-public-origin/admin` and finish the server-side onboarding checks.

## Image tags

| Tag | Meaning | Use |
| --- | --- | --- |
| `X.Y.Z` | Exact released version. | Recommended production pin. |
| `X.Y` | Most recent stable patch in that minor line. | Deliberate minor-line tracking. |
| `latest` | Most recent stable release. | Evaluation only unless you intentionally accept automatic upgrades. |
| `X.Y.Z-rc.N` | Exact prerelease. | Testing; it never updates `latest` or `X.Y`. |

For reproducible production deployments, pin `X.Y.Z` or the published image digest rather than
`latest`. Set `MELODEE_IMAGE_TAG=X.Y.Z` in `.env` before `docker compose pull && docker compose
up -d`.

## Persistent state and mounts

The example declares these paths intentionally:

| State | Compose location | Notes |
| --- | --- | --- |
| PostgreSQL | `melodee_postgres` | Authoritative catalog, identity, users, settings, templates, themes, user images, and audit state. |
| Inbound media | `${MELODEE_INBOUND_PATH}` → `/mnt/inbound:ro` | User-owned source library; Melodee treats it as read-only. |
| Canonical media | `melodee_library` → `/var/lib/melodee/library` | Verified immutable Release bundles. |
| Normalization work and retained attempt evidence | `melodee_work` → `/var/lib/melodee/work` | Non-authoritative, separate from inbound and canonical storage. |
| Tantivy | `melodee_search` → `/var/lib/melodee/search` | Rebuildable projection, but persist it to avoid unnecessary rebuilds. |
| Logs | container stdout/stderr | Collect with your container runtime; no file-log volume is required. |

The `initialize-storage` service changes ownership of only the three new named-volume roots to the
image's non-root UID `10001`. It does not recursively modify your media. If you replace named
volumes with host binds, create the directories yourself and grant that UID write access only to
Melodee-owned roots. Keep inbound mounted read-only and never overlap it with work, canonical, or
search roots. Mounted/NAS paths are valid when they provide the filesystem semantics required by
the [storage model](./storage/).

Podcast downloads are disabled in the example. If you enable the podcast worker, add a separate,
Melodee-owned persistent path for `MELODEE__PODCAST__STORAGE_ROOT`; it must not overlap any path in
the table.

## Image contents and health checks

The multi-stage image contains `melodee-server`, the `melodee-cli` administrative CLI, the built admin
SPA, CA certificates, timezone data, and Debian-packaged `ffmpeg`, `ffprobe`, and Chromaprint
`fpcalc`. The image runs as UID/GID `10001`, not root. Tool versions are logged by the release
workflow and vary only with the selected supported Debian base package set; the project requires
compatible tools, not a developer-machine version.

The container health check calls `/healthz`, which intentionally proves only that the HTTP process
is live. Use `/readyz` for an orchestration readiness check: it requires PostgreSQL and, when
search is enabled, a usable Tantivy generation. See [Troubleshooting](../troubleshooting/) for the
exact boundary.

Melodee receives `SIGTERM` as a graceful shutdown request, stops accepting work, drains the HTTP
server and workers, then closes PostgreSQL. Give the container a suitable stop timeout for your
workload.

## Supply chain and package visibility

Every published image is built only after the full repository gate, a PostgreSQL-backed runtime
smoke test, and vulnerability scanning. GitHub Actions publishes an SPDX SBOM with the image and a
keyless GitHub OIDC provenance attestation. Verify a release image with the GitHub CLI:

```bash
gh attestation verify oci://ghcr.io/melodee-project/melodee:X.Y.Z \
  -R melodee-project/melodee-rs
```

The application is MIT-licensed. The image also contains FOSS Debian runtime packages; their
package-specific copyright and license notices are available in the image under `/usr/share/doc`.

After the first release, an organization administrator may need to set the GHCR package to
**public** and confirm its repository linkage. This is GitHub package policy, not a setting that a
safe release workflow can override.
