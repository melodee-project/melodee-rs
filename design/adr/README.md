# Architecture Decision Records

ADRs capture durable decisions that affect architecture, product behavior, compatibility, storage,
security, dependencies, migration, or multiple modules.

## Why numbering starts at 0100

The existing C# repository already contains ADR-0001 through ADR-0007. This repository starts rewrite
ADRs at **ADR-0100** to avoid collision and preserve historical identity. See
[legacy/README.md](legacy/README.md).

## Withdrawn evidence reports

The `design/evidence/` phase reports were removed from the repository on 2026-08-25
(commit `26cb87c`) as historical artifacts. Accepted ADRs, the requirement index
(`design/requirements/REQUIREMENTS-INDEX.md`), and the committed compatibility evidence in
`compatibility/` are the authoritative record of what was decided, tested, and shipped.

Where an ADR or design document cites one of those reports, the citation names it as a
withdrawn evidence report. The original text remains recoverable from git history, for
example:

```bash
git show 26cb87c^:design/evidence/phase-0/PHASE-0-REPORT.md
```

## Status lifecycle

- **Proposed** — under review; not authority for irreversible implementation.
- **Accepted** — current decision and binding unless superseded.
- **Superseded** — replaced by a newer ADR; retain the file and link the replacement.
- **Deprecated** — no longer recommended, without a direct replacement.
- **Rejected** — considered and deliberately not selected; retained to avoid repeating the debate.

## When an ADR is required

Write an ADR when a decision is hard to reverse, likely to be debated, or affects:

- public API/client compatibility;
- domain language or identity;
- database/storage/search architecture;
- ingestion state or destructive behavior;
- security/authentication;
- deployment/infrastructure;
- production dependencies or FOSS/licensing policy;
- migration/cutover; or
- the UI design system.

## File naming

```text
ADR-0100-short-kebab-case-title.md
```

Use:

```bash
python3 scripts/new_adr.py "short title"
```

## Current Phase 4 decisions

- [ADR-0117: Use a PostgreSQL catalog, identity, and contribution schema with set-based manifest import](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0118: Use AudioIdentity and immutable manifest v3 semantic overlays](ADR-0118-use-audioidentity-and-immutable-manifest-v3-semantic-overlays.md)
- [ADR-0119: Use audit-only corrections for legacy Releases without manifest v3](ADR-0119-use-audit-only-corrections-for-legacy-releases-without-manifest-v3.md)

## Current Phase 5 decision

- [ADR-0120: Phase 5 classification, novelty, and policy schema](ADR-0120-phase5-classification-novelty-and-policy-schema.md)

## Current Phase 6 decisions

- [ADR-0121: Search projection schema and lifecycle](ADR-0121-search-projection-schema-and-lifecycle.md)
- [ADR-0122: MQL syntax and semantics](ADR-0122-mql-syntax-and-semantics.md)

## Current Phase 7 decisions

- [ADR-0123: Phase 7 scope overlap handling](ADR-0123-phase-7-scope-overlap-handling-for-party-requests-podcasts-shares-analytics.md)
- [ADR-0124: Native authentication, session, and refresh-token compatibility](ADR-0124-native-authentication-session-and-refresh-token-compatibility.md)
- [ADR-0125: Native stream-token validation and range semantics](ADR-0125-phase-7-stream-token-validation-and-range-semantics.md)
- [ADR-0126: Native API rate limiting and client-address semantics](ADR-0126-rate-limiting-and-client-address-semantics-for-native-api-v1.md)
- [ADR-0127: Native public-ID migration and identity-preserving schema](ADR-0127-native-public-id-migration-and-id-preserving-schema-for-api-v1.md)
- [ADR-0128: Capture-required native-v1 legacy bug fixes](ADR-0128-capture-required-legacy-bug-fixes-in-native-api-v1.md)
- [ADR-0129: Secure native public-share bearers and resolvable links](ADR-0129-secure-native-public-share-bearers-and-resolvable-links.md)

## Release distribution and upgrade decisions

- [ADR-0144: Publish official releases as GHCR OCI images](ADR-0144-publish-official-releases-as-ghcr-oci-images.md)
- [ADR-0169: Accept known comment-only SQLx migration checksums](ADR-0169-accept-known-comment-only-sqlx-migration-checksums.md)

## Development tooling decisions

- [ADR-0171: Confirm and safely wipe configured development roots](ADR-0171-confirm-and-safely-wipe-configured-development-roots.md)

## Release closure decisions (R0–R3)

- [ADR-0145: Drain inbound into durable staging before ingestion](ADR-0145-drain-inbound-into-durable-staging-before-ingestion.md)
- [ADR-0146: CUE-driven track segmentation in normalization](ADR-0146-cue-driven-track-segmentation-in-normalization.md)
- [ADR-0147: Configured transcoding for OpenSubsonic and Jellyfin streaming](ADR-0147-configured-transcoding-for-opensubsonic-and-jellyfin-streaming.md)
- [ADR-0148: Legacy migration contract (Phase 11 tooling)](ADR-0148-legacy-migration-contract.md)
- [ADR-0149: Phase 11 rehearsal, freeze, cutover, and rollback procedures](ADR-0149-phase-11-rehearsal-freeze-cutover-and-rollback-procedures.md)
- [ADR-0150: Legacy media metadata dispositions for Phase 11 migration](ADR-0150-legacy-media-metadata-dispositions-for-phase-11-migration.md)
- [ADR-0151: Use Debian Trixie runtime base for release images](ADR-0151-use-debian-trixie-runtime-base-for-release-images.md)

## Product correctness decisions

- [ADR-0152: Execute confirmed Release deletion with audited tombstones](ADR-0152-execute-confirmed-release-deletion-with-audited-tombstones.md)
- [ADR-0153: Reuse unambiguous Release Artist identity for matching Tracks](ADR-0153-reuse-unambiguous-release-artist-identity-for-matching-tracks.md) (superseded by ADR-0170)
- [ADR-0170: Reuse uniquely matching Release Artist identities for Tracks](ADR-0170-reuse-uniquely-matching-release-artist-identities-for-tracks.md) (supersedes ADR-0153)
- [ADR-0172: Parse Track billing with Release-scoped identity evidence](ADR-0172-parse-track-billing-with-release-scoped-identity-evidence.md)
- [ADR-0173: Reconcile repeated Track Artist credits within one Release](ADR-0173-reconcile-repeated-track-artist-credits-within-one-release.md)
- [ADR-0174: Require review for name-only Artist identity matches](ADR-0174-require-review-for-name-only-artist-identity-matches.md) (partially supersedes ADR-0160)
- [ADR-0175: Converge repeated primary Release Artist credits](ADR-0175-converge-repeated-primary-release-artist-credits.md) (partially supersedes ADR-0174)
- [ADR-0176: Complete Artist merge coalescing and multi-select administration](ADR-0176-complete-artist-merge-coalescing-and-multi-select-administration.md)
- [ADR-0177: Accept ordinary Unknown Releases and reconcile the obsolete default review rule](ADR-0177-accept-ordinary-unknown-releases-and-reconcile-the-obsolete-default-review-rule.md)

## Feature Completion Program decisions

- [ADR-0154: External-provider enrichment scope and runtime architecture](ADR-0154-external-provider-enrichment-scope-and-runtime.md) (supersedes [ADR-0136](ADR-0136-defer-non-last-fm-external-provider-integrations-to-post-parity.md))
- [ADR-0155: End-user web UI product surface and architecture](ADR-0155-end-user-web-ui-surface-and-architecture.md)
- [ADR-0156: MPD/MPV playback backend connectivity and lifecycle](ADR-0156-mpd-mpv-backend-connectivity-and-lifecycle.md)
- [ADR-0157: Metadata transformation contract and safe file rewrites](ADR-0157-metadata-transformation-and-safe-rewrite.md)
- [ADR-0164: Metadata rewrite integrity uses decoded-audio fingerprints](ADR-0164-metadata-rewrite-integrity-uses-decoded-audio-fingerprints.md)
- [ADR-0158: Scheduled-job run history and operator visibility](ADR-0158-scheduled-job-run-history-storage.md)
- [ADR-0159: OpenSubsonic stored-artwork sizing](ADR-0159-opensubsonic-stored-artwork-sizing.md)
- [ADR-0160: Cross-Release Artist/Release identity reuse and disposition](ADR-0160-cross-release-artist-release-identity-reuse.md)
- [ADR-0161: MeloAmp owns listener workflows and the server exposes only admin and narrow HTML surfaces](ADR-0161-meloamp-owns-listener-workflows-and-the-server-exposes-only-admin-and-narrow-html-surfaces.md) (supersedes ADR-0155)
- [ADR-0162: Durable provider enrichment tasks and Release external evidence](ADR-0162-durable-provider-enrichment-tasks-and-release-external-evidence.md)
- [ADR-0163: Rust-native MusicBrainz DecentDB replica reads](ADR-0163-rust-native-musicbrainz-decentdb-replica-reads.md) (superseded by ADR-0168)
- [ADR-0168: Upgrade pinned DecentDB replica reader to 2.17.1](ADR-0168-upgrade-pinned-decentdb-replica-reader-to-2-17-1.md) (supersedes ADR-0163)

## ADR quality rules

- State the decision, not merely the chosen technology.
- Explain alternatives and why they were rejected.
- Record consequences, including negative ones.
- Link PRD/SPEC requirement IDs and implementation evidence.
- Do not modify an accepted ADR to erase history. Supersede it.
- Proposed ADRs may contain open questions; accepted ADRs may not hide unresolved critical risks.
