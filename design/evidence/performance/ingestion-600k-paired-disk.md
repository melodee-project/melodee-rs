# Disk-backed ingestion and catalog retry comparison

This is a controlled checkpoint, not final program acceptance. One baseline and one current workload ran sequentially on the same machine using the same harness, target cardinalities, per-stage concurrency and media-generation commands. Cache state was uncontrolled, the short generated files are not representative of album duration, and neither executable's exact source correspondence was independently verified. The current working tree was dirty during measurement; do not attribute these numbers to a final commit.

The baseline worktree was checked out at `a0979cde1fd2aeff5b8027d9d59336b678f1aa12`, with the measurement test added as an untracked file and a baseline-specific CLI used to migrate its disposable database. Its release-server binary SHA-256 was `98e48029c573ce369adace7cba474bfe37200abb50fa7023ea16b1cfb614f530`. The current repository's HEAD was `08de08d50cb1be6e76c66000232dae257c1a2d2f` with tracked changes at measurement time, and its server binary SHA-256 was `2b3149d7a28213fc12e427923c7523fd8e64bd36d3cc92250d271f4cc5fe7913`. The raw baseline log reports the coordinator repository HEAD, not the baseline worktree HEAD; the identities above disambiguate it. Both binaries were supplied from `target/release`; the harness did not independently verify their build profile or source correspondence.

The [baseline raw log](ingestion-600k-baseline-a0979cde.log) has SHA-256 `a32b7d16a0d3340aa473f9cf8ab4039dc647889df875a8d11bfb9427d6e7f329`; the [current raw log](ingestion-600k-current-disk.log) has SHA-256 `94ffe1630939cb06246380f3e3c574a977ae67ccaaadbea55f53da2a0e2899b4`. The logs contain every observed work-item transition. Both fixture databases and generated media directories were invocation-owned and cleaned up.

| Measurement | Baseline | Current |
| --- | ---: | ---: |
| Artists / releases / tracks in background fixture | 35,000 / 50,000 / 600,000 | 35,000 / 50,000 / 600,000 |
| Fixture seed time | 148.1 s | 149.9 s |
| Releases reaching cataloged | 200 / 200 | 200 / 200 |
| Workload elapsed time, one run | 96.771 s | 62.689 s |
| Releases/minute, derived | 124.0 | 191.4 |
| Tracks/second, derived | 2.07 | 3.19 |
| Same-item catalog retry p50 / p95, 50 samples | 3.067 / 4.121 ms | 3.081 / 4.109 ms |
| Same-item catalog retry maximum | 8.976 ms | 10.483 ms |

The observed total elapsed ratio is approximately 1.54, calculated from one run per version; it is not a statistically established speedup. Workload time includes server startup, queueing, polling and shutdown. It excludes fixture seeding, synthetic media generation and the separate retry measurement. Each release contained one five-second synthetic tone: alternating retained MP3 and transcoded FLAC, 200 tracks and 12,587,640 source bytes total in each run. Equal byte counts do not prove the generated corpora were byte-identical. Each ingestion stage used concurrency 2.

For each run, the direct catalog retry measurement repeatedly invoked the production `CatalogStore::import_manifest_v3` path for the **same cataloged work item**. All 50 calls retained its Release and Track IDs and did not add catalog import rows. The current run compared the complete outstanding identity seed set before and after; its cardinality was not recorded and it might have been empty. The baseline schema had no seed table. These measurements omit full ingestion, media reprocessing, SQL round-trip counts and filesystem read/hash counters, so they do not establish all unchanged-reimport or avoided-work obligations.

The following state-residence p50/p95 values are in seconds, with 200 samples per state. They measure time between successive durable transition timestamps and include queue and poll delays; they are not isolated stage execution times. The p95 is the sorted sample at index `floor((n - 1) × 0.95)`.

| State | Baseline p50 / p95 | Current p50 / p95 |
| --- | ---: | ---: |
| settling | 3.506 / 3.506 | 3.218 / 3.218 |
| ready | 35.565 / 65.331 | 11.925 / 22.626 |
| inspecting | 0.310 / 0.343 | 0.179 / 0.261 |
| normalizing | 11.416 / 21.241 | 18.247 / 31.474 |
| validating | 0.490 / 0.969 | 0.518 / 0.881 |
| staged | 0.982 / 1.018 | 0.863 / 1.029 |
| publishing | 0.054 / 0.064 | 0.052 / 0.184 |
| published | 0.936 / 1.456 | 0.912 / 0.991 |
| cataloging | 0.014 / 0.023 | 0.013 / 0.022 |

The host reported an AMD Ryzen 9 3900X, 24 logical CPUs, 65,727,588 KiB memory, x86-64 Linux 7.3.4 and glibc 2.44. Generated media, staging, working and canonical roots were under the workspace's `.local-dev` on a btrfs filesystem backed by `/dev/nvme0n1p1`, unlike the earlier tmpfs checkpoint. PostgreSQL configuration queried after the earlier run was version 18.3, `shared_buffers=128MB`, `work_mem=4MB`, `max_connections=100` and `synchronous_commit=on`; it was not independently sampled during each paired run. Baseline and current schemas differ as required by their migrations.

Search, policy, discovery, enrichment and podcast workers were disabled. Neither run exercised artwork, CUE, cross-filesystem acquisition, media fault recovery, streaming or concurrent interactive traffic. No resource-use, syscall, hash, read/write, probe, subprocess or SQL round-trip counters were collected. The 600,000-track background catalog was seeded in PostgreSQL and does not imply 600,000 physical audio files. These results do not establish the full RM-00, RM-11 or overall remediation acceptance criteria.
