# ADR-0206: Allow confirmed administrator and editor deletion in Release Review

- **Date:** 2026-11-24
- **Status:** Accepted
- **Decision owners:** Melodee maintainers, following the operator's Release Review decision
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-POLICY-007, PRD-INGEST-010, PRD-API-ADMIN-002, PRD-UI-008

## Context

Validation can send a Release to review before catalog import creates a Release identity and
policy decision. The review page offered Delete at this stage, but the API rejected every
confirmation. Administrators and editors need to discard such a Release even when classification
is incomplete and they have no note to add.

## Decision

An enabled administrator or editor can explicitly delete a work item awaiting review. The UI
shows its impact and requires the displayed confirmation phrase. The API requires a deliberate
override and checks the caller's current role. An explanation is optional. Editors can open
the review queue, inspect evidence and confirm Delete. Other review decisions, reprocessing
and general administration keep their existing authorization.

Before catalog import, Melodee records the actor, role, optional note, time and managed paths
against the work item. Only its normalization workspace, retained staging tree and a canonical
bundle with a publication receipt qualify for cleanup. A proposed publication destination and
a historical inbound path do not establish ownership. The work item reaches Deleted after
cleanup. A failure leaves the original authorization available for retry and blocks conflicting
review decisions. After catalog import the existing policy decision, sealed fingerprint,
tombstone and projection removal flow applies. Its audit records the actual confirming role.

## Alternatives considered

Requiring a catalog policy decision would force an unwanted Release through import. Relying on a
typed reason would exclude deliberate decisions without a note. The authenticated actor,
explicit confirmation and durable authorization identify the decision in either case.

## Consequences and rollout

Migration 0050 adds pre-catalog authorization records and permits editor actors on Delete
decisions and their audit snapshots. It leaves existing rows intact. Apply it before starting the
new server. For rollback after use, stop writers and restore a coordinated pre-upgrade database
and media backup. An older server cannot interpret editor decision actors, and a database
rollback alone cannot restore removed audio.

## Validation

The HTTP integration tests exercise listener denial, editor and administrator confirmation,
missing notes, interrupted cleanup, retry, original-actor attribution and retention of unrelated
files. The review screen tests exercise typed confirmation and optional notes.

## References

- [Confirmed Release deletion with audited tombstones](ADR-0152-execute-confirmed-release-deletion-with-audited-tombstones.md)
- [SPEC.md](../../SPEC.md)
