# ADR-0114: Accept MeloAmp playlist add-songs object body alongside canonical array form

- **Date:** 2026-08-09
- **Status:** Accepted
- **Decision owners:** Repository maintainers (Phase 7 compatibility)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-M1-001, PRD-API-M1-002, PRD-API-M1-005, PRD-API-M1-006

## Context

Phase 0 capture proved a client/server request-shape mismatch on the native Melodee API:

- The legacy server binds `POST /api/v1/playlists/{apiKey}/songs` to `[FromBody] Guid[]` — a
  **bare JSON array** of song IDs.
- Current MeloAmp posts an **object body** `{ "songIds": [...] }`
  (`PlaylistPickerDialog.tsx:91`), which fails ASP.NET model binding with a 400 validation
  envelope. Golden evidence:
  `compatibility/melodee-api-v1/golden/native-catalog/catalog-playlist-crud-add-songs-meloamp-shape.json`.
- The bare-array form is additionally broken at the baseline (500, null-navigation in
  `PlaylistService.AddSongsToPlaylistAsync` when the playlist already has songs), so no working
  add-songs path exists for MeloAmp today.

Strict parity would reproduce the 400 for MeloAmp's object body. That permanently breaks a
current client feature.

## Decision

For the Phase 7 native API adapter:

1. Preserve the currently valid Melodee API v1 request shape (bare `Guid[]` array) — existing
   captured contract.
2. Additionally accept the object shape `{ "songIds": [...] }` used by current MeloAmp, where
   safe (unambiguous content negotiation by JSON body shape).
3. Do NOT preserve the legacy 500 failure of the bare-array form (see coverage-matrix D4);
   both accepted shapes must work correctly.

The Phase 7 adapter documents both forms with an OpenAPI `oneOf`. The bare array remains the
canonical legacy form; the object form is an additive first-party-client compatibility form.

## Alternatives considered

### Bare array only (strict legacy contract)

Rejected as the sole form: knowingly breaks a current first-party client feature. The object
form is unambiguous and trivially distinguishable from an array body.

### Object only (redesign to MeloAmp's shape)

Rejected: changes the documented/captured contract for any other consumer of the bare-array
form and violates the compatibility rules without evidence that nothing else uses it.

## Rationale

Accepting both shapes is a small, additive, non-breaking extension that repairs a real client
without weakening the captured contract. It aligns with the compatibility hierarchy: observable
behavior plus actual client usage over the published document.

## Consequences

### Positive

- MeloAmp add-to-playlist works without a client release.
- The canonical captured contract remains valid.

### Negative / trade-offs

- The endpoint accepts two body shapes; contract tests must cover both, and the OpenAPI
  description should be updated in Phase 7 to document them.

### Follow-up work

- Phase 7: implement dual-shape binding with golden tests for both forms (including the legacy
  400-envelope behavior for malformed bodies).
- Phase 7: fix the underlying add-songs behavior (no 500; correct ordering/duration updates).

## Validation

- Phase 7 golden scenarios: object form accepted; bare-array form accepted; malformed body →
  the captured 400 validation envelope.

## References

- [PRD.md](../../PRD.md) (PRD-API-M1-001/002/005/006)
- [Phase 0 report](../evidence/phase-0/PHASE-0-REPORT.md) (risk R3)
- [Native client route inventory](../evidence/phase-0/native-client-route-inventory.md)
  (finding 5)
- `compatibility/melodee-api-v1/golden/native-catalog/catalog-playlist-crud-*`
