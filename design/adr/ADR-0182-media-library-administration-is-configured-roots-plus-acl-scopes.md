# ADR-0182: media library administration is configured roots plus ACL scopes

- **Date:** 2026-11-09
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SETUP-001, PRD-WORK-001, PRD-API-M1-004, PRD-CONFIG-002

## Context

The C# server models each media library as a mutable database entity with a name, type
(Music/Podcast/Templates/Theme/User Images), filesystem path, scan flags and API key, and
ships `/data/libraries*` and `/media/library*` administration pages plus a
`SystemExportService` document that carries library rows next to settings.

The Rust replacement deliberately separates *storage topology* (deployment configuration:
canonical publication root, inbound sources, staging and work roots — validated for
absolute paths, overlap and symlink components at startup) from *access identity* (library
IDs used by group ACLs, stream fencing, and per-library browse/search predicates). It has
no library table.

The 2026-09-20 C# reconciliation classified the capability as `PARITY` with the note
"Admin UI/ACL/configuration". That overstated the boundary: the ACL and per-library
fencing are real, but a mutable library-entity CRUD surface (create a library, point it at
an arbitrary path, change its type) does not exist and is not planned as a runtime feature.

## Decision

1. Media storage topology is deployment configuration, not runtime state. The authoritative
   runtime roots are the canonical publication root, the configured inbound sources, the
   staging root and the work area, all resolved and validated at process start
   (`IngestionConfig`, `MELODEE__INGEST__*` / `MELODEE__STORAGE__*`).
2. Library *identity* for access control is a UUID carried by `native_library_group_libraries`.
   Group→library bindings, membership, and fail-closed per-library enforcement remain the
   supported runtime administration surface (`/api/admin/v1/groups/*`, Users & Access UI).
3. There is no library-entity CRUD endpoint, and no library name/type/path is accepted from
   an API caller. Adding a new media source is a configuration change plus a restart, not a
   database write.
4. Legacy library rows are preserved as ACL scope identifiers during migration. Their names
   and paths are **not** imported as runtime truth; the migration report records them as
   reference-only configuration evidence (ADR-0148). Library API keys were never an
   authentication mechanism on a captured surface and are not carried forward.
5. The legacy `SystemExportService` document's `libraries` array is imported as reference
   evidence only; settings import continues to carry the operator's settings.
6. This is an accepted, documented difference, not missing work: the parity matrix row is
   updated to `PARITY (approved difference)` and this ADR is the governing record.

## Alternatives considered

### Alternative A — Implement a mutable library entity

Benefits: literal feature-by-feature parity with the C# administration pages.

Costs and why not selected: a runtime-mutable library path reintroduces path-driven semantic
identity, which SPEC forbids ("filesystem paths never determine semantic identity"), makes
canonical publication roots operator-mutable at runtime (unsafe for in-flight publication
and immutability guarantees), and duplicates the validated storage-topology contract in two
places. It would also require a destructive-schema decision and a new migration with no
client-visible benefit: no current client enumerates libraries for user-facing behavior.

### Alternative B — Keep configuration-only roots and remove ACL library identities

Benefits: simplest model.

Costs and why not selected: library-scoped access restrictions are a shipped product
requirement (PRD lists user-group/library ACL as required) and the frozen native API exposes
library-aware filtering. Removing the ACL scope would be a real functional regression.

## Rationale

The rewrite's storage model is a single immutable canonical publication root with
configuration-defined inbound sources. That model is what makes publication idempotent,
public IDs stable, and the filesystem safe to treat as derived storage. Retaining library
*identity* only for authorization, while keeping topology in validated configuration,
preserves the required access-control behavior without reintroducing path-as-truth.

## Consequences

### Positive

- One validated storage-topology contract; no runtime path mutation.
- Library ACLs and per-library fencing remain fully supported and tested.
- Migration keeps library IDs for ACL bindings and reports the rest as reference-only.

### Negative / trade-offs

- An operator cannot add a library from the Admin UI; it is a configuration change and
  restart. This is the documented difference from the C# admin pages.
- The C# system-export `libraries` array is not imported as runtime state.

### Follow-up work

- Documentation states the configuration-and-restart model in the storage and Admin UI
  pages (`docs/src/content/docs/installation/storage.md`,
  `docs/src/content/docs/admin/users-and-access.md`).
- The parity matrix row cites this ADR instead of claiming runtime CRUD parity.
- Release-owner acceptance of the reference-only disposition stays tracked under the
  ADR-0150 follow-up (MEL-12/MEL-18).

## Validation

- `apps/melodee-server/tests/admin_access_api.rs` (group/library binding, fail-closed ACL),
  `native_stream_acl.rs`, `native_catalog_api.rs` ACL cases.
- `crates/melodee-persistence/tests/auth_phase7.rs` (ACL predicates).
- Legacy migration reference-only issue tests and the MEL-13 rehearsal report.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0148](ADR-0148-legacy-migration-contract.md)
- [ADR-0150](ADR-0150-legacy-media-metadata-dispositions-for-phase-11-migration.md)
- [Feature parity matrix](../requirements/FEATURE-PARITY-MATRIX.md)
