# ADR-0190: Name the imported Artist state `active` rather than `provisional`

- **Date:** 2026-11-13
- **Status:** Accepted
- **Decision owners:** User-directed product correction
- **Supersedes:** [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md) decision item 8, only its Artist status vocabulary
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-010, PRD-ARTIST-011,
  PRD-CATALOG-001, PRD-I18N-006

## Context

ADR-0117 item 8 fixed the Artist status vocabulary as `provisional`, `verified`, `merged`, and
`deprecated`, and import created every Artist as `provisional`.

That naming described a missing operator assertion rather than what the row is. An imported Artist is
live and complete: it carries real credits, appears in browse and search, streams to clients, and never
needs to be "finished" before use. Labelling all of it `provisional` produced a catalogue where:

- every Artist on the Artists screen read Provisional, so the label carried no information;
- the state read as unfinished work even though nothing was pending, because Identity Review only
  queues *unresolved observations* and after a clean import there are none;
- an operator reasonably concluded the catalogue was stuck, when 2,548 Artist rows, 961 Releases, and
  9,638 Tracks had all imported correctly.

The word was also leaking into unrelated vocabulary: the generic pending chip kind, the
`leave-provisional` resolution verb, hints, and prose. `verified` remains meaningful and stays: it
records that a person asserted the identity, which is a quality signal on top of a working Artist.

## Decision

1. The imported Artist state is **`active`**. The domain enum, database value, admin API, admin UI,
   and public documentation all use `active`.
2. The Artist vocabulary is `active`, `verified`, `merged`, `deprecated`. Migration
   `0044_rename_artist_status_provisional_to_active.sql` relabels existing rows and tightens
   `artists_status_check`. It rewrites only the status column; ids, public ids, tombstones, decisions,
   evidence, aggregates, and audit history are untouched.
3. The state itself is unchanged. Import still does not claim identity authority, and every guard that
   keyed on "unverified" now keys on `active`: automated merge candidates, name-reuse conflict gates,
   review proposals, browse/search eligibility, and the metadata edit allow-list.
4. `verified` keeps its meaning and its single promotion path (an operator `confirm`). Nothing becomes
   `verified` automatically.
5. The admin API resolution verb `leave-provisional` becomes `leave-unresolved`; the old verb stays
   accepted as an alias so existing scripts keep working. The generic pending status-chip kind becomes
   `pending`.
6. `audio_identities` keeps its own `provisional` vocabulary. It is an internal recording-identity
   state, never an Artist state, and it is not operator-facing.

## Alternatives considered

### Keep `provisional` and only change the UI label

Rejected. The database value, admin API, and every operator-facing document would still say
`provisional`, so the next exporter, script, or support conversation reintroduces the same
misunderstanding. A label that disagrees with the stored value is worse than either alone.

### Drop the unverified state entirely and call every Artist `verified`

Rejected. `verified` would then mean nothing, and it is the only durable signal that a person
confirmed the identity. Erasing the distinction also removes the safety property that automated
reuse and merging only ever act on identities no human has asserted.

### Name it `imported`

Rejected as a description of provenance rather than of state: after a merge or a provider enrichment
the Artist is no longer simply "imported", but it is still active.

### Make every Artist `verified` when a provider returns a candidate

Rejected and out of scope. Provider candidates are evidence, not identity (PRD-ARTIST-012), and 2,356
candidates across 131 Artists is a long way from an identity claim.

## Rationale

`active` states what is true: the Artist is live, credited, and usable, and its identity is not yet
asserted. It stops the vocabulary from implying unfinished work, keeps `verified` meaningful, and
removes the word from every operator-facing surface in one auditable rename. Relabelling instead of
restructuring means no behaviour change, no reindex, and no risk to identity safety.

## Consequences

### Positive

- The Artists screen reads Active for the normal state, and Verified stands out as the exceptional one.
- Operators can tell "this is fine" from "a person confirmed this identity" without inferring intent.
- API values, database values, documentation, and UI agree, so scripts and support conversations stop
  disagreeing with the product.

### Negative / trade-offs

- A vocabulary change touches identifiers everywhere, so it is a wide diff for a small semantic
  change. Migration `0044` is intentionally narrow to keep the risk low.
- `audio_identities` keeps `provisional`, so the codebase now has two vocabularies. That is deliberate
  but it is a place a future reader can be confused; the migration comment and this ADR say why.
- Any external integration that matched on the string `provisional` must switch to `active`. The
  admin API resolution alias is the only compatibility shim retained.
- The historical migration `0040` wrote `provisional` Artist rows; it remains correct because it runs
  before `0044` on a fresh database, and `0044` relabels its output on an existing one.

### Follow-up work

- Provider-confirmed promotion (`active` → `verified` on a durable trusted identity) is still not
  implemented; it is the natural next step to make `verified` self-service.
- The generic pending chip kind should stay `pending`; do not reintroduce product-state names in the
  visual vocabulary.

## Validation

- Domain tests compile against `ArtistStatus::Active`; no `Provisional` variant remains.
- Migration tests: forward migration from the prior schema relabels rows; stepwise tests that drive
  current import code against a historical snapshot install the current vocabulary explicitly.
- PostgreSQL integration suite: import writes `active`, metadata edits accept `active`/`verified` and
  reject `merged`/`deprecated`, reconciliation gates operate on `active`.
- Admin API tests assert `status: "active"` on list and detail reads.
- Admin UI: typecheck, locale-structure check, and 300 unit tests pass with `Active` labels and the
  `pending` chip kind.
- Live check on the development catalogue: after migration `0044`, all 1,945 imported Artists read
  `active`, none read `provisional`, and the API returns `"status": "active"`.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0189](ADR-0189-reuse-an-existing-artist-by-exact-normalized-name-at-import.md)
- `crates/melodee-persistence/migrations/0044_rename_artist_status_provisional_to_active.sql`
