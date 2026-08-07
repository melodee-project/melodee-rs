# Compatibility Baseline

This directory stores sanitized, reviewable evidence of the existing C# server's external behavior.
It is not a place for production credentials or private library dumps.

## Surfaces

- `melodee-api-v1/` — native first-party REST API.
- `opensubsonic/` — OpenSubsonic/Subsonic behavior.
- `jellyfin/` — implemented music-focused Jellyfin subset.
- `scenarios/` — declarative capture scenarios.
- `captures/` — local generated output; ignored by Git except `.gitkeep`.

## Capture public baseline

```bash
export LEGACY_MELODEE_BASE_URL=http://127.0.0.1:5157
python3 scripts/capture_baseline.py
```

For scenarios marked `auth: bearer`, set a temporary token in
`LEGACY_MELODEE_BEARER_TOKEN`. Never commit it.

## Golden fixture rules

A fixture promoted from `captures/` into a `golden/` directory must:

- contain no secrets, private hostnames, real user emails, or private filesystem paths;
- include request metadata and status/headers needed for comparison;
- identify volatile fields and the normalizer used;
- identify the legacy commit/version/database fixture;
- remain human-readable where practical; and
- be accompanied by a test.

Do not normalize meaningful differences merely to make tests pass.
