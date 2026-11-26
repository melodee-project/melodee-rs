# Media normalization stage codec matrix, 2026-11-26

These are **production media normalization function measurements**, not
end-to-end ingestion or a worker-admission qualification. Raw samples and source
hashes are in [baseline JSON](codec-matrix-baseline-2026-11-26.json) and
[final JSON](codec-matrix-final-2026-11-26.json). The final sample finished at
19:53 UTC and the baseline at 19:57 UTC; they were run serially in that order.
The latter used an isolated checkout of `a0979cde1fd2aeff5b8027d9d59336b678f1aa12`.
The former used a dirty working tree based on `8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`;
raw evidence records SHA-256 hashes of the media sources, benchmark, and lockfile.
The same harness was copied into the baseline as an untracked test without
changing its production sources, then the copies were removed. Source digests
for the final run match the working tree at collection time.

Both runs used a release build, FFmpeg 9.0.2, fpcalc 1.6.1, and an AMD Ryzen 9
3900X (24 logical CPUs) on Linux. Each case uses eight synthetic, single-track
Releases, 60 seconds of mono sine-wave source audio, two samples, warm reuse of
the same source file, and the unmodified production FFmpeg options. The table
shows mean Releases/s across the two samples and final sampled peak process-tree
RSS (MiB, maximum of two samples). RSS sampling includes pre-timing inspection
and can miss short-lived peaks. CPU seconds, individual latencies, source and
output sizes and block counts are recorded in the JSON.

| Input | Workers | Baseline releases/s | Final releases/s | Final sampled peak RSS MiB |
|---|---:|---:|---:|---:|
| MP3 | 1 | 1.049 | 1.641 | 76.8 |
| MP3 | 2 | 1.920 | 3.530 | 140.1 |
| MP3 | 4 | 3.908 | 6.649 | 206.4 |
| FLAC | 1 | 0.928 | 1.429 | 74.6 |
| FLAC | 2 | 1.879 | 2.848 | 143.3 |
| FLAC | 4 | 3.564 | 5.476 | 280.4 |
| Ogg Vorbis | 1 | 0.937 | 1.453 | 73.7 |
| Ogg Vorbis | 2 | 1.797 | 2.940 | 141.9 |
| Ogg Vorbis | 4 | 3.492 | 5.738 | 277.3 |
| WAV | 1 | 0.915 | 1.431 | 73.4 |
| WAV | 2 | 1.789 | 2.875 | 141.1 |
| WAV | 4 | 3.708 | 5.540 | 277.0 |

The observed difference is not a causal attribution to a particular change.
These small, synthetic, sequentially ordered samples do not establish an
interactive latency budget or prove a worker default is safe. No PostgreSQL,
HTTP, full ingestion scheduling, acquisition, publication, mixed codecs, or
concurrent interactive traffic is included; the test does not exercise the
shared ingestion admission gate. FFmpeg's thread count was not varied. The
corresponding production-shaped workload, resource admission coverage and a
codec/thread matrix under mixed load remain required. See the
[benchmark instructions](README.md#media-codec-and-worker-matrix).
