# R4 current migrated-client acceptance

This is the release-candidate checklist for the current Phase 11 migrated
target. Historical Phase 7/8 reports remain useful fixtures, but none of them
is a current R4 acceptance result. Every row below needs a report containing
the exact Melodee commit, migrated-run id (or redacted baseline fingerprint),
client version, platform version, configuration fingerprint, command, result,
and sanitized logs/screenshots.

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

# Frozen Android client checkout and Android Auto-mode instrumentation probe
MELODEE_PHASE7_CLIENT_BASE_URL=http://10.0.2.2:<disposable-fixture-port> \
  scripts/run_phase7_android_auto_probe.sh
```

For R4, run the same workflows after the target has been produced by
`legacy import`, `legacy verify`, and `search rebuild`; record the migrated run
and do not relabel the historical fixture evidence. The native Android phone
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
not close the physical-client gate.

