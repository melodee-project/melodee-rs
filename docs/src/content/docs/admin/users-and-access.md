---
title: Users and Access
description: User accounts, groups, and library access control.
sidebar:
  order: 5
---

Native Melodee accounts have durable public IDs, bcrypt password hashes, roles, refresh-token
families, and optional library-access group membership. Access tokens do not contain library
lists; each catalog or streaming operation evaluates the current PostgreSQL membership, so an
administrator can revoke access without waiting for a token to expire.

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

## Operator CLI

The `melodee` CLI provides headless account and group administration. All commands use the
configured `MELODEE__DATABASE__URL` and coordinate through PostgreSQL; they never bypass leases,
audit, or account-state controls.

- `melodee users bootstrap-admin --username <name> --email <address>` creates the first
  administrator and fails closed once any administrator exists. The password is read from the
  `MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD` environment variable — it is never accepted as a
  command-line argument, because shell history and process listings are not secret stores.
- `melodee users disable <username>` / `melodee users enable <username>` are the operator
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
