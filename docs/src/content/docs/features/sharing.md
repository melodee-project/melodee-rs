---
title: Sharing
description: Sharing Releases, Tracks, and playlists from Melodee.
sidebar:
  order: 4
---

Authenticated native Melodee clients can create shares for Artists, Releases, Tracks, and
playlists. Each share has an immutable public ID and a public-link bearer generated from at least
128 bits of operating-system randomness. Share lists expose
only the current user's shares; only the owner or an administrator can read, change, or delete a
protected share.

The returned `/share/{token}` URL works directly and has the same JSON behavior as
`/api/v1/shares/public/{token}`. These public routes do not require an account token. They expose
only the captured public DTO, increment the visit counter, apply request-rate limits, and return the
same `404 Not Found` response for unknown, malformed, expired, or revoked links. Downloadable
shares report that policy in the DTO; the share does not bypass stream authorization or filesystem
path checks.

Use an expiry for links that should not remain valid indefinitely. Deleting a share removes the
derived share record, not the underlying catalog entity or media.

During migration, existing share URLs are preserved only when their bearer has at least 128 bits of
verified unpredictable material. Weaker legacy bearers are rotated, remain inactive, and are
reported by the migration so the owner can distribute the replacement URL.
