# Production ingestion media-operation counter smoke, 2026-11-26

An invocation-owned PostgreSQL fixture containing 3 artists, 4 releases and 12
existing tracks ingested two generated five-second releases, one retained MP3
and one FLAC-to-MP3 conversion, with stage concurrency 2. Both reached
`cataloged` in 6.158 seconds. This is a wiring smoke, not representative
performance, a before/after comparison, or an RM-11 completion claim.

Opt-in structured TRACE logging produced 49 valid operation events with no
truncation, malformed lines, reported partial byte counts, missing required
labels or abnormal server shutdown. The [sanitized result](media-operation-count-smoke-2026-11-26.json)
contains the bounded fixed-label totals and limitations. Selected counts:

| Instrumented operation | Completed | Logical bytes read | Logical bytes written |
| --- | ---: | ---: | ---: |
| Acquisition tree hash | 12 | 751,974 | 0 |
| Media file hash | 8 | 438,532 | 0 |
| Retained media copy | 1 | 40,682 | 40,682 |
| Validation hash | 2 | 62,545 | 0 |
| Publication copy | 2 | 62,545 | 62,545 |
| Publication hash | 8 | 250,180 | 0 |

The same run recorded six ffprobe, two ffmpeg, two fpcalc, and six tool-version
processes. Concurrent cold-cache misses can launch duplicate version probes;
these counts do not imply six distinct tools. The logical byte totals are
reported at instrumented Rust read/write boundaries, not physical disk I/O.
Subprocess file reads, native tag-library reads, memory mapping, and uninstrumented
operations are excluded. Memory-buffer hash counts, if present, are bytes hashed
in memory and must not be added to disk-read totals. Trace overhead has not been
measured. Counts cover server startup through shutdown, not isolated stages.

The release server's [build record](media-operation-smoke-build-2026-11-26.json)
identifies binary SHA-256
`cdd948d0793aa705015474db5970b5e3ad89ad357f818d1d32673eeb6800a82b`, with
matching before/after source fingerprints at HEAD `37f2988`. The server build
included unintegrated custody code and emitted compiler warnings. The release
migration CLI had SHA-256
`453d8b5a2c00d0d229a6e99f99f2f6b6d39cd322756b6dfaf776ad909fee8c51`;
all 70 migration source hashes matched the server build record before and after
the CLI build, but the CLI's complete source correspondence was not separately
attested and other source changed after the server build. The workload script
hash was captured after the run only. These limitations prevent treating the
sample as final integrated evidence.

The first smoke attempt used an older debug migration CLI and the server stopped
before ingestion with `VersionMismatch(67)`; it produced no operation events.
The successful retry used the separately built CLI above. Both invocations
cleaned their owned databases and temporary media; no generated corpus or raw
server log is retained. Representative source-matched and trace-disabled
control workloads remain necessary to assess counts and sampling overhead.
