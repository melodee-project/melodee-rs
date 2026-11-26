# Representative background catalog, ingestion checkpoint

This is a checkpoint measurement, not final remediation acceptance or a before/after comparison. The raw per-item transition events and workload parameters are in [the sanitized log](ingestion-600k-checkpoint-8a4cf6f.log). The server executable was from a dirty worktree based on `8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`, SHA-256 `c7a1405446d0f568c0497233fce2df0ba0ab941428685d7e3e1c4d33be77257d`. It was supplied from `target/release`; its source correspondence and build profile were not independently verified, and it predates the unfinished migration 0065. Do not treat it as the final implementation.

On 2026-11-26 the fixture seeded 35,000 artists, 50,000 releases and 600,000 database tracks in 153.2 seconds. The server then discovered, inspected, normalized, validated, published and cataloged 200 of 200 generated releases in 60.656 seconds, approximately 197.8 releases/minute and 3.30 tracks/second. Timing includes server startup, worker polling and shutdown; it excludes fixture seeding and corpus generation. Each release had one five-second synthesized tone, with retained MP3 and transcoded FLAC alternating. The 200 audio files totaled 12,587,640 bytes. This short, synthetic workload is not representative of the length, complexity, or total bytes of ordinary albums.

State residence times below are derived from successive durable transition timestamps, for 200 samples per state. They include queueing and polling, not just execution time; `normalizing`, for example, includes waiting to be claimed by a normalization worker. The percentile is the observed sorted sample at index `floor((n - 1) × 0.95)`.

| State | p50 (seconds) | p95 (seconds) |
| --- | ---: | ---: |
| settling | 3.834 | 3.834 |
| ready | 10.353 | 19.665 |
| inspecting | 0.168 | 0.188 |
| normalizing | 17.602 | 31.504 |
| validating | 0.489 | 0.953 |
| staged | 0.793 | 1.060 |
| publishing | 0.038 | 0.162 |
| published | 0.964 | 1.003 |
| cataloging | 0.012 | 0.024 |

The host reported an AMD Ryzen 9 3900X, 24 logical CPUs, 65,727,588 KiB memory, x86-64 Linux 7.3.4 and glibc 2.44. The generated inbound, staging, work and canonical directories were on `/tmp` tmpfs, so these figures are **not disk-backed media performance**. Each ingestion stage had concurrency 2. Cache state was uncontrolled. PostgreSQL settings queried after the run on the development server were version 18.3, `shared_buffers=128MB`, `work_mem=4MB`, `max_connections=100` and `synchronous_commit=on`; they were not independently sampled during the measurement. The fixture and generated directories were invocation-owned and cleaned up.

Search, policy, discovery, enrichment and podcast workers were disabled. The run did not exercise concurrent interactive activity, streaming, reimport, artwork, CUE, cross-filesystem acquisition, retry or fault recovery. It collected no syscall, hash, read/write, probe, subprocess, resource-use or per-stage execution counters. The background catalog is synthetic and does not provide 600,000 physical files. No baseline or final comparable run was collected. These omissions prevent using this result as evidence that RM-00, RM-11, or the overall performance remediation is complete.
