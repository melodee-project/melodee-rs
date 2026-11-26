# ADR-0213: Store full manifest evidence on persistent file storage

- **Date:** 2026-11-25
- **Status:** Accepted
- **Decision owners:** ingestion/catalog maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-007, PRD-INGEST-008, PRD-FP-001,
  PRD-FP-002, PRD-FP-003, PRD-CATALOG-005, PRD-STORAGE-001, PRD-PERF-001,
  PRD-PERF-006, PRD-MIG-001, PRD-MIG-002
- **Applies:** ADR-0210 (content-addressed store, installation binding,
  no-replace publication)
- **Uses:** ADR-0186 (fingerprint digest as the comparison key)

## Context

Production manifest rows occupied about 219 MiB for 4,424 work items. A sample
of 147 manifests contained 84 complete Blackbeard provenance documents, and
normalization also stores kilobyte-scale Chromaprint strings per track. The
manifest payload is a versioned, immutable evidence document, not a queryable
relation, yet it lived in PostgreSQL `jsonb` and was traversed by SQL on browse,
policy and compatibility paths.

PostgreSQL's storage rule in `AGENTS.md` is explicit: file payloads belong on
configuration-driven persistent storage; only references, sizes, media types,
content hashes, ownership and provenance belong in the database.

## Decision

1. **One logical manifest row, one verified reference.** The row keeps its ID,
   work-item association, `schema_version`, `manifest_hash` (semantic),
   sealed/semantic hashes, generation facts and lifecycle. Its payload is
   written as an immutable evidence object under
   `MELODEE__STORAGE__ASSET_ROOT`, using the ADR-0210 installation binding,
   content-addressed publication and SHA-256/length verification. The existing
   on-disk publication bundle (`manifest.json` in the publish layout) is
   unchanged.
2. **Evidence is the complete document, byte for byte.** Deterministic
   versioned JSON (`serialization_version = 1`) preserves Blackbeard provenance
   documents, raw fallback text, lyrics, parsed sidecars, unknown fields and
   full Chromaprint strings. Sidecars retain original bytes as read by the
   existing bounded, hash-verified read. No audio is re-read and no fingerprint
   is recomputed during upgrade.
3. **Compact query facts stay in PostgreSQL.** `release_work_item_manifest_facts`
   stores release title, source path, corrupt-media flag, structured/embedded
   release traits and a lyrics-present flag.
   `release_work_item_manifest_track_facts` stores per-track relative path,
   audio technical fields (bit depth, sample rate, channels), the BLAKE3
   fingerprint digest and its length, corrupt flag, genre and source ordinal.
   These are the only values browse, search, policy batches, genre projection
   and Admin listings read. The fingerprint comparison key is the digest
   (ADR-0186); the raw fingerprint lives in evidence.
4. **Evidence above 10 MiB is chunked.** The single-object limit and every
   upload limit stay as they are. Larger evidence is streamed in fixed 4 MiB
   parts, each an ordinary store object, and represented by an ordered,
   versioned descriptor containing chunk keys, total SHA-256 and length.
   Reconstruction verifies the descriptor version, every part digest and the
   overall digest/length. Every part is registered in `asset_files`, so shared
   parts survive one owner's deletion. A retry reuses immutable objects.
   `release_work_item_manifest_evidence_revision` is the append-only ownership
   history of superseded references; the store's documented retention policy
   applies (retain, report cost, never auto-delete).
5. **Verify, then bind.** Every writer publishes the object, reads it back from
   its final path, compares it with the serialized bytes, and only then inserts
   the reference, facts and receipt. Allowed pre-seal review edits replace the
   reference inside the existing transaction, fenced by the expected manifest
   hash and schema version; sealed v3 identity is preserved and the database
   trigger rejects any attempt to reintroduce an inline payload.
6. **Upgrade is expand → backfill → verify → finalize.**
   - Expand: migration `0059` adds the evidence, facts, receipts and progress
     tables and makes `manifest_payload` nullable. No inline bytes are removed.
   - Backfill: `melodee-cli migrate` (and a bare server start, through the same
     migrator gate) converts inline payloads in stable ID order with at most 50
     manifests per checkpoint and one large manifest in flight. Each row is
     stored, read back, compared with its source and only then receipted.
   - Verify: an independent pass reads every evidence file from its final path,
     checks SHA-256/length, parses it, compares reconstructed JSON against any
     remaining inline payload and compares recomputed facts against the stored
     facts. Any discrepancy stops finalization and preserves the source.
   - Finalize: inline payloads are cleared only after that check, per row, in a
     transaction fenced by the manifest hash.
   A repeat run over a completed installation performs no bulk work; a bare
   server start requires that every manifest has a verified reference and
   receipt and fails closed otherwise.
7. **Operator interface.** `melodee-cli migrate --preflight|--status|--verify`
   are read-only and safe before and after upgrade. `--verify` exits nonzero on
   a discrepancy. A populated SQL-only upgrade lacking verified receipts fails
   closed.
8. **Storage guard.** Payload markers are rejected from persisted columns by
   synthetic checks in the test suite, including fallback/error paths, caches
   and diagnostics. Cryptographic metadata (digests, salts, receipts) is
   permitted and unaffected.

## Alternatives considered

### Keep `jsonb` and add indexes

Does not remove the 219 MiB of evidence/TOAST, and every new reader adds
another payload traversal. Rejected by the repository storage rule.

### One row per evidence field

Would move the same bytes into more rows and destroy the document boundary the
sealed hash protects.

### Delete the inline payload during the schema migration

Irreversible if a file is corrupt or the store is mis-bound. Rejected: the
upgrade must preserve the recoverable source until an independent check passes.

## Rationale

The manifest is evidence with a lifecycle, not a relation. Files give it
immutability, deduplication and bounded database growth; facts give the hot
paths what they actually query. The expand/backfill/verify/finalize shape means
an interrupted upgrade is always recoverable and a completed one is provable.

## Consequences

### Positive

- Manifest evidence no longer grows PostgreSQL TOAST.
- Hot paths (browse, search, policy, compatibility) read bounded indexed facts.
- Full evidence, including provenance and raw fingerprints, remains available
  to authorized detail paths and replay.
- The paired database/store can be verified independently before inline bytes
  are dropped.

### Negative / trade-offs

- A database backup alone no longer reconstructs manifests; the asset store is
  part of the backup pair (ADR-0210).
- Full-manifest reads (catalog import, lyrics detail) require a filesystem read.
- Superseded evidence is retained and reported, not reclaimed automatically.
- The upgrade is I/O-bound and must run with old writers stopped.

### Follow-up work

- Operator-driven reclamation of superseded store objects (the store keeps the
  retention policy; this ADR only reports cost).
- Streaming parse for very large manifests if a future schema needs it.

## Validation

- `crates/melodee-persistence/tests/manifest_evidence.rs`: chunked >10 MiB
  round trip, receipt mismatch rejection, shared-part reuse, import writing
  evidence + facts with no inline payload, genre projection from facts, legacy
  inline upgrade with unknown fields and non-ASCII lyrics, idempotent repeat
  run, missing/corrupt file detection, and database-level rejection of
  reintroduced inline evidence.
- `crates/melodee-persistence/tests/migrations.rs` (all green) exercises the
  full migration chain, including the historical-schema import path.
- Catalog, policy, Native and OpenSubsonic suites assert preserved values after
  the readers moved to facts/evidence.
- Storage measurements and byte counts: see
  `design/evidence/storage/ENRICHMENT-AND-STORAGE-CLOSEOUT.md` (recorded with
  the change).

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0210](./ADR-0210-persist-binary-assets-in-a-content-addressed-filesystem-store.md)
- [ADR-0186](./ADR-0186-durable-acoustic-identity-evidence-and-exact-fingerprint-novelty-matching.md)
- [ADR-0165](./ADR-0165-materialize-catalog-genre-serving-facts.md)
