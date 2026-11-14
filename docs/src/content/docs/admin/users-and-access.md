---
title: Users and Access
description: User accounts, groups, and library access control.
sidebar:
  order: 5
---

## Users & Access screen

Open **System → Users & Access** (`/admin/system/users`). The screen provides:

- **Users** — a server-paged, server-sorted, server-filtered account table with create, edit
  (roles and the admin/editor flags, enable/disable), and password-reset flows. Every mutation
  surfaces the stable error envelope (`409 user_conflict`, `400 invalid_password`, …) localized;
  passwords are accepted only at creation and on the set-password flow and never render.
- **Groups** — the library-access groups tab: create, rename, list, delete, add or remove member
  users, and bind or unbind libraries. The server refuses to delete a group that still has
  members or bindings (`409 group_not_empty`).

## Account model

Native Melodee accounts have durable public IDs, bcrypt password hashes, roles, refresh-token
families, and optional library-access group membership. Access tokens do not contain library
lists; each catalog or streaming operation evaluates the current PostgreSQL membership, so an
administrator can revoke access without waiting for a token to expire.

Library IDs are access scopes over the configured storage roots, not editable path records; media
roots are configured at deployment time
([storage](../installation/storage.md#libraries-are-access-scopes-not-mutable-path-records)).

A library with no group binding is available to authenticated users. As soon as one or more
groups are bound to a library, only members of at least one bound group can browse, search,
favorite/rate, scrobble, or stream its Releases and Tracks. Restricted facts are also removed from
user lists, counts, and listening-history responses. Unknown and inaccessible entity IDs share the
same not-found behavior to avoid revealing catalog membership.

The native `/api/v1/admin/users` route is limited to administrators and returns a bounded account
summary. Passwords, stream signing keys, refresh-token digests, and internal database IDs are never
returned.

Broader administration lives on the Rust-native [Admin API](../api/admin-api/): administrators can
create accounts (with an initial password, stored as a bcrypt hash), update roles and the
admin/editor flags, and disable or re-enable accounts. Disabling or locking an account revokes all
of its refresh sessions immediately, and every change invalidates in-flight credential issuance.
The same surface manages library-access groups: create, rename, list, delete, add or remove member
users, and bind or unbind libraries. A group can be deleted only when it has no members and no
library bindings; the server refuses the delete otherwise.

Users can inspect and remove their own linked authentication providers. A Google account can be
linked only after its ID token passes the configured issuer, audience, lifetime, verified-email,
and optional hosted-domain checks. A provider subject is owned by at most one user and is never
exposed in API responses or logs.

## Account self-service

MeloAmp owns the listener-facing account screens. It uses the additive native account routes; the
Rust server does not recreate the legacy Blazor listener SPA:

```http
POST /api/v1/auth/register
GET /api/v1/user/me/profile
PATCH /api/v1/user/me/profile
```

Registration is enabled by default and is controlled by the `register.disabled` setting. When
`register.privateCode` has a value, registration also requires that access code. The first
successful registration is bootstrapped as the administrator; later registrations cannot assign
roles or administrator authority. Browser password-reset completion remains a separate security
surface.

The profile route can update only the authenticated user's username, email, description/bio,
timezone, preferred language, and preferred theme. Values are bounded and validated, including
safe timezone identifier syntax. Role, account-state, public-ID, and password fields are not
accepted by the profile request. Each actual change creates a value-free audit event in the same
PostgreSQL transaction.

In MeloAmp, open **Profile** (`/profile`) from the sidebar footer. The screen edits those fields
and manages the user's avatar: choose a JPEG/PNG, upload it, or remove it. The server owns the
bounded resize/re-encode pipeline; the app only previews it and refreshes `/user/me` after each
change.

Administrators can manage the same fields and the avatar for any account from **System → Users →
Edit** in the Admin UI. The dialog keeps the existing role and account-state controls and adds the
profile fields and avatar upload/remove. Those admin mutations are audited with the acting
administrator as the actor; the Admin API endpoints are documented in
[Admin API — Users](../api/admin-api/#users).

### Browser password reset

Password-reset completion is the one bounded anonymous security page served by Rust. When email
delivery is enabled, a reset notification links to `/reset-password?token=…`; opening that link
validates the one-use token and presents an accessible new-password form. Passwords must contain
at least 8 characters. A successful reset revokes the account's existing refresh/session
authority, and the user can then sign in again from MeloAmp or another native client.

The request API deliberately returns the same success message for known and unknown email
addresses. Reset links expire after one hour by default, are invalid after use, and are rate
limited. The page removes the token from browser history before validation, makes only same-origin
API requests, sends no ambient session credentials, loads no third-party assets, and returns
`no-store`/`no-referrer` security headers. Invalid, expired, replayed, malformed, and missing
tokens share stable localized error states. Rust does not recreate the legacy listener SPA or
redirect to an unserved browser login page.

Operators should set `MELODEE__NATIVE__PUBLIC_ORIGIN` to the externally reachable origin so
notification links resolve correctly. `MELODEE__NATIVE__AUTH__DEVELOPMENT_RESET_TOKENS` exposes a
clear token only for isolated development testing and must remain disabled in production.

## Avatars

Users manage their own avatar through the native API:

```http
GET /api/v1/user/me/avatar
POST /api/v1/user/me/avatar
DELETE /api/v1/user/me/avatar
```

Uploads are raw request bodies with a `Content-Type` of `image/jpeg` or `image/png`. The bytes are
validated by content (magic bytes must match the declared type), decoded under bounded resource
limits, downscaled to at most 1024 pixels per edge, and re-encoded to at most 1 MiB before storage.
`GET` returns the stored bytes with their content type and an ETag, and `DELETE` removes the avatar
(`404` when none is stored). OpenSubsonic clients read the same image through `getAvatar`, and the
`getUser` `avatarLastChanged` field reflects the time of the last avatar change.

During legacy migration, the importer reads the configured C# User Images library read-only and
accepts only direct regular files named `<legacy-user-id padded to 8>.gif`. Accepted images are
decoded and normalized to bounded PNG bytes before storage. Unsafe, unsupported, unmapped, or
unreadable files become migration issues; the source path is never used as identity or copied into
the Rust database.

## Operator CLI

The `melodee` CLI provides headless account and group administration. All commands use the
configured `MELODEE__DATABASE__URL` and coordinate through PostgreSQL; they never bypass leases,
audit, or account-state controls.

- `melodee-cli users bootstrap-admin --username <name> --email <address>` creates the first
  administrator and fails closed once any administrator exists. The password is read from the
  `MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD` environment variable — it is never accepted as a
  command-line argument, because shell history and process listings are not secret stores.
- `melodee-cli users disable <username>` / `melodee-cli users enable <username>` are the operator
  recovery path. Disabling takes effect immediately: in-flight credential issuance is fenced and
  every refresh family is revoked atomically.
- `melodee groups list` shows every library-access group with its member and binding counts.
  `melodee groups create <name>` creates a group (an exact name conflict fails closed);
  `melodee groups add-user <group> --username <name>` and
  `melodee groups remove-user <group> --username <name>` manage membership;
  `melodee groups bind-library <group> --library-id <uuid>` and
  `melodee groups unbind-library <group> --library-id <uuid>` manage library bindings;
  `melodee groups delete <name>` deletes a group only after its members and bindings were removed
  — an access-control group is never silently detached.
- `melodee-cli notifications templates list` shows the stored notification templates (key, locale,
  version, subject). `melodee-cli notifications test-send --template <key> --to <address>
  [--locale <tag>]` renders a template and attempts a real delivery through the configured
  [email transport](../installation/configuration/), printing the honestly recorded audit row —
  including `skipped` when delivery is disabled — so notification behavior is verifiable
  headlessly.
