---
applyTo: "compatibility/**,crates/**/*api*/**,tests/compatibility/**"
---

# Compatibility instructions

- Capture legacy behavior before implementing or changing an adapter.
- Compare method, path, parameters, headers, status, body shape, property casing/types, omitted/null
  distinctions, ordering, auth, errors, range behavior, and conditional/gone/not-implemented behavior.
- Normalize only demonstrably volatile fields. Document every normalizer.
- Preserve existing public API keys/UUIDs through migration.
- Native Melodee API v1, OpenSubsonic, and Jellyfin are separate products with separate contracts.
- Do not improve or unify protocols at the expense of compatibility.
- An approved bug fix or difference must be documented and tested explicitly.
- Never commit credentials, tokens, private URLs, emails, filesystem paths, or production media.
