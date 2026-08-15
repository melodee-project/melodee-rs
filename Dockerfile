# syntax=docker/dockerfile:1.7

# The runtime is intentionally assembled from maintained Debian packages rather
# than the developer workstation.  ADR-0108 requires explicit, FOSS media
# tooling; Debian's ffmpeg and libchromaprint-tools packages provide ffmpeg,
# ffprobe, and fpcalc respectively.
ARG RUST_VERSION=1.97.1
ARG NODE_VERSION=22

FROM node:${NODE_VERSION}-bookworm-slim AS admin-ui-build

WORKDIR /src/admin-ui

# pnpm's frozen lockfile commits to both its workspace build-script policy and
# the dependency-rewrite hook, so they must be present before installation.
COPY admin-ui/package.json admin-ui/pnpm-lock.yaml admin-ui/pnpm-workspace.yaml admin-ui/.pnpmfile.cjs ./
RUN corepack enable && pnpm install --frozen-lockfile

COPY admin-ui/ ./
RUN pnpm build

FROM rust:${RUST_VERSION}-bookworm AS rust-build

WORKDIR /src

COPY . ./
RUN cargo build --locked --release -p melodee-server -p melodee-cli

FROM debian:bookworm-slim AS runtime

ARG VERSION=dev
ARG REVISION=unknown
ARG CREATED=unknown

LABEL org.opencontainers.image.title="Melodee" \
      org.opencontainers.image.description="Self-hosted Melodee music server" \
      org.opencontainers.image.source="https://github.com/melodee-project/melodee-rs" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.created="${CREATED}" \
      org.opencontainers.image.licenses="MIT"

RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl ffmpeg libchromaprint-tools tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 melodee \
    && useradd --uid 10001 --gid melodee --create-home --home-dir /var/lib/melodee \
        --shell /usr/sbin/nologin melodee \
    && install -d --owner=melodee --group=melodee /opt/melodee /var/lib/melodee/work \
    && install -d /usr/share/licenses/melodee

WORKDIR /opt/melodee

COPY --from=rust-build /src/target/release/melodee-server /usr/local/bin/melodee-server
COPY --from=rust-build /src/target/release/melodee-cli /usr/local/bin/melodee-cli
COPY --from=admin-ui-build --chown=melodee:melodee /src/admin-ui/dist /opt/melodee/admin-ui/dist
COPY LICENSE /usr/share/licenses/melodee/LICENSE

ENV MELODEE__SERVER__BIND=0.0.0.0:8080 \
    MELODEE__ADMIN_UI__DIST=/opt/melodee/admin-ui/dist \
    MELODEE__NORMALIZATION__WORK_AREA=/var/lib/melodee/work

USER melodee:melodee

EXPOSE 8080

# Liveness deliberately avoids PostgreSQL: orchestration should use /readyz
# when it needs the database/search readiness contract instead.
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["melodee-server"]
