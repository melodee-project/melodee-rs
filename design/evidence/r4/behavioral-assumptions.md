# R4 behavioral assumptions and release boundaries

Status: **Working assumptions for rehearsal and evidence only**. This file does not accept,
amend, or supersede an ADR, product decision, compatibility contract, or release authorization.

Reviewed: 2026-08-20  
Scope: R4 rehearsal, migration, differential, client, integration, freeze, cutover, and rollback
work packages, plus the preparatory R5 evidence that depends on them.

These assumptions make the current local work reproducible. A release owner or decision owner must
confirm or replace any assumption that affects production behavior before the corresponding gate can
be marked complete.

## Compatibility

- Native Melodee API v1, OpenSubsonic, and Jellyfin remain separate compatibility products with
  separate wire DTOs, route adapters, normalization rules, and client evidence.
- Existing routes, properties, identifiers, status codes, errors, authentication, pagination,
  range behavior, and intentional `410`/`501` behavior are treated as immutable unless current
  compatibility evidence, an explicit product decision, an accepted ADR, and a client/migration
  plan authorize a change.
- A differential mismatch is not assumed to be volatility merely because it is inconvenient. It
  must be fixed, mapped under accepted authority, narrowly compared as a genuinely volatile field,
  or held for a decision-owner disposition.
- Historical Phase 7/8 client evidence is regression evidence only. It is not assumed to prove the
  current migrated release-candidate client gate.

## Migration and identity

- The source database and canonical media are supplied as an approved isolated snapshot and mapped
  media root; migration opens the source read-only and never seeds, writes, drops, moves,
  re-encodes, or deletes the source.
- The target PostgreSQL database becomes authoritative only after the operator-approved cutover.
  Tantivy is a rebuildable projection generated from migrated PostgreSQL authority and is never
  treated as migration authority.
- Existing public IDs and source-primary-key mappings are migration data. Display names,
  normalized names, paths, and filesystem layout are never semantic identity.
- A legacy field without an accepted Rust representation remains an auditable reference-only issue
  or an explicitly decided mapping; no metadata semantics are inferred from a proposed ADR.
- A supplied backup must be independently validated before target creation, and a failed target is
  retained for forensic inspection during rollback rehearsal.

## Security and data-loss boundaries

- Local commands and fixtures use disposable databases, temporary roots, synthetic credentials, and
  non-sensitive test data. No local validation targets a live production database or media root.
- Probe tooling is bounded and fail-closed. It does not submit credentials, create accounts, send
  mail, mutate databases, or write to canonical media; secrets, tokens, private roots, and personal
  data must not appear in logs or evidence.
- Filesystem operations reject symlink/reparse components and unexpected file types where the
  relevant safety contract requires no-follow behavior. Uncertainty never authorizes deletion or
  replacement of inbound, staged, canonical, podcast, backup, or search data.
- Cutover and rollback evidence must identify objective abort triggers, the decision owner, the
  decision window, and verification commands. A synthetic rollback result is not assumed to prove
  production-topology rollback.

## Release and authorization

- The centralized workspace version remains pre-1.0 until an unchanged, release-owner-accepted R5
  candidate exists. Documentation or local image preparation does not authorize versioning.
- No tag, push, registry publication, provenance attestation, SBOM publication, or GitHub Release
  is performed without separate release-owner authorization.
- A local image, scan, or workflow validation is not assumed to be candidate evidence. Candidate
  claims require the exact candidate SHA, architecture-specific artifacts, image digests, SBOM,
  provenance, vulnerability results, checksums, and required external workflow evidence.
- ADR-0149, ADR-0150, and ADR-0151 are Proposed in the current repository. Their procedures,
  metadata mappings, and runtime-base choice are not implementation authority until their decision
  owners accept or supersede them.

## Required confirmations before promotion

The following are inputs, not assumptions, for release promotion:

1. Release-owner dispositions for ADR-0149, ADR-0150, and ADR-0151.
2. An approved isolated production snapshot, source backup location, canonical-root mapping, and
   authorization to run the read-only rehearsal.
3. Current physical-client access and operator participation for Android, Android Auto/DHU, MeloAmp,
   representative OpenSubsonic clients, and representative Jellyfin clients.
4. Approved non-sensitive deployment endpoints and test credentials/recipients for provider,
   proxy/TLS, storage, backup/restore, and cutover validation.
5. Release-owner acceptance of the exact R5 candidate before R6 versioning or publication.

