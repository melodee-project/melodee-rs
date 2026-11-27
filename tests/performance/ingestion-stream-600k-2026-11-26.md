# Streaming and HTTP during 200-Release ingestion, 2026-11-26

A release-profile server imported 200 synthetic Releases while serving requests
on loopback HTTP/1 against an invocation-owned PostgreSQL fixture with 35,000
Artists, 50,000 Releases and 600,000 Tracks. Media lived on btrfs. The workload
first imported a separate playable 1,200-second synthetic MP3, then opened one
slow direct stream and exposed 200 prepared Release directories to discovery.
Each Release contained one five-second generated Track, alternating MP3 and
FLAC input. Inspection, normalization, validation and publication each had a
configured concurrency of two. All 200 work items reached `cataloged` in
60.900807 seconds, including worker polling; input preparation and the initial
stream-Track import are excluded. The batch contained 12,587,640 source bytes;
the direct-stream source contained 28,801,759 bytes.

During ingestion, one client repeatedly opened a direct stream, timed until the
first media byte, read 64 KiB, and closed it. Another repeatedly called the
authenticated Jellyfin `/System/Info` route. The table uses samples wholly
inside the window bounded by the first and last database polls that observed
unfinished work items (1.046410–60.383780 seconds after batch exposure). The
window can contain quiet gaps between polls; sample selection does not prove
each request overlapped active media processing. Percentiles use nearest rank.

| Request | Samples | p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: |
| Direct stream first media byte | 59 | 2.535 ms | 4.517 ms | 5.241 ms |
| System-info response | 236 | 0.549 ms | 0.718 ms | 0.894 ms |

No request failed among all 61 stream and 243 system-info samples. The separate
slow stream stayed open during the batch and delivered 1,232,897 bytes; its
first-byte time was 3.406 ms. This is not an audible-underrun measurement.
Server peak RSS was 55,916 KiB and server CPU time 7.57 seconds; these exclude
PostgreSQL and subprocesses. [Sanitized raw samples](./ingestion-stream-600k-2026-11-26.json)
include sample boundaries and all observed statuses.

The host had an AMD Ryzen 9 3900X with 24 logical CPUs and approximately
62.7 GiB RAM. PostgreSQL 18.3 had 128 MiB `shared_buffers`, 4 MiB `work_mem`,
and 100 maximum connections. Fixture seeding took 181.5 seconds. The cache
was uncontrolled. This is one run with synthetic short single-Track Releases,
no real player, external proxy, baseline comparison, search rebuild, or
concurrent search/policy/discovery/enrichment/podcast workers. It does not
establish production tail latency, stream quality or a safe admission limit.

The supplied release binary SHA-256 was
`9d7b9aafa85f0708b1eefd915c02a21dfb4dfd90686b8c0b087ddd2044bdbc13`.
Observed HEAD was `08de08d50cb1be6e76c66000232dae257c1a2d2f` and the tracked
working tree was dirty. Exact binary source identity was not independently
established, so this is not a clean final-commit result. The raw evidence also
records the harness SHA-256.

Reproduce with the prebuilt migration CLI and release server:

```text
python3 scripts/perf_representative.py -- python3 scripts/perf_ingestion_stream_workload.py --albums 200 --timeout 900 --media-seconds 1200 --media-parent .
```

The workload refuses databases not owned by its fixture and unknown or memory
filesystems. It creates a unique media tree under the selected disk-backed
parent, launches its own loopback server and child process group, and removes
its own temporary resources. It does not clear shared filesystem caches.
