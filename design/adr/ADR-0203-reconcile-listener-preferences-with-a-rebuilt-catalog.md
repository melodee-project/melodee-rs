# ADR-0203: Reconcile listener preferences with a rebuilt catalog

- **Date:** 2026-11-24
- **Status:** Accepted
- **Decision owners:** Migration tooling and catalog identity
- **Supersedes:** ADR-0200 decision 5 for explicitly approved public-ID links only
- **Superseded by:** [ADR-0204](ADR-0204-match-portable-listener-accounts-by-unique-email.md) for unique account emails and [ADR-0205](ADR-0205-match-rebuilt-tracks-by-exact-artist-release-and-track-title.md) for unique exact track matches
- **Related requirements:** PRD-MIG-002, PRD-MIG-003

## Context

[ADR-0200](ADR-0200-portable-user-state-export-and-import.md) and
[ADR-0201](ADR-0201-import-available-user-state-as-the-catalog-grows.md) resolve portable
listener data by public ID. That restores newly available preferences when a migration
preserves those IDs. An independent catalog rebuild assigns new IDs. The schema-v1 export
carries the old IDs and listener preferences but omits the descriptive information needed
to compare two recordings or editions with different IDs.

## Decision

Retain the original export. Keep a reusable record of confirmed correspondences between the
source and target public IDs, scoped by entity type and target installation. Resolve existing
accounts before creating missing ones. Link activity to an existing account without replacing
its credentials, privileges, or profile. Keep both catalogs' public IDs intact.

Use legacy catalog evidence and live target catalog evidence to find candidates. Present the artist,
release edition, track title, disc or track position, and duration where available. Distinguish
missing music from ambiguous candidates, such as multiple editions containing the same recording.
A metadata suggestion requires explicit operator confirmation. The review file records stable
public IDs. Unattended metadata-based approval is outside this decision. The default importer
continues to use exact public IDs.

Re-evaluate unresolved items on each run. Reuse confirmed links and the importer's existing
timestamp, play-count, and event deduplication rules. Report matched and unmatched preferences
by listener, plus positive legacy preferences superseded by equal-time or newer local choices.
List ambiguous candidates separately for review. The final check accounts for each source
preference, including a source favorite superseded by a newer local edit.

## Alternatives considered

The governed legacy catalog migration preserves public IDs but requires adopting the old catalog.
Repeated public-ID lookups alone cannot recover a preference for music assigned a new ID.
A title shared by a studio recording, live performance, and remaster can identify several candidates.

## Validation

Exercise a rebuilt catalog with different account and music IDs. Import one available release,
add another, and repeat the same export. Confirm the first listener's newer edits survive, history
does not duplicate, and a third run adds nothing. Exercise ambiguous editions, colliding usernames,
inactive releases, and a saved link that points to an unavailable target. A dry run must leave both
databases untouched.
