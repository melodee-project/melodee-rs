# Compatibility Normalization

Normalization is intentionally minimal and versioned. It may replace correlation IDs, known generated
timestamps, deployment base URLs, and explicitly unstable ordering only when legacy behavior proves
those values are volatile and non-contractual.

Never normalize status codes, property names/types, null versus omission, array order used by clients,
public IDs, pagination, errors, cache/range headers, or authentication behavior merely to make a test
pass. Each rule must name the field, rationale, surface, fixture, and approving review/ADR.
