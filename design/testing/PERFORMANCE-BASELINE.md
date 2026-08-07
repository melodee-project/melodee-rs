# Performance Baseline Specification

Measure the current C# server before choosing targets or declaring improvements.

Required scenarios:

- discover and settle 200 inbound Releases;
- normalize representative MP3/FLAC/OGG/WAV Releases;
- no-op scan of an unchanged large tree;
- database insertion/catalog update;
- Artist/Release/Track browse pages;
- native, OpenSubsonic, and Jellyfin search;
- stream start, seek/range, and concurrent streams;
- Tantivy candidate indexing/rebuild once implemented.

Report p50/p95/p99 where meaningful, throughput, errors, CPU, RSS, disk/network bytes, database query
count/time, filesystem entries touched, and cold/warm-cache distinction. Targets in the PRD are gates,
not permission to omit the legacy baseline.
