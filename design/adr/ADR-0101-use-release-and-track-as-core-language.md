# ADR-0101: Use Release and Track as the core media language

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Melodee project owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-CREDIT-001 through PRD-CREDIT-008

## Context

`Album` and `Song` imply music and do not fit comedy, spoken word, skits, intros, interviews,
audiobooks, broadcasts, or mixed content. Existing public APIs already use Album/Song vocabulary and
cannot be casually changed.

## Decision

The Rust domain and new admin API/UI use `Release` and `Track`. Compatibility adapters continue to use
`Album` and `Song` exactly where existing Melodee API v1, OpenSubsonic, Jellyfin, or migration
contracts require them.

## Rationale

Release and Track are accurate, content-neutral, and understandable while preserving external
compatibility through adapters.

## Consequences

- Core code must reject accidental legacy vocabulary.
- Adapter mapping is explicit and tested.
- Documentation must distinguish core and compatibility terms.
