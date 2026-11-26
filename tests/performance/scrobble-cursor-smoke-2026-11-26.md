# Phased scrobble cursor plan smoke, 2026-11-26

This is a small-fixture diagnostic, not a production-scale measurement. A
release-build test executable with SHA-256
`f4564e1c8e9ec96d5e73760da6f9a768ca8183ee6f580039182621910413ebec`
was built from an unrecorded dirty working tree; its exact source is not
established. The disposable fixture held 120 Artists, 150 Releases and 300
Tracks, with 450 observations/current decisions and 300 initial Track credits.

The test extracted the exact SELECT body of the then-current phased scrobble
cursor declaration and ran `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` on an
ordinary Track and a separate Track with additional primary Artist credits. It
executed the whole SELECT; it did not measure cursor FETCH latency, locks or
the whole scrobble operation. Plans are in
[scrobble-cursor-smoke-2026-11-26-plans.json](scrobble-cursor-smoke-2026-11-26-plans.json).

| Fixture | Primary credit rows | Plan output rows | Planning, ms | Execution, ms |
| --- | ---: | ---: | ---: | ---: |
| Ordinary | 1 | 2 | 0.104 | 0.044 |
| Additional credits | 120 | 240 | 0.071 | 0.204 |

Both plans contain an outer Sort above Append and an inner Sort in each branch,
although both credit scans are index-only. This is evidence of sorting and
materialization pressure, not evidence of bounded streaming. The 120-credit
case is limited by the small fixture and is not the requested 1,001-credit
representative case. This plan corresponds to the earlier cursor and must not
be attributed to a later implementation without remeasurement.
