# Jellyfin Audio ordering index plan, 2026-11-26

This isolated SQL experiment evaluated the index added by migration 0069. It
is not an HTTP measurement. An invocation-owned fixture held 35,000 artists,
50,000 releases, and 600,000 tracks and took 177.2 seconds to seed. Four
consecutive `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` samples were taken before
the index, then four after it. Cache state was uncontrolled and the second group
may benefit from earlier execution. [Raw plans and observations](jellyfin-audio-index-plan-2026-11-26.json)
include per-node execution and buffer details.

The unfiltered production page query joins active releases and playable assets,
orders by `track.track_number, track.public_api_id`, limits to 25 and obtains
the first primary artist's name and public ID. Optional search and release
parameters were simplified to SQL NULL. The separate exact count query joined
tracks to active releases. Measured page execution changed from 109.7–118.4 ms
to 1.29–1.68 ms; the plan changed from a parallel scan and sort across 600,000
tracks to an ordered index scan returning 25 tracks. Exact count remained
600,000 and the complete 25-row SQL JSON SHA-256 matched before and after.
Count execution remained 67.6–76.8 ms across the eight samples. PostgreSQL's
ascending B-tree defaults to `NULLS LAST`, and the public ID key preserves the
existing tie order.

The two-column index occupied 24,387,584 bytes and took 0.233 seconds to build
in one trial on the fixture. These figures exclude deployment coordination and
do not measure production write overhead. Every inserted or deleted track, and
updates to either indexed key, incur index maintenance. Migration 0069 uses
ordinary transactional DDL under the ADR-0166 maintenance fence; operators
must budget disk space and stopped-writer migration time for their own catalog.
A verified pre-migration restore or a forward migration dropping the index is
the rollback path. There is no schema or data deletion in the migration.

The host was an AMD Ryzen 9 3900X (24 logical CPUs), Linux, btrfs, PostgreSQL
18.3. The debug migration CLI SHA-256 was
`4b4055812139e888fb546137374647974f2066b7133aa3114ac0986e5a44d730`;
its source identity was not independently recorded. The workspace was dirty at
HEAD `08de08d50cb1be6e76c66000232dae257c1a2d2f`. The fixture predates the
final integrated migration build; this experiment does not rehearse migration
0069 or establish final-server performance. It does not cover filtered search,
deep offsets, concurrent activity, or write amplification. Afterward, the
fresh migration and populated 0059-to-current upgrade/repair tests passed; the
populated test verifies the 0069 index's exact definition and validity. The 20
Jellyfin media API tests also passed, including null/tie pagination and the
separate Artist page-count regression. These correctness tests do not turn the
SQL experiment into a final-server performance measurement.

The original measurements above used a one-off probe. The reusable equivalent
below verifies the invocation-owned parent fixture, creates and registers a
fresh clone, compares the same queries with and without the exact candidate
index, records complete plans, and cleans up its clone. Choose a new output
path. The final migration CLI must already be built; SIGKILL, host failure, or
an uncertain clone-creation acknowledgement may require operator inspection.
The [tiny smoke](jellyfin-audio-plan-harness-smoke-2026-11-26.json) exercised
this reusable harness on a 12-track owned fixture and confirmed matching count
and page hashes; it is not representative performance evidence.

```console
python3 scripts/perf_representative.py --seed-timeout 1200 --migration-cli target/debug/melodee-cli -- python3 scripts/perf_jellyfin_audio_plan.py --output tests/performance/jellyfin-audio-index-reproduction.json
```
