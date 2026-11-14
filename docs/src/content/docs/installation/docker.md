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

:::caution[Release availability]
This procedure requires a published image tag. The project is still qualifying its initial
v1.0.0 release; the Compose file and release workflow do not establish that a qualified image or
`latest` tag is available. Check the release notes first, or use a source build for development.
:::

The repository's [`compose.yml`](../../../../../compose.yml) is the reference deployment. It uses
the released GHCR image, PostgreSQL 18, named volumes for Melodee-owned state, and an inbound bind
mount that permits only candidate-directory rename/removal after verified acquisition.

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs
cp compose.env.example .env
# Edit .env: set a published MELODEE_IMAGE_TAG, distinct random secrets,
# a matching PostgreSQL password/URL, a public origin, and an absolute inbound path.
docker compose pull
docker compose up -d
docker compose ps
```

Use `openssl rand -base64 48` for both Melodee secrets. Do not commit `.env`. If the PostgreSQL
password contains reserved URL characters, percent-encode it in `MELODEE_DATABASE_URL`. The
example permits the exact local origins used by MeloAmp through
`MELODEE_NATIVE_CORS_ALLOWED_ORIGINS`; remove an origin only if clients at that origin do not need
the native API.

The Compose example binds the HTTP port only to `127.0.0.1` by default. Put a TLS-terminating
reverse proxy in front of it and set `MELODEE_NATIVE_PUBLIC_ORIGIN` in the Compose `.env` to that
public HTTPS origin (Compose passes it to `MELODEE__NATIVE__PUBLIC_ORIGIN` in the container).
Only set `MELODEE_BIND_ADDRESS=0.0.0.0` for a deliberately firewalled direct deployment.

After startup, verify the server and create the first administrator:

```bash
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
# Bash: read the password without echoing it, then pass the environment variable by name.
read -r -s -p 'Administrator password: ' MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
export MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
docker compose exec -e MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD \
  melodee melodee-cli users bootstrap-admin --username admin --email admin@example.test
unset MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
```

The password is supplied through the process environment rather than a command-line argument.
Open `https://your-public-origin/admin` and finish the server-side onboarding checks.

### Reverse-proxy client addresses

Set `MELODEE_SERVER_TRUSTED_PROXIES` to the exact source addresses or CIDRs of the proxies that
connect to the container. With a native server and a proxy on loopback, use
`MELODEE__SERVER__TRUSTED_PROXIES=127.0.0.1,::1`. In a bridge-networked container the observed
source may instead be the bridge gateway or another container address; trust only controlled proxy
infrastructure. Have the proxy replace incoming forwarding headers. This configuration applies to
all APIs, rate limits, and configured IP deny lists.

Keep `MELODEE_OPENSUBSONIC_LEGACY_AUTH_BYPASS=false`. Loopback connections and a matching Host
are not credentials under the default policy. Existing local clients that relied on anonymous access
must configure credentials. See [OpenSubsonic authentication](../api/opensubsonic/#authentication)
and [proxy configuration](./configuration/#native-api-urls-streaming-and-proxies).

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

`MELODEE_IMAGE_TAG` accepts a tag, not a bare digest: the reference Compose file inserts a colon
before its value. To pin a digest, override the `melodee` service's full `image` value with
`ghcr.io/melodee-project/melodee@sha256:<published-manifest-digest>` in your deployment configuration.

## Persistent state and mounts

The example declares these paths intentionally:

| State | Compose location | Notes |
| --- | --- | --- |
| PostgreSQL | `melodee_postgres` | Authoritative catalog, identity, users, settings, templates, themes, user images, and audit state. |
| Inbound media | `${MELODEE_INBOUND_PATH}` → `/mnt/inbound` | External filesystem drop directory. Media bytes are immutable; the runtime must be able to rename/remove a successfully acquired candidate directory. |
| Durable staging | `melodee_staging` → `/var/lib/melodee/staging` | Persistent Melodee-owned original source for processing, review, quarantine, and recovery. |
| Canonical media | `melodee_library` → `/var/lib/melodee/library` | Verified immutable Release bundles. |
| Normalization work and retained attempt evidence | `melodee_work` → `/var/lib/melodee/work` | Non-authoritative, separate from inbound and canonical storage. |
| Tantivy | `melodee_search` → `/var/lib/melodee/search` | Rebuildable projection, but persist it to avoid unnecessary rebuilds. |
| Logs | stdout/stderr; optional `melodee_logs` → `/var/log/melodee` | Console collection remains the default. Set `MELODEE_LOGGING_DIRECTORY=/var/log/melodee` in the Compose `.env` to additionally append daily UTC files named `/var/log/melodee/YYYY-MM-DD.log`; configure external retention for older files. |

The `initialize-storage` service changes ownership of only the named-volume owned roots to the
image's non-root UID `10001`. It does not recursively modify your media. If you replace named
volumes with host binds, create the directories yourself and grant that UID write access only to
Melodee-owned roots. Inbound must allow the runtime UID to rename/remove a successfully acquired
candidate directory, while its media bytes remain immutable. Never overlap inbound with staging,
work, canonical, or search roots. Mounted/NAS paths are valid when they provide the filesystem semantics required by
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

The migration CLI is included in the same image. Check availability without opening a source or
changing a database:

```bash
docker compose exec melodee melodee-cli legacy --help
docker compose exec melodee melodee-cli rollback --help
```

Use the [legacy migration guide](./legacy-migration/) and the [cutover/rollback runbook](https://github.com/melodee-project/melodee-rs/blob/main/design/runbooks/CUTOVER-AND-ROLLBACK.md)
for source read-only enforcement, public-ID preservation, backups, final delta, and approval
gates. A local image smoke test is not production cutover evidence.

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

Release publication is intentionally release-event-only. A candidate can be built and scanned
locally, but the official GHCR image, immutable semantic-version tag, stable floating tags, SBOM,
vulnerability result, and provenance are not claimed until the release workflow completes with an
approved GitHub Release.

Do not treat a historical image digest, a dirty-tree build, or a scan of an image rebuilt after
scanning as a release candidate. The accepted release record must bind the exact commit, version,
amd64 and arm64 digests, SPDX SBOMs, vulnerability reports, checksums, and provenance. If a
required security finding has no fixed version, it still needs a documented release-owner
disposition; “no fixed version” is not an automatic waiver.

## Release identity and publication retries

Every official release job builds and validates the same pinned Git commit. The publication job
loads the archived images that passed smoke tests and vulnerability scans; it does not rebuild them.
An exact version tag cannot be replaced with a different image digest by the official workflow.
A changed image requires a new version. Publication is serialized, and floating tags are updated
before the immutable exact-version tag records completion. Retrying an already completed publication
with the same digest leaves all tags unchanged.

Release maintainers should rerun failed publication jobs using the original scanned-image evidence
artifacts, which are retained for 14 days. Rebuilding a published version is not a way to repair its
image. Registry authentication or network failures stop publication rather than being interpreted as
an absent version tag. Restrict package-write access to the controlled release workflow to preserve
these guarantees outside that workflow as well.
