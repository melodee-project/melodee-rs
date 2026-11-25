# ADR-0204: Match portable listener accounts by unique email

- **Date:** 2026-11-24
- **Status:** Accepted
- **Decision owners:** Migration tooling and homelab operator
- **Supersedes:** ADR-0203 account review for unique email matches only
- **Superseded by:** None
- **Related requirements:** PRD-MIG-002, PRD-MIG-003

## Context

A clean library can contain the same listeners under newly generated public IDs. C# enforces
uniqueness of its stored email field and Rust enforces uniqueness of normalized email.
An administrator importing a trusted export wants those existing listeners matched automatically.
Two C# addresses can still be equal after normalization, and a mailbox can change owners.

## Decision

Resolve exact public IDs and previously bound IDs first. For remaining accounts, compare email
using the native login normalization (trim and uppercase). Accept an exact match for a real
address unique in the export and target, including when the username has changed. An old username
that already belongs to a different target, duplicate addresses, exporter placeholders, or a target
claimed by a different export identity cannot establish an automatic link. Operators can resolve
collisions explicitly. Music metadata suggestions continue to require review under
[ADR-0203](ADR-0203-reconcile-listener-preferences-with-a-rebuilt-catalog.md).

A dry run reports email matches without recording them. On apply, atomically record the two public
IDs in an installation-specific account map in the export directory before writing activity. Keep
this file with the export. Subsequent runs with that export use the recorded IDs even if a mailbox
is reassigned or the operator uses a different music review file. Reject conflicting saved links.
Use the workspace's existing tempfile dependency for private atomic file replacement. This is an
operator migration feature. Authentication, account privileges, linked profiles and avatars keep
their existing values. There is no database migration.

## Alternatives considered

Requiring review for each matching email repeats a decision the operator can express once. Looking
up email afresh on every import risks attaching additional activity to the next owner of a mailbox.

## Validation

Exercise renamed usernames, case differences, duplicate source addresses, username-only collisions,
read-only preview, linked profile preservation, and a rerun after the original email is reassigned.
