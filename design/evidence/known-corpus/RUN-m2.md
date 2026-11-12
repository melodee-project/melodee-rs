# Known-Answer Corpus — Run Evidence (m2, post-fix)

Second run of the known-answer exercise (`design/testing/UI-AND-WORKFLOW-AUDIT.md`), performed after
the two findings from `RUN-m1.md` were fixed. Purpose of this run: prove the fixes on a fresh corpus
with stronger assertions — every formerly "known gap" is now a hard check.

```bash
python3 scripts/generate_known_corpus.py --output /tmp/melodee-known-corpus-m2 --tier all --run-id m2
MELODEE__SERVER__BIND=0.0.0.0:8080 MELODEE__NATIVE__PUBLIC_ORIGIN=http://<host-lan-ip>:8080 make up
MELODEE_LIVE_USERNAME=… MELODEE_LIVE_PASSWORD=… \
  python3 scripts/verify_known_corpus.py --corpus /tmp/melodee-known-corpus-m2 --ingest \
    --client-base-url http://<host-lan-ip>:8080 \
    --emit-ui-expectations admin-ui/e2e/live/known-corpus.generated.json
MELODEE_LIVE_USERNAME=… MELODEE_LIVE_PASSWORD=… make audit-live
```

- corpus run: `m2` · tier `all` · 11 fixtures · **323 assertions**
- behaviour failures: **0**
- documented gaps: **0**

## What changed since m1

| Finding | m1 status | m2 evidence |
|---|---|---|
| F1 tagged date never reaches the Release | open (18 gap assertions) | `releases.release_date` and `release_date_text` carry the tagged date for all nine dated fixtures; the sidecar fixture keeps its documented text-only year |
| F2 import/provider cover art not served | open (annotated in the UI suite) | `/images/album_<id>/original` returns the published plan bytes byte-for-byte; `medium` returns a decoded 640px derivative |
| F2b provider art outranked the published plan | not yet known | precedence is now override → plan → provider → placeholder, asserted per fixture |
| F3 History opened on an empty filter | fixed in m1 | still asserted: finished fixtures appear under `Active`, never under an in-flight phase |

Verified values after the fix:

```
Static Bloom [m2]        release_date=2019-04-12  release_date_text=2019-04-12
Paper Lanterns [m2]      release_date=2020-09-01  release_date_text=2020-09-01
Two Rooms [m2]           release_date=2021-02-20  release_date_text=2021-02-20
Loose Ends [m2]          release_date=2018-11-30  release_date_text=2018-11-30
Hoppípolla EP [m2]       release_date=2022-06-17  release_date_text=2022-06-17
Unpictured [m2]          release_date=2023-01-05  release_date_text=2023-01-05
Greatest Hits [m2]       release_date=2017-08-08  release_date_text=2017-08-08
Broken Transmission [m2] release_date=2024-03-03  release_date_text=2024-03-03
Sidecar Source [m2]      release_date=NULL        release_date_text=2025   (sidecar year only)
```

## New assertions this run added

- **Release dates** — typed date *and* verbatim text, per fixture, as hard checks. A partial fix
  (text only) now fails instead of degrading to a gap.
- **Artwork serving** — the image endpoint is fetched for every fixture and compared with the
  published plan asset by SHA-256, plus a resized derivative check. This is the assertion that makes
  "artwork is published but never served" impossible to miss again.
- **Disposition visibility** — fixture 07 (`policy.disposition.review`) is asserted to keep its
  Release hidden: a non-Accept disposition deprecates the row, so the image endpoint must answer the
  empty `200` rather than leak artwork. Encoded as an assertion rather than an assumption.

## Harness lessons (recorded so the next run does not repeat them)

1. **Browser suites must run over loopback.** The admin session cookie is `Secure`; Chromium accepts
   it on `http://127.0.0.1` (potentially trustworthy) but not on a private plain-HTTP address, so the
   browser silently drops it and every screen renders signed out. Drive Playwright over
   `http://127.0.0.1:8080/admin` and use the LAN address only for the non-browser client leg
   (`verify_known_corpus.py --client-base-url`).
2. **A storage state minted for another origin is worse than none.** The cookie is domain-scoped, so a
   state file written for `127.0.0.1` produced a signed-out browser while the API helpers (which send
   the cookie by hand) kept working. `signIn()` now ignores persisted cookies that do not match the
   configured host and signs in again.
3. **Poll for image decoding.** `naturalWidth` is `0` until the image loads; asserting it once right
   after the element appears is a flaky test, not a product defect.
