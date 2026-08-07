# Phase 0 Legacy Capture Runbook

1. Check out and build a recorded legacy Melodee commit/tag.
2. Restore a sanitized representative database/media fixture.
3. Record legacy commit, server/database/tool versions, enabled features, fixture counts, and startup
   command in `design/evidence/phase-0/legacy-baseline.md`.
4. Start the legacy server on a non-public interface.
5. Capture public scenarios:

```bash
LEGACY_MELODEE_BASE_URL=http://127.0.0.1:5157 \
  python3 scripts/capture_baseline.py
```

6. Add authenticated scenario files and inject temporary credentials/tokens only through environment
   variables.
7. Inspect all output for secrets/private data before promotion.
8. Promote approved fixtures to the relevant `golden/` directory with baseline metadata.
9. Implement a test and document any normalizer.
10. Re-run the same scenarios against Rust and compare.

Do not use the production music library or database as a committed fixture.
