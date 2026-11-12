# ADR-0187: equalizer preset list compatibility alias for envelope-reading clients

- **Date:** 2026-11-12
- **Status:** Proposed
- **Decision owners:** native API maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-001, PRD-POLICY-002

## Context

`GET /api/v1/equalizer/presets` is frozen with a named collection and a pager:

```json
{ "presets": [ ... ], "meta": { "totalCount": 0, ... } }
```

That shape is captured in
`compatibility/melodee-api-v1/golden/native-community/community-lifecycle-equalizer-list-empty.json`
and restated by C# `EqualizerPresetsController.ListAsync`
(`return Ok(new { presets, meta = ... })`).

Every *other* native paged collection uses `{ "data": [...], "meta": { ... } }`.
MeloAmp — the reference desktop client this backend must serve — reads paged
collections through one generic helper that unwraps `data`. Its equalizer screen
therefore throws `t.map is not a function` on the frozen shape and renders an
error boundary, while every other list works. The client cannot be changed here.

## Decision

The equalizer list response keeps `presets` and `meta` byte-identical to the
frozen contract and **adds** a `data` field carrying the same collection:

```json
{ "presets": [ ... ], "meta": { ... }, "data": [ ... ] }
```

Rules:

- `presets` remains authoritative; `data` is a compatibility alias and must always
  equal `presets` (asserted).
- No other endpoint gains an alias: everything else already uses the `data`
  envelope the frozen contract defines.
- No request parameter selects the shape; both are always present, so an unknown
  key cannot break a strict client (JSON clients ignore unknown members) and the
  frozen keys never change.
- If a future C# capture shows this endpoint moved to the generic envelope, the
  alias is promoted to the only field and this ADR is superseded.

## Alternatives considered

### Alternative A — leave the frozen shape and treat the client as broken

Preserves literal fidelity, but ships a view that cannot work for the reference
client. The client is a product requirement ("MeloAmp must work against
melodee-rs"), so a strict-fidelity argument that leaves a screen unusable is not
acceptable.

### Alternative B — return a bare array

Matches one of the client's accepted shapes but discards `meta` and breaks the
captured contract outright. Rejected: the alias keeps the frozen keys.

### Alternative C — negotiate the shape by Accept header or query parameter

Requires the client to ask for something it does not ask for. Rejected as
unusable.

## Rationale

A purely additive member is the smallest change that makes the reference client
work without removing or renaming anything the frozen contract promises. The
assertions keep `presets`/`meta` exact, so compatibility drift in the authoritative
shape still fails the gate.

## Consequences

- MeloAmp's equalizer screen loads presets instead of throwing.
- The compatibility test for this endpoint asserts the frozen keys exactly and the
  alias equals them.
- Response size grows by the serialized collection; the pager bound (20 default,
  validated limit) keeps that small.

## Validation

- `apps/melodee-server/tests/native_community_api.rs` — empty list: `presets` and
  `meta` equal the captured golden with `data` removed, and `data == presets`.
- Live verification in MeloAmp: the equalizer view renders without the error
  boundary (`design/evidence/meloamp-parity/MELOAMP-VERIFY.md`).
