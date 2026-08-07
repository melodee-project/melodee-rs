---
applyTo: "**"
---

# Security instructions

- Treat paths, URLs, headers, metadata, manifests, archives, tags, images, scripts, and media as
  untrusted input.
- Prevent path traversal, symlink escapes, command injection, SSRF, decompression bombs, and log
  forging.
- Child processes receive argument arrays, bounded resources, timeouts, cancellation, and captured
  sanitized diagnostics. Never invoke a shell for media arguments.
- Redact passwords, salts paired with hashes, JWTs, refresh tokens, API keys, HMAC tokens, cookies,
  and private paths from logs/fixtures.
- Use least privilege and role checks for administration and destructive actions.
- Delete is a durable authorized disposition with audit evidence; uncertainty defaults to review or
  quarantine, never destruction.
- Authentication compatibility changes require explicit tests and review.
