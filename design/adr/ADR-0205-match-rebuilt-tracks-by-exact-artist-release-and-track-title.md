# ADR-0205: Match rebuilt tracks by exact artist, release, and track title

- **Date:** 2026-11-24
- **Status:** Accepted
- **Decision owners:** Migration tooling and homelab operator
- **Supersedes:** [ADR-0203](ADR-0203-reconcile-listener-preferences-with-a-rebuilt-catalog.md) for unique exact track matches
- **Superseded by:** None
- **Related requirements:** PRD-MIG-002, PRD-MIG-003

## Context

Reprocessing media into a clean catalog assigns new Melodee public IDs. Listeners expect their
ratings and favorites to follow the recording on its original release. Confirming every uniquely
identified track by hand would repeat the same decision across thousands of preferences.

## Decision

For a portable listener import, prefer saved links and preserved public IDs. For remaining
tracks, compare the exported release artist, release title, and track title with tracks on live
releases. Normalize Unicode to NFKC, lowercase, trim, and collapse whitespace in PostgreSQL on
both sides. All three values must be present. Accept a pair only when exactly one referenced
source and one live target have the triple and the target is available for that source.
Edition and version text remain significant. Track number and duration are additional clues for
human review. Artist-only and release-only links retain the review policy of ADR-0203.

On apply, record the two public IDs in the export directory, scoped to the target installation,
before writing listener data. Load the same binding on retries even if titles change or another
edition appears. A bound target that becomes unavailable leaves its rows pending. Preserve newer
local listener data and original source IDs for play-event deduplication. The dry run reads the
catalog and reports the same matches without saving pairs or activity.

## Consequences

A sole live edition can receive a match before another edition with identical tags is ingested.
Operators who already know of a different intended target can supply a reviewed pair before the
first apply. Ambiguous triples require review. Missing tracks can match on a later run using the
same export. Portable identity maps and their backups are part of repeatable imports.
The catalog schema and external client protocols are unaffected.

## Validation

PostgreSQL integration coverage checks the preview, automatic apply, distinct editions on each
side, the strict preflight, saved associations on rerun, and the manual review path.
