# Architecture Decision Records

ADRs capture durable decisions that affect architecture, product behavior, compatibility, storage,
security, dependencies, migration, or multiple modules.

## Why numbering starts at 0100

The existing C# repository already contains ADR-0001 through ADR-0007. This repository starts rewrite
ADRs at **ADR-0100** to avoid collision and preserve historical identity. See
[legacy/README.md](legacy/README.md).

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

## Release distribution decision

- [ADR-0144: Publish official releases as GHCR OCI images](ADR-0144-publish-official-releases-as-ghcr-oci-images.md)

## Release closure decisions (R0–R3)

- [ADR-0145: Drain inbound into durable staging before ingestion](ADR-0145-drain-inbound-into-durable-staging-before-ingestion.md)
- [ADR-0146: CUE-driven track segmentation in normalization](ADR-0146-cue-driven-track-segmentation-in-normalization.md)
- [ADR-0147: Configured transcoding for OpenSubsonic and Jellyfin streaming](ADR-0147-configured-transcoding-for-opensubsonic-and-jellyfin-streaming.md)
- [ADR-0148: Legacy migration contract (Phase 11 tooling)](ADR-0148-legacy-migration-contract.md)

## ADR quality rules

- State the decision, not merely the chosen technology.
- Explain alternatives and why they were rejected.
- Record consequences, including negative ones.
- Link PRD/SPEC requirement IDs and implementation evidence.
- Do not modify an accepted ADR to erase history. Supersede it.
- Proposed ADRs may contain open questions; accepted ADRs may not hide unresolved critical risks.
