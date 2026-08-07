# Compatibility Normalization

Normalization is intentionally minimal and versioned. It may replace correlation IDs, known generated
timestamps, deployment base URLs, and explicitly unstable ordering only when legacy behavior proves
those values are volatile and non-contractual.

Never normalize status codes, property names/types, null versus omission, array order used by clients,
public IDs, pagination, errors, cache/range headers, or authentication behavior merely to make a test
pass. Each rule must name the field, rationale, surface, fixture, and approving review/ADR.

## Mechanism

Normalization rules are declared per scenario in the `normalize` block (see
`compatibility/schema/scenario.schema.json`):

- `maskJsonPaths`: JSON paths (`$.a.b`, `$.a[*].b`, `$.a[0].b`) whose values are replaced with
  `<volatile>` before comparison. Use only for values that vary between identical environments:
  tokens, fixture-generated UUIDs, timestamps, build-stamped version strings, configured base URLs.
- `dropHeaders`: response headers removed before comparison.

`capture_baseline.py` copies the scenario's `normalize` block into each capture's metadata so
comparisons are self-contained. `compare_captures.py` applies the masks and then requires exact
structural equality: object key order is ignored, but types, null-vs-omitted, and array order are
significant.

Current declared volatile fields:

| Scenario | Field | Rationale |
|---|---|---|
| jellyfin-system-info-public | `$.LocalAddress` | configured listener address |
| jellyfin-system-info-public | `$.Version` | embeds build timestamp (`2.2.0+build<ts>.<commit>`) |
| jellyfin-system-info-public | `$.Id` | server UUID generated per installation/database |
| native-auth-login | `$.token`, `$.refreshToken`, `$.expiresAt`, `$.refreshTokenExpiresAt` | credential material and derived timestamps |
| native-auth-login | `$.user.id`, `$.user.createdAt`, `$.user.updatedAt`, `$.user.thumbnailUrl`, `$.user.imageUrl` | fixture-generated UUID/timestamps; URLs embed configured baseUrl and user id |
| native-user-me | same user fields | same rationale |
