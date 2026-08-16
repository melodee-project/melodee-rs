# Storage and Manifests

Canonical layout v1 is Release-centric and sharded by stable UUIDv7 Release ID. Database
relationships determine Artist views and contributions; Artist/title/source display values never
determine physical identity.

```text
<storage-root>/
  releases/
    <release-id-blake3-byte-0-hex>/
      <release-id-blake3-byte-1-hex>/
        rel_<release-uuid-simple>/
          manifest.json
          media/<blake3>.mp3
          artwork/<blake3>.jpg
```

Paths stored in PostgreSQL and manifest publication facts are storage-root-relative and portable.
The configured storage root is provenance, not a semantic hash input. The exact layout, hashing,
and migration consequences are accepted in ADR-0116.

A manifest is the durable boundary between media analysis and catalog insertion. It must be versioned,
self-describing, and sufficient to rebuild catalog/search state without re-probing every media file.

Manifest schema v1 records inspection; v2 adds normalization, fingerprints, normalized artwork, and
parsed sidecars; v3 adds the stable Release ID, structural validation, durable publication
authorization, retained source disposition, layout/plan identity, and a sorted canonical asset map.
V1/v2 remain readable and v2-to-v3 upgrade is explicit and tested. Required categories across the
evolving contract are:

- source and canonical file maps;
- media properties and tool versions;
- hashes/fingerprints;
- observed and resolved identities;
- Release/Track presentation data;
- contributions;
- classification evidence;
- validation and policy decision references; and
- schema/normalization versions.

## Publication boundary

PostgreSQL and the target filesystem are not one transaction. Before canonical I/O, PostgreSQL
stores an immutable authorization/plan. Publication then builds and verifies a deterministic hidden
incoming directory, fsyncs it, and exposes the complete Release with a kernel-enforced no-replace
rename. Only after final verification may PostgreSQL store the receipt and enter `Published`.

Retries resume a matching partial incoming directory or adopt an exact final. A different final is
a durable target conflict and is never removed or overwritten. The durable staged source is
retained; its move/delete requires a later, separate policy decision and audit. Under ADR-0145 the
inbound occurrence is drained after its verified durable staging receipt — inbound is a transient
mailbox, not retention storage.

See ADR-0106, ADR-0108, ADR-0115, ADR-0116, ADR-0145, and
[Phase 3 canonical-publication evidence](../evidence/phase-3/canonical-publication/README.md).
