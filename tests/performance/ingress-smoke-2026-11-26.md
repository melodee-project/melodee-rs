# HTTP ingress harness smoke, 2026-11-26

This is a functional smoke test of the real HTTP benchmark, **not** evidence
for a production connection limit. It used the dirty working tree based on
`08de08d50cb1be6e76c66000232dae257c1a2d2f`; the exact source state of the
release binary was not independently captured. The binary SHA-256 was
`2b3149d7a28213fc12e427923c7523fd8e64bd36d3cc92250d271f4cc5fe7913`.

The invocation-owned PostgreSQL 18.3 fixture contained 10 Artists, 20 Releases
and 40 Tracks before the benchmark generated and ingested one valid, synthetic
30-second MP3. The source media was 721,798 bytes and the temporary media
root was on tmpfs. The host exposed 24 logical CPUs; PostgreSQL used 128 MiB
`shared_buffers`, 4 MiB `work_mem` and 100 maximum connections. Cache state was
uncontrolled. The process used a cap of four HTTP connections, one slow stream,
two Jellyfin stopped requests, two authenticated `/System/Info` requests, and
an explicitly held Track row lock for 0.5 seconds.

- The stream returned HTTP 200; time to headers was 0.003793 seconds. The
  client consumed 25,600 bytes, then deliberately closed the stream.
- Both Jellyfin stopped requests returned HTTP 204 and the fixture contained
  both distinct play events. Neither request finished while the Track was
  locked. Request durations were 0.598387 and 0.528135 seconds.
- Both system-info requests returned HTTP 200 and finished while the Track was
  locked; durations were 0.000675 and 0.000514 seconds.
- The scenario elapsed 0.598628 seconds. At the end, the server process reported
  31,432 KiB peak RSS and 0.08 seconds CPU time; child process resources were
  not included.

The sample is too small to establish that the connection cap saturated or to
infer tail latency. The workload uses an artificial row lock, loopback HTTP/1,
one temporary synthetic stream, no reverse proxy, no concurrent ingestion or
rebuild after setup, and does not measure audible playback underruns. It did not
compare a baseline or determine a safe connection limit.

The updated harness was re-smoked with the same binary and cardinalities after
adding one authenticated idle keep-alive connection, exact popularity checks and
request-error reporting. Two Jellyfin requests returned 204 and persisted two
distinct plays; Track popularity also equaled two. No play finished during the
0.5-second Track lock; both system-info probes finished during that lock. Play
durations were 0.528688 and 0.614659 seconds; probe durations were 0.000463 and
0.000990 seconds. The stream returned 200 with 0.004145-second time to headers
and 26,624 bytes consumed. Server peak RSS was 31,392 KiB and CPU time was
0.09 seconds. Request scheduling was not controlled; these samples do not
prove saturation or a safe production cap.

Reproduce after building the debug migration CLI and release server:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo build --locked --release -p melodee-server --bin melodee-server
python3 scripts/perf_representative.py --artists 10 --releases 20 --tracks-per-release 2 -- python3 scripts/perf_ingress_workload.py --connection-limit 4 --plays 2 --probes 2 --hold-seconds 0.5 --media-seconds 30
```

The fixture creates and removes its own UUID-named database; the workload owns
its server and temporary media. The output contains only fixture metadata and
timings, never the synthetic access token or database credentials.
