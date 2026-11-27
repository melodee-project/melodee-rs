# HTTP ingress first-byte contention diagnostic, 2026-11-26

The release server exercised real HTTP/1 Jellyfin authentication, direct streaming,
and playback-stopped paths on an invocation-owned PostgreSQL fixture containing
35,000 Artists, 50,000 Releases and 600,000 Tracks. The server ingested one valid,
480-second generated MP3 before measurements. Each limit was tested once with one
authenticated idle keep-alive, one deliberately slow stream, 16 stopped requests and
16 authenticated `/System/Info` requests. A transaction held the synthetic Track
row lock for three seconds. The client requested a 4,096-byte socket receive buffer
and consumed the stream slowly before closing it early.

The nearest-rank p95 values use **16 requests** each; p95 and p99 are the largest
sample and are not stable estimates of production tail latency. First-media-byte
time is a **single** observation per limit, measured after reading the first payload
byte. See [sanitized raw samples](./ingress-ttfb-600k-2026-11-26.json).

| Connection limit | Distinct plays persisted | Probes finished during lock | Stopped p95 | Probe p95 | First media byte | Peak server RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 (disabled) | 16 / 16 | 16 / 16 | 3.653507 s | 0.004768 s | 0.004519 s | 32,972 KiB |
| 4 | 16 / 16 | 0 / 16 | 3.126812 s | 3.117635 s | 0.004576 s | 31,276 KiB |
| 16 | 16 / 16 | 0 / 16 | 3.661334 s | 3.361548 s | 0.004297 s | 32,916 KiB |
| 32 | 16 / 16 | 16 / 16 | 3.626268 s | 0.003766 s | 0.004842 s | 33,756 KiB |
| 64 | 16 / 16 | 16 / 16 | 3.222981 s | 0.004099 s | 0.004036 s | 33,044 KiB |

All stopped requests returned 204; every case persisted 16 distinct plays and the
Track recommendation popularity equaled its committed plays. All streams returned
200. Caps 4 and 16 delayed all unrelated probes until the held lock was released.
Caps 32 and 64 did not in this particular sample; this does not establish a safe
general default. Any finite shared cap can be exhausted by idle or slow clients.
The earlier [separate run](./ingress-600k-2026-11-26.md) measured response headers
only; these are newly collected first-byte observations, not a correction of that run.

This was one host, one sample per cap, loopback HTTP/1, one synthetic held row lock,
one idle connection and one slow stream. It did not measure an actual player or
audible underruns, an external proxy, concurrent ingestion or a search rebuild.
The cache and request scheduling were uncontrolled. The fixture took 178.1 seconds
to seed; temporary storage was tmpfs, and ingestion was excluded from scenario time.
The host had an AMD Ryzen 9 3900X (24 logical CPUs), approximately 62.7 GiB RAM, and
PostgreSQL 18.3 with 128 MiB `shared_buffers`, 4 MiB `work_mem`, and 100 maximum
connections. Server resource samples include startup and authentication, not child
processes. The release-profile binary SHA-256 was
`9d7b9aafa85f0708b1eefd915c02a21dfb4dfd90686b8c0b087ddd2044bdbc13`;
the observed Git HEAD was `08de08d50cb1be6e76c66000232dae257c1a2d2f` with a dirty
working tree. Binary source identity was not independently established; this is not
a clean final-commit benchmark.

Reproduce after building the debug migration CLI and release server:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo build --locked --release -p melodee-server --bin melodee-server
python3 scripts/perf_representative.py -- python3 scripts/perf_ingress_workload.py --connection-limit 0 --connection-limit 4 --connection-limit 16 --connection-limit 32 --connection-limit 64 --plays 16 --probes 16 --hold-seconds 3 --media-seconds 480 --idle-connections 1
```

The harness verifies its owned database identity and sends requests only to its own
loopback listener. Temporary database, generated media, tokens and child processes
are owned and cleaned up by the invocation.
