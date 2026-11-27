# Media normalization after subprocess supervision

This is a **media-stage** measurement of the final subprocess-supervision code, not an end-to-end
import, mixed-load test, or proof that a production worker default is appropriate. Raw per-sample
results and source hashes are in [the supervised matrix](codec-matrix-supervised-2026-11-26.json).
The earlier [baseline](codec-matrix-baseline-2026-11-26.json) and
[pre-supervisor sample](codec-matrix-final-2026-11-26.json) used the same stage harness parameters;
they were collected earlier and are not measurements from this run. Their source identities and
other details are documented in [the earlier comparison](codec-matrix-2026-11-26.md).

The supervised run completed at approximately 02:24 UTC on 2026-11-27 (20:24 local on November 26).
It used a release executable SHA-256
`2355a8e77c810c049a03d57e61a15c03ba24ca0243d0f54258d1888cad1e5420`, built from a dirty worktree
based on commit `08de08d50cb1be6e76c66000232dae257c1a2d2f`. The JSON records SHA-256 hashes of
`Cargo.toml`, `Cargo.lock`, the media manifest and source files, benchmark source, and test harness;
the earlier artifacts do not contain executable hashes. No source was changed during this run.

Environment: AMD Ryzen 9 3900X, 24 logical CPUs; Linux 7.3.4 on x86-64, glibc 2.44; Python 3.15.2;
FFmpeg and ffprobe 9.0.2; fpcalc 1.6.1. Generated fixtures and work areas used a disposable directory
on `/tmp` tmpfs, with approximately 32.4 GB free at fixture creation. The same source fixture was
reused without cache eviction. Each sample contains eight single-track releases from a 60-second
mono sine-wave file. Each row has **two samples**; the table reports mean releases per second,
maximum sampled process-tree RSS, and mean process-tree child CPU seconds (user plus system) per
sample. The CPU and RSS accounting includes the test process and subprocess activity outside the
normalization-only throughput timer, including inspection. RSS is sampled at 50 ms intervals and
can miss short-lived peaks.

| Codec | Workers | Original baseline releases/s | Pre-supervisor releases/s | Supervised releases/s | Supervised peak RSS MiB | Supervised child CPU s |
|---|---:|---:|---:|---:|---:|---:|
| MP3 | 1 | 1.049 | 1.641 | 1.731 | 73.5 | 5.853 |
| MP3 | 2 | 1.920 | 3.530 | 3.505 | 140.9 | 5.784 |
| MP3 | 4 | 3.908 | 6.649 | 6.853 | 261.1 | 5.821 |
| FLAC | 1 | 0.928 | 1.429 | 1.419 | 74.8 | 7.509 |
| FLAC | 2 | 1.879 | 2.848 | 2.825 | 143.6 | 7.593 |
| FLAC | 4 | 3.564 | 5.476 | 5.418 | 281.0 | 7.901 |

The observed samples are broadly comparable to the earlier pre-supervisor stage results, but the
small sample and separate collection times do not establish causation or a tail-latency bound. They
exercise actual inspection and normalization functions with default FFmpeg options and the new
media-tool admission, but at most four release workers do not saturate its eight-tool limit; they do
**not** exercise PostgreSQL, filesystem acquisition, ingestion worker
scheduling, publication, HTTP, search, concurrent interactive traffic, or streaming. They do not
vary FFmpeg's internal thread count or test mixed codecs. These results therefore cannot establish
mixed-workload responsiveness or justify a universal worker default. Native Windows Job Object and
Android process-tree execution were not measured.

Reproduce using `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_codec_matrix.py
--workers 1 2 4 --releases 8 --seconds 60 --repeats 2 --codecs mp3 flac --output <new-path>`.
The harness creates and removes its own temporary media and refuses to overwrite the output file.
