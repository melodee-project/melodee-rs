# Interactive route diagnostic, 2026-11-26

Measured in-process on the dirty working tree based on commit
`8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`. Concurrent work occurred while
building; the exact source snapshot was not recorded. No before/after
comparison, TCP transport, concurrent load, percentile distribution, CPU,
RSS, or I/O resource measurements were collected. Four serial samples per
route were collected in the printed order, including the first warm-up sample;
cache state was uncontrolled.

The invocation-owned PostgreSQL fixture held 35,000 artists, 50,000 releases,
600,000 tracks, matching primary release and track credits, playable asset
references, synthetic artist/release/track ratings and synthetic acoustic
references. It has no physical media, production import/receipt/manifest
provenance, or ACL bindings. Native routing used an injected non-admin principal
and therefore did not measure native authentication; admin endpoints used a
synthetic signed bearer credential; Jellyfin used an actual login, outside the
timed samples. Rating of a non-admin user was independently checked against the
database. Response checks verified success and populated shapes, plus exact
headline counts, rather than complete golden equivalence. No restricted ACL
or revocation behavior was exercised.

Environment: AMD Ryzen 9 3900X (12 cores/24 threads), 62 GiB RAM, Linux
7.3.4 Fedora x86_64, NVMe-backed btrfs with zstd compression; PostgreSQL 18.3,
shared_buffers 128 MiB, work_mem 4 MiB, max_connections 100. Rust 1.99.0 release
profile, `CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=1`. The PostgreSQL settings
are captured on the same server; runtime cache conditions were not controlled.

Reproduce with an already built debug migration CLI and isolated test server:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-server --test perf_interactive_routes --no-run
python3 scripts/perf_representative.py --seed-timeout 900 -- cargo test --release -p melodee-server --test perf_interactive_routes -- --ignored --nocapture
```

Raw output:

```text
Seeded 35000 artists, 50000 releases, 600000 tracks in 180.0s

running 1 test
route=native_artists iteration=0 elapsed_ms=10.856 bytes=10260
route=native_artists iteration=1 elapsed_ms=6.298 bytes=10260
route=native_artists iteration=2 elapsed_ms=5.045 bytes=10260
route=native_artists iteration=3 elapsed_ms=4.974 bytes=10260
route=native_releases iteration=0 elapsed_ms=7.301 bytes=21899
route=native_releases iteration=1 elapsed_ms=4.119 bytes=21899
route=native_releases iteration=2 elapsed_ms=3.932 bytes=21899
route=native_releases iteration=3 elapsed_ms=3.927 bytes=21899
route=native_tracks iteration=0 elapsed_ms=36.894 bytes=44980
route=native_tracks iteration=1 elapsed_ms=22.937 bytes=44980
route=native_tracks iteration=2 elapsed_ms=22.092 bytes=44980
route=native_tracks iteration=3 elapsed_ms=24.980 bytes=44980
route=native_suggest iteration=0 elapsed_ms=7.129 bytes=1841
route=native_suggest iteration=1 elapsed_ms=3.203 bytes=1841
route=native_suggest iteration=2 elapsed_ms=1.469 bytes=1841
route=native_suggest iteration=3 elapsed_ms=1.380 bytes=1841
rating_check=passed
route=admin_stats iteration=0 elapsed_ms=616.489 bytes=2746
route=admin_stats iteration=1 elapsed_ms=585.636 bytes=2745
route=admin_stats iteration=2 elapsed_ms=603.894 bytes=2746
route=admin_stats iteration=3 elapsed_ms=556.045 bytes=2746
route=admin_artist_search iteration=0 elapsed_ms=20.220 bytes=8275
route=admin_artist_search iteration=1 elapsed_ms=20.304 bytes=8275
route=admin_artist_search iteration=2 elapsed_ms=23.803 bytes=8275
route=admin_artist_search iteration=3 elapsed_ms=22.612 bytes=8275
route=jellyfin path=/Artists?limit=25 iteration=0 elapsed_ms=61.136 bytes=28754
route=jellyfin path=/Artists?limit=25 iteration=1 elapsed_ms=53.260 bytes=28754
route=jellyfin path=/Artists?limit=25 iteration=2 elapsed_ms=52.296 bytes=28754
route=jellyfin path=/Artists?limit=25 iteration=3 elapsed_ms=51.511 bytes=28754
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=0 elapsed_ms=33.202 bytes=30889
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=1 elapsed_ms=39.075 bytes=30889
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=2 elapsed_ms=32.284 bytes=30889
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=3 elapsed_ms=36.246 bytes=30889
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=0 elapsed_ms=156.937 bytes=60057
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=1 elapsed_ms=161.744 bytes=60057
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=2 elapsed_ms=155.190 bytes=60057
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=3 elapsed_ms=156.475 bytes=60057
test measure_interactive_routes ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.74s


```
