# ADR-0201: Import available user state as the catalog grows

- **Date:** 2026-11-15
- **Status:** Accepted
- **Decision owners:** Migration tooling, CLI
- **Supersedes:** ADR-0200 decision 5 catalog-gap failure default only
- **Superseded by:** None
- **Related requirements:** PRD-MIG-002, PRD-MIG-003, PRD-USER-001, PRD-PLAYLIST-001

## Context

The administrator's intended workflow is to export listener state once, import the rows for music
already available, and run the same import again after more music arrives. ADR-0200 instead made
missing catalog entities a fatal error unless every invocation included `--allow-unmatched`.
A partial catalog consequently prevented all matched likes, dislikes, play counts, and history
from being imported. The dry-run state count also included unresolved catalog rows, while missing
playlist items and bookmarks were not reported consistently.

The product decision is to make incremental catalog coverage the ordinary import workflow. The
identity and merge rules of ADR-0200 remain in force; a catalog gap never authorizes name matching.

## Decision

1. `users import-state --input DIR` skips rows whose catalog entity is missing or not live and
   reports unmatched counts for entity states, play history, playlist items, and bookmarks.
   Human-readable output explains that rerunning the same export retries these rows after more
   music is available. The export remains unchanged; no pending-state table is introduced.
2. `--dry-run` follows the same matching policy and counts only resolved rows in its plan. It
   never creates accounts or writes user state.
3. `--require-all-matches` restores strict catalog validation for administrators who require a
   complete catalog. Every catalog-bearing stream is checked before creating any accounts or
   applying state. It conflicts with `--allow-unmatched`.
4. `--allow-unmatched` remains accepted for existing commands. Catalog gaps already skip by
   default; this flag additionally permits skipping missing users with `--no-create-users`.
   Missing accounts are still created unprivileged by default.
5. The same export is resolved afresh on every invocation. Newly available music receives its
   state; existing rows retain the additive/idempotent merge rules. Newer local likes and
   dislikes survive replay, and play counts and last-played times remain monotonic maxima
   independently of the export row's modification time. Equal-time flag conflicts favor local
   state, as specified by ADR-0200.

## Alternatives considered

### Require `--allow-unmatched` for every partial catalog

Rejected: it makes the principal incremental workflow look like an error and prevents an ordinary
read-only preview from showing the matched rows.

### Match missing tracks by artist and title

Rejected: names do not establish Melodee identity. Preserved public IDs and live catalog status
remain mandatory under SPEC-INV-001 and SPEC-INV-003.

### Persist unresolved user state in the target database

Not needed for this workflow: the administrator retains the export and explicitly reruns it.
Keeping the export as the retry source avoids schema changes and automatic application of old
state when music arrives.

## Consequences

- Partial catalogs can receive useful listener data immediately, with visible unmatched counts.
- Complete-catalog checks require `--require-all-matches`; existing `--allow-unmatched` commands
  continue to work.
- A catalog with unrelated public IDs can report all rows unmatched. Operators must review the
  summary or use strict mode; the importer never guesses identity.
- No schema, format version, public API, credential, privilege, or media-storage change is needed.

## Validation

`apps/melodee-cli/tests/user_state_import.rs` exercises a mixed active/missing/inactive catalog,
read-only planning, repeated imports, new music arriving between runs of the same export,
likes/dislikes/play counts, preserved local changes, deduplicated history, playlist additions,
bookmarks, and strict rejection of every catalog-bearing stream before account creation.

## References

- [ADR-0200](ADR-0200-portable-user-state-export-and-import.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [Public import guide](../../docs/src/content/docs/installation/user-state-import.md)
