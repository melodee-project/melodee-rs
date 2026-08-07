# Test Strategy

Tests are organized by risk and evidence type rather than by implementation framework.

- `python/` — tests for repository and Phase 0 tooling.
- `compatibility/` — differential/golden protocol tests.
- `media-corpus/` — representative and malformed media/metadata fixtures.
- `fault-injection/` — crash, retry, lease, filesystem, database, and child-process failures.
- `performance/` — reproducible catalog, ingestion, search, browse, and stream benchmarks.

The lowest useful test is required, but external contracts and side-effect workflows also require
integration evidence. “Unit tests passed” is not proof of client or crash compatibility.
