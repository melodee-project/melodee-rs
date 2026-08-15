---
title: Quick Start
description: Run a Melodee server with the official container deployment.
sidebar:
  order: 2
---

Melodee is deployed from the official OCI image, not from downloaded native binaries. The safest
first server uses the reference Compose file with PostgreSQL and read-only inbound media.

1. Clone the repository only to obtain the reviewed deployment files, then prepare your local
   configuration:

   ```bash
   git clone https://github.com/melodee-project/melodee-rs.git
   cd melodee-rs
   cp compose.env.example .env
   ```

2. Edit `.env`. Set independent values for `POSTGRES_PASSWORD`, `MELODEE_NATIVE_JWT_KEY`, and
   `MELODEE_JELLYFIN_TOKEN_PEPPER` with `openssl rand -base64 48`. Set the public HTTPS origin and
   `MELODEE_INBOUND_PATH` to an absolute host directory containing your inbound media. Do not
   commit `.env`.

3. Pull and start the image:

   ```bash
   docker compose pull
   docker compose up -d
   curl --fail http://127.0.0.1:8080/readyz
   ```

4. Bootstrap the first administrator, then open `/admin` at your configured public origin:

   ```bash
   docker compose exec -e MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD='a-long-unique-password' \
     melodee melodee-cli users bootstrap-admin --username admin --email admin@example.test
   ```

5. Complete the onboarding checks. They test the live database, storage roots, media tools,
   search index, administrator account, and email configuration before the server accepts
   completion.

The example exposes HTTP only on loopback. Deploy a TLS reverse proxy before making the service
reachable remotely and set the public origin to that proxy. For image tags, persistent volumes,
backups, upgrade steps, and attestation verification, continue to
[Container deployment](../installation/docker/).

Melodee currently has no supported migration/cutover path from an existing C# production
installation. That Phase 11 work remains separate from starting a new Rust deployment.

## First-run onboarding (implemented)

The admin UI carries a guided first-run wizard at **`/admin/onboarding`**. Sign in with an
administrator account; while onboarding is incomplete, the [Overview](../admin/overview/)
dashboard also shows a banner linking to it.

The wizard evaluates the live server on every load or refresh. Completion is fail-closed: the
server re-evaluates every required check at submission, so a regression between display and submit
is rejected with the affected checks. The equivalent headless command is `melodee-cli config validate`.
The same status and completion operations are available through the
[Admin API](../api/admin-api/#onboarding).
