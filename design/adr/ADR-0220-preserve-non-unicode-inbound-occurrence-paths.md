# ADR-0220: Preserve non-Unicode inbound occurrence paths

- **Date:** 2026-11-26
- **Status:** Proposed
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-002, PRD-PERF-003, PRD-SCALE-002

## Context

Inbound discovery previously converted filesystem paths with lossy Unicode
conversion. Distinct byte names could therefore share one work item, and the
staging worker could not reopen the original occurrence. Discovery also permits
an alias to a canonical nested directory within the source, while acquisition
previously rejected all nested paths. Filesystem paths identify occurrences,
not Artist or Release identities.

## Decision

Propose a reversible textual representation for non-Unicode occurrence paths.
Normalized relative Unicode paths keep their existing form. On Unix, an
otherwise non-Unicode relative path is represented by `//melodee-unix-hex:`
followed by lowercase hex of the complete native path bytes. Windows uses
`//melodee-windows-u16:` followed by lowercase four-digit UTF-16 code units for
unpaired-surrogate paths. The leading `//` is outside the normalized relative
path namespace. Decoders reject malformed encodings, alternate encodings of
valid Unicode, absolute paths and traversal before opening a source. Encodings
for a different operating-system path model fail closed. These values are path
metadata, not encoded file contents.

Acquisition may address canonical nested paths within a configured source only
through no-follow parent traversal and the existing lease, generation, snapshot,
content-hash, and cleanup fences. Cleanup syncs the immediate affected parent.
A symlink replacing any parent must not redirect acquisition or cleanup outside
the configured source.

Old lossy rows cannot be uniquely reconstructed when multiple native paths map
to the same text. During discovery, a matching old lossy value is conservatively
marked seen while new byte-distinct occurrences receive independent work items.
No automatic remapping or deletion of the old row is authorized; an operator
must inspect historical handoff evidence before resolving ambiguity. Existing
Unicode source-path strings and identities are unchanged. The diagnostic
`source_path` value for a newly observed non-Unicode occurrence changes from
lossy text to its encoded representation. This operator-visible change and
client compatibility require explicit final acceptance before this ADR is
marked accepted.

## Alternatives considered

Lossy conversion is not reversible and conflates occurrences. A new database
binary-object column is unnecessary for path metadata and would require wider
schema and wire-model changes. Percent encoding without a disjoint namespace
can collide with an ordinary valid filename. Rejecting all non-Unicode names or
all nested in-root aliases leaves previously discoverable inputs unprocessable.

## Rationale

A disjoint textual namespace preserves ordinary path strings while making new
native occurrence keys distinct and recoverable. Conservative treatment of
historical ambiguity avoids guessing which physical source an old work item
represented.

## Consequences

The encoded diagnostic value must be documented for operators and clients.
Non-Unicode occurrences cannot be resumed transparently on an operating system
with a different native path representation. Historical ambiguous rows remain
for review; no destructive automatic repair is provided. Snapshot entry names
inside a Release also require separate non-Unicode collision analysis. This
proposal does not establish cross-resource atomicity between filesystem root
verification and database commits.

The experimental custody proof uses a domain-separated `v2:` BLAKE3 digest
with length-delimited native path components, because the existing staging
receipt digest uses lossy path text and can conflate distinct native names.
The same descriptor-relative traversal can calculate both digests from the
same byte reads, preserving compatibility with existing staging receipts.
Neither matching digest establishes that a mutable tree remained unchanged
after traversal. The new custody protocol is not yet used for acquisition or
purge: isolated custody, repeated verification, root fencing, and crash recovery
must be implemented and validated before it is enabled. Existing legacy
receipts remain unchanged and are not sufficient custody identity evidence.

## Validation

Require distinct-invalid-byte and valid-Unicode collision tests; complete
production-path discovery, settling, staging, and receipt verification for
byte-distinct candidates; nested in-root alias acquisition; adversarial parent
symlink/root replacement and traversal tests; crash/retry and populated upgrade
coverage. Run Windows-native tests for unpaired-surrogate names and reparse
points. Review this proposal independently for security, compatibility and data
recovery before acceptance.

## References

- [ADR-0145](ADR-0145-drain-inbound-into-durable-staging-before-ingestion.md)
- [ADR-0217](ADR-0217-fence-and-bound-inbound-discovery-scans.md)
- [ADR-0218](ADR-0218-use-capability-directory-handles-for-windows-discovery.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
