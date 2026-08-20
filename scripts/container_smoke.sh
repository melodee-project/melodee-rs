#!/usr/bin/env bash
set -euo pipefail

# Keep the operator command self-describing without requiring an image,
# credentials, Docker access, or a running PostgreSQL service. This is also
# used by the release runbook's command-help audit.
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'USAGE'
Usage: SMOKE_IMAGE=<local-image> RELEASE_VERSION=<version> [options] scripts/container_smoke.sh

Required environment:
  SMOKE_IMAGE       Local image reference to inspect and run.
  RELEASE_VERSION   Version expected in OCI labels and melodee-cli output.

Optional environment:
  RELEASE_REVISION  Expected OCI revision label when supplied.
  SMOKE_RUN_ID      Unique disposable Docker object suffix (default: local-<pid>).
  SMOKE_PORT        Unused host port from 1024-65535 (default: 18080).
  POSTGRES_IMAGE    PostgreSQL image for the disposable smoke database
                    (default: postgres:18.4-alpine).

The harness creates only uniquely named disposable Docker objects, refuses to
reuse them, validates image labels and runtime behavior, and removes the
objects on exit. It never publishes or mutates a registry image.
USAGE
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "unknown argument: $1 (use --help)" >&2
  exit 2
fi

# Reproduce the release-container runtime smoke test without publishing an
# image. Every disposable Docker object has a validated, caller-owned name;
# existing objects cause a fail-closed exit instead of being removed.
: "${SMOKE_IMAGE:?SMOKE_IMAGE must name the image to test}"
: "${RELEASE_VERSION:?RELEASE_VERSION must match the image version}"

release_revision="${RELEASE_REVISION:-}"

smoke_run_id="${SMOKE_RUN_ID:-local-$$}"
smoke_port="${SMOKE_PORT:-18080}"
postgres_image="${POSTGRES_IMAGE:-postgres:18.4-alpine}"

if [[ ! "$smoke_run_id" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$ ]]; then
  echo "SMOKE_RUN_ID contains unsupported characters or is too long" >&2
  exit 2
fi
if [[ ! "$smoke_port" =~ ^[0-9]+$ ]] || (( smoke_port < 1024 || smoke_port > 65535 )); then
  echo "SMOKE_PORT must be an integer between 1024 and 65535" >&2
  exit 2
fi

network="melodee-release-smoke-${smoke_run_id}"
database="${network}-postgres"
server="${network}-server"

if [[ "${#network}" -gt 63 || "${#database}" -gt 63 || "${#server}" -gt 63 ]]; then
  echo "SMOKE_RUN_ID produces a Docker name longer than 63 characters" >&2
  exit 2
fi

for object in "$network" "$database" "$server"; do
  if docker ps -a --format '{{.Names}}' | grep -Fx -- "$object" >/dev/null; then
    echo "refusing to reuse existing Docker container: $object" >&2
    exit 2
  fi
done
if docker network ls --format '{{.Name}}' | grep -Fx -- "$network" >/dev/null; then
  echo "refusing to reuse existing Docker network: $network" >&2
  exit 2
fi
if ! command -v ss >/dev/null; then
  echo "ss is required to validate the disposable host port" >&2
  exit 2
fi
if ss -ltnH | awk '{print $4}' | grep -Eq ":${smoke_port}$"; then
  echo "refusing to use occupied host port ${smoke_port}" >&2
  exit 2
fi

network_created=false
database_created=false
server_created=false
cleanup() {
  if [[ "$server_created" == true ]]; then
    docker rm -f "$server" >/dev/null 2>&1 || true
  fi
  if [[ "$database_created" == true ]]; then
    docker rm -f "$database" >/dev/null 2>&1 || true
  fi
  if [[ "$network_created" == true ]]; then
    docker network rm "$network" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

image_id="$(docker image inspect --format '{{.Id}}' "$SMOKE_IMAGE")"
[[ "$image_id" == sha256:* ]]
labels="$(docker image inspect --format '{{json .Config.Labels}}' "$SMOKE_IMAGE")"
jq -e --arg version "$RELEASE_VERSION" --arg revision "$release_revision" '
  .["org.opencontainers.image.title"] == "Melodee" and
  .["org.opencontainers.image.source"] == "https://github.com/melodee-project/melodee-rs" and
  .["org.opencontainers.image.version"] == $version and
  .["org.opencontainers.image.licenses"] == "MIT" and
  ($revision == "" or .["org.opencontainers.image.revision"] == $revision)
' <<< "$labels" >/dev/null
docker image inspect "$SMOKE_IMAGE" >/dev/null
docker network create "$network" >/dev/null
network_created=true
docker run -d --name "$database" --network "$network" \
  -e POSTGRES_USER=melodee -e POSTGRES_PASSWORD=melodee-test \
  -e POSTGRES_DB=melodee "$postgres_image" >/dev/null
database_created=true

for _ in $(seq 1 30); do
  if docker exec "$database" pg_isready -U melodee -d melodee >/dev/null; then
    break
  fi
  sleep 1
done
docker exec "$database" pg_isready -U melodee -d melodee >/dev/null

test "$(docker run --rm --entrypoint id "$SMOKE_IMAGE" -u)" = "10001"
ffmpeg_version="$(docker run --rm --entrypoint ffmpeg "$SMOKE_IMAGE" -version)"
ffprobe_version="$(docker run --rm --entrypoint ffprobe "$SMOKE_IMAGE" -version)"
printf '%s\n' "${ffmpeg_version%%$'\n'*}"
printf '%s\n' "${ffprobe_version%%$'\n'*}"
docker run --rm --entrypoint fpcalc "$SMOKE_IMAGE" -version
cli_version="$(docker run --rm --entrypoint melodee-cli "$SMOKE_IMAGE" --version)"
[[ "$cli_version" == *"$RELEASE_VERSION"* ]]

docker run -d --name "$server" --network "$network" -p "127.0.0.1:${smoke_port}:8080" \
  -e MELODEE__DATABASE__URL="postgres://melodee:melodee-test@${database}:5432/melodee" \
  -e MELODEE__NATIVE__PUBLIC_ORIGIN="http://127.0.0.1:${smoke_port}" \
  -e MELODEE__NATIVE__AUTH__JWT_KEY=release-smoke-native-jwt-key-at-least-thirty-two-bytes \
  -e MELODEE__JELLYFIN__TOKEN_PEPPER=release-smoke-jellyfin-pepper-at-least-thirty-two-bytes \
  -e MELODEE__INGEST__ENABLED=false "$SMOKE_IMAGE" >/dev/null
server_created=true

for _ in $(seq 1 30); do
  if curl --fail --silent --show-error "http://127.0.0.1:${smoke_port}/readyz" >/dev/null; then
    break
  fi
  sleep 1
done
curl --fail --silent --show-error "http://127.0.0.1:${smoke_port}/healthz" \
  | jq -e --arg version "$RELEASE_VERSION" '.status == "ok" and .version == $version' >/dev/null
curl --fail --silent --show-error "http://127.0.0.1:${smoke_port}/readyz" \
  | jq -e --arg version "$RELEASE_VERSION" '.status == "ready" and .version == $version' >/dev/null
admin_html="$(curl --fail --silent --show-error "http://127.0.0.1:${smoke_port}/admin/")"
grep -q '<div id="root"' <<< "$admin_html"

docker stop --time 15 "$server" >/dev/null
test "$(docker inspect --format '{{.State.ExitCode}}' "$server")" = "0"
printf 'container smoke passed: image=%s image_id=%s version=%s\n' \
  "$SMOKE_IMAGE" "$image_id" "$RELEASE_VERSION"
