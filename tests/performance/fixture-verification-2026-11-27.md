# Representative fixture verification, 2026-11-27

This is one **fixture-construction and integrity smoke**, not an application
performance benchmark or a before/after comparison. The owned database and
asset directory were removed by the fixture's cleanup. The run used dirty
intermediate source based on `37f29889fb05d1825faecb8b597387f0eb971b5e`;
the migration CLI was an earlier release build, not the final integrated
binary. Its SHA-256 was
`145ca42a7cd28a2b971a407072e2562f10591e8bb3e8f5767ff24d038f6628ad`.
The seeding script SHA-256 was
`104372c1539ec741e0b521fdaddee518d60b402f828c145344c7ca23a252fda6`.
The test file was updated during seeding, before the child test process was
started; the version actually run had SHA-256
`48c819926430b77f5a9794585e42ad84b603c8ad71bddf81f6223697f6a48881`.

Command, with the explicitly supplied prebuilt release migration CLI:

```text
python3 scripts/perf_representative.py --seed-timeout 900 --migration-cli target/release/melodee-cli -- python3 tests/python/test_perf_representative.py RepresentativeFixtureTests.test_seeded_ratings_cover_all_five_values RepresentativeFixtureTests.test_seeded_playable_and_alternate_assets RepresentativeFixtureTests.test_seeded_artwork_references_resolve_to_content_addressed_file RepresentativeFixtureTests.test_seeded_acl_exercises_allowed_and_denied_releases
```

The actual database counts were 35,000 Artists, 50,000 Releases and 600,000
Tracks, with 600,000 playable and 30,000 alternate media links. The four tests
passed: each of the three rating types contained values 1–5; both playable and
alternate links existed; Artist and Release artwork references pointed to a
file with the recorded SHA-256 and length; and two non-admin users each had
both allowed and denied Releases through the production ACL predicate. Seeding
took 183.2 seconds on this one run; that includes fixture construction and is
not import throughput. The fixture's generated names and distributions are
deterministic, but UUIDs are generated anew and there is no numeric random seed.

The host was the same 24-logical-CPU, 62-GiB Linux development machine used
for the intermediate identity measurements; this run did not record the
PostgreSQL server settings, storage latency or cache residency. It used a
synthetic shared PNG and database media references, not a physical audio
corpus. ACL publication and manifest records are placeholders, not receipts
from real imports. The test validates data shape, not real route latency,
representative identity history, worker behavior, or release acceptance.
Matched baseline/final end-to-end workloads and final-source evidence remain
required.
