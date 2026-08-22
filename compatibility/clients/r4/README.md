# Migrated-client acceptance

This is the release-candidate checklist for the current migrated target.
Historical client reports remain useful fixtures, but none of them is a current
acceptance result. Every passing or failing row below needs a
report containing the exact Melodee commit, migrated-run id (or redacted
baseline fingerprint), client version, platform version, configuration
fingerprint, command, result, and sanitized logs/screenshots.

The committed `acceptance-matrix.json` is schema version 3. A reported row must
include an explicit pass/fail result for every workflow listed in its
`workflowSchema`; a row cannot be marked `pass` while any required workflow is
missing or failed. Validate it with:

```bash
python3 scripts/validate_client_acceptance.py
```

The validator permits the current `pending-external` rows, requires all
provenance fields for a reported result, requires every evidence path to name
an existing repository file, rejects absolute or escaping evidence paths and
obvious unredacted credentials, and only reports `pass` when all five rows
have passing evidence. It is an evidence-shape gate, not a client acceptance
substitute.

## Safety boundary

Run client acceptance only against a disposable migrated target and a copied or
isolated canonical-root mapping. Do not point a client at a production
database, canonical media root, OIDC tenant, Last.fm account, SMTP recipient,
or external provider until the release owner supplies the approved endpoint
and operator direction. The client probes must not print bearer tokens,
passwords, cookies, private roots, or personal data.

## Reproducible local launch paths

The existing harnesses provide the deterministic client launch pieces:

```text
# Feishin v1.15.1, OpenSubsonic + Jellyfin protocol smoke on a disposable fixture
python3 scripts/run_phase8_client_smoke.py --feishin-dist /path/to/feishin/out/web

# The same smoke against an already migrated, isolated target. The URL is
# supplied through the environment so it is not exposed in shell history or
# the launcher's output.
MELODEE_MIGRATED_DATABASE_URL='<redacted isolated URL>' \
  python3 scripts/run_phase8_client_smoke.py \
    --feishin-dist /path/to/feishin/out/web \
    --existing-db-url-env MELODEE_MIGRATED_DATABASE_URL \
    --existing-storage-root /isolated/canonical-root

# Frozen Android client checkout and Android Auto-mode instrumentation probe
MELODEE_PHASE7_CLIENT_BASE_URL=http://10.0.2.2:<disposable-fixture-port> \
  scripts/run_phase7_android_auto_probe.sh
```

For a release candidate, use the existing-target form after the target has been produced by
`legacy import`, `legacy verify`, and `search rebuild`; record the migrated run
and do not relabel the historical fixture evidence. The launcher does not seed
or drop the supplied database and validates that the supplied storage root is
an existing directory with no symlinked path components. It waits for the
local client server and relay before starting the probe. The native Android phone
and MeloAmp workflows use the client launch procedures in
`design/evidence/phase-7/REAL-CLIENT-ACCEPTANCE.md`; the representative
OpenSubsonic/Jellyfin workflow uses
`compatibility/clients/phase8/README.md`.

## Acceptance matrix

| Client/surface | Required current workflow | Status | Owner/input |
|---|---|---|---|
| Android phone / native v1 | login, refresh, browse/search, direct play, configured transcode, range/seek, next/previous, background playback, reconnect, playlists, scrobble, expected errors | Pending external | release owner: physical phone and current app build |
| Android Auto or DHU | root browse, search/voice playback, next/previous, seek, ended-event advance, reconnect/auth restore | Pending external | release owner: head unit or DHU and operator |
| MeloAmp | login/restore, browse/search, direct play/seek, playlists, reconnect, error paths | Pending external | release owner: current MeloAmp build/runtime |
| Representative OpenSubsonic client | connect/auth, browse/search, direct play/range, playlist, favorites, reconnect, expected errors | Pending external | release owner: selected client/version |
| Representative Jellyfin music client | connect/auth, browse/search, direct play/transcode/range, favorites/playlists, reconnect, expected errors | Pending external | release owner: selected client/version |

No row is complete until the migrated release-candidate target and the current
client versions are recorded. A passing historical emulator or Feishin run does
not close the physical-client gate. Result artifacts must use
repository-relative paths so private filesystem roots cannot enter committed
evidence.
