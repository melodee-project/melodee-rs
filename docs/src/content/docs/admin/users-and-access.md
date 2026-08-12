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

Users can inspect and remove their own linked authentication providers. A Google account can be
linked only after its ID token passes the configured issuer, audience, lifetime, verified-email,
and optional hosted-domain checks. A provider subject is owned by at most one user and is never
exposed in API responses or logs.
