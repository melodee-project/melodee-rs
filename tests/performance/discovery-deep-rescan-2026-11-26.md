# Deep snapshot rescan characterization

Measured 2026-11-26 with a release-profile test build at Git HEAD
`08de08d50cb1be6e76c66000232dae257c1a2d2f` plus uncommitted changes. The
actual binary SHA-256 was
`679f7c3c48c3b696eeda51bef14472d71c9c58518ff54f69a91f0c2f47566f7c`.

Command: `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --locked --release
-p melodee-ingestion --lib snapshot::tests::characterize_deep_directory_rescan
-- --ignored --nocapture`.

The production snapshot visitor traversed 17 nested directories and 1,000
one-byte files in the deepest directory: 1,017 metadata entries in **225 ms**
(one sample). The test creates the fixture immediately before traversal, so
metadata caches were warm. This specifically exercises the fallback that
reopens and rescans deep directories to bound retained descriptors. Its work
grows quadratically with directory width; the single sample does not establish
acceptable performance for wider directories or slower storage.

Host: AMD Ryzen 9 3900X, 24 logical CPUs, Linux 7.3.4, glibc 2.44; disposable
fixture on `/tmp` tmpfs. No PostgreSQL, media decoding, worker scheduling,
network, or concurrent interactive workload was exercised. This is not an
end-to-end ingestion measurement.

Source SHA-256 at measurement: `snapshot.rs`
`492f39795ccafbf96a2c3c26822604648ec6401ac019b031d400466d69f5284d`;
`reconcile.rs` `c5bf39aa3f69bf5cc07fefcd5f86faec12bbfd2681c439a0a9b01acb3d8098ea`;
`source.rs` `c89dc8738dee7ec8238a7c152b566b5588825da5699f8b1ff8cea6e91633cdd6`;
`source_windows.rs` `4e4e14825353484c400f14073d095bd53e7e86091b6416f1ddcaf98d69577d00`;
`Cargo.lock` `10225de828b200cd4845074d031571d21ecd352a53ad69bc0b45098f099654b8`.
