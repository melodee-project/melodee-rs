# ACL provenance on representative catalog, 2026-11-26

The release-profile `perf_interactive_routes` test ran against an
invocation-owned PostgreSQL fixture containing 35,000 artists, 50,000 releases,
and 600,000 tracks with primary credits and synthetic ratings. After the other
route samples, the shared catalog test helpers built two additional releases
through real manifest, normalization, publication and catalog-import paths,
using an invocation-owned temporary asset root. The test bound each additional
release to a separate library group, granted the synthetic non-admin user
membership in one group, verified HTTP 200 for the allowed release and HTTP 404
for the denied release, revoked membership, and verified HTTP 404 for the
formerly allowed release.

Only these two additional releases have the full provenance and bound ACLs; the
50,000 synthetic catalog releases lack import provenance and remain
unrestricted. These single-user, in-process detail checks do not establish
restricted catalog-list performance, concurrent revocation behavior, TCP
latency, production-scale ACL distributions, percentiles, CPU or RSS. Four
serial samples per route are shown including warm-up, with uncontrolled cache
state. Native authentication was injected. Admin routes used a signed bearer
credential and Jellyfin login occurred outside timed samples. Output assertions
check response shapes, headline counts, one rating and the ACL behavior, not
full golden equivalence.

Environment: AMD Ryzen 9 3900X (12 cores/24 threads), 62 GiB RAM, Linux
7.3.4 Fedora x86_64, NVMe-backed btrfs with zstd compression; PostgreSQL 18.3,
shared_buffers 128 MiB, work_mem 4 MiB, max_connections 100. Rust 1.99.0 release
profile and an eight-connection benchmark pool. The benchmark executable was
built from a dirty working tree based on `8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`;
concurrent edits occurred, and the exact source snapshot was not recorded.
It is not a final-commit or before/after comparison.

Reproduce after building the migration CLI:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-server --test perf_interactive_routes --no-run
python3 scripts/perf_representative.py --seed-timeout 900 -- cargo test --release -p melodee-server --test perf_interactive_routes -- --ignored --nocapture
```

Raw output:

```text
Seeded 35000 artists, 50000 releases, 600000 tracks in 178.4s

running 1 test
route=native_artists iteration=0 elapsed_ms=11.422 bytes=10260
route=native_artists iteration=1 elapsed_ms=10.171 bytes=10260
route=native_artists iteration=2 elapsed_ms=7.377 bytes=10260
route=native_artists iteration=3 elapsed_ms=6.670 bytes=10260
route=native_releases iteration=0 elapsed_ms=10.791 bytes=21899
route=native_releases iteration=1 elapsed_ms=5.296 bytes=21899
route=native_releases iteration=2 elapsed_ms=5.660 bytes=21899
route=native_releases iteration=3 elapsed_ms=4.880 bytes=21899
route=native_tracks iteration=0 elapsed_ms=40.767 bytes=44980
route=native_tracks iteration=1 elapsed_ms=23.361 bytes=44980
route=native_tracks iteration=2 elapsed_ms=22.737 bytes=44980
route=native_tracks iteration=3 elapsed_ms=22.833 bytes=44980
route=native_suggest iteration=0 elapsed_ms=7.174 bytes=1841
route=native_suggest iteration=1 elapsed_ms=3.101 bytes=1841
route=native_suggest iteration=2 elapsed_ms=1.377 bytes=1841
route=native_suggest iteration=3 elapsed_ms=1.378 bytes=1841
rating_check=passed
route=admin_stats iteration=0 elapsed_ms=614.279 bytes=2746
route=admin_stats iteration=1 elapsed_ms=603.592 bytes=2746
route=admin_stats iteration=2 elapsed_ms=606.004 bytes=2746
route=admin_stats iteration=3 elapsed_ms=573.202 bytes=2746
route=admin_artist_search iteration=0 elapsed_ms=22.795 bytes=8275
route=admin_artist_search iteration=1 elapsed_ms=19.739 bytes=8275
route=admin_artist_search iteration=2 elapsed_ms=21.084 bytes=8275
route=admin_artist_search iteration=3 elapsed_ms=22.382 bytes=8275
route=jellyfin path=/Artists?limit=25 iteration=0 elapsed_ms=55.342 bytes=28754
route=jellyfin path=/Artists?limit=25 iteration=1 elapsed_ms=54.280 bytes=28754
route=jellyfin path=/Artists?limit=25 iteration=2 elapsed_ms=56.224 bytes=28754
route=jellyfin path=/Artists?limit=25 iteration=3 elapsed_ms=52.573 bytes=28754
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=0 elapsed_ms=32.557 bytes=30889
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=1 elapsed_ms=38.575 bytes=30889
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=2 elapsed_ms=32.019 bytes=30889
route=jellyfin path=/Items?includeItemTypes=MusicAlbum&limit=25 iteration=3 elapsed_ms=35.501 bytes=30889
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=0 elapsed_ms=155.499 bytes=60057
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=1 elapsed_ms=154.986 bytes=60057
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=2 elapsed_ms=145.217 bytes=60057
route=jellyfin path=/Items?includeItemTypes=Audio&limit=25 iteration=3 elapsed_ms=153.202 bytes=60057
route=native_acl_allowed iteration=0 elapsed_ms=1.319
route=native_acl_allowed iteration=1 elapsed_ms=0.681
route=native_acl_allowed iteration=2 elapsed_ms=0.672
route=native_acl_allowed iteration=3 elapsed_ms=0.663
route=native_acl_denied iteration=0 elapsed_ms=0.528
route=native_acl_denied iteration=1 elapsed_ms=0.986
route=native_acl_denied iteration=2 elapsed_ms=0.537
route=native_acl_denied iteration=3 elapsed_ms=0.652
acl_revocation_check=passed
test measure_interactive_routes ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.59s


```
