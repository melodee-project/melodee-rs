# ADR-0122: MQL syntax and semantics

- **Date:** 2026-08-11
- **Status:** Accepted
- **Decision owners:** User-directed Phase 6 authority work
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SEARCH-004, PRD-SEARCH-005, PRD-SEARCH-006, PRD-SEARCH-007,
  PRD-SCALE-001, PRD-SCALE-002, PRD-PERF-003, PRD-TEST-001, PRD-TEST-005, PRD-TEST-006, PRD-ARCH-004

## Context

PRD-SEARCH-004 requires MQL to parse `input string -> MQL AST -> catalog/search query plan` with no
Tantivy types leaking into the AST or planner contract. PRD-SEARCH-005 requires preserving current
capabilities (field search, numeric comparisons, date/range expressions, boolean logic, supported
regex/patterns) while making structured participation searchable via explicit role concepts
(`artist`, `primaryArtist`, `featuredArtist`, `performer`) rather than collapsing everything into one
ambiguous Artist field.

The frozen legacy C# server is the behavioral oracle (`scripts/legacy_oracle.py`). Legacy MQL is a
lucene-flavored query language; we must preserve the supported subset and explicitly reject known
legacy bugs. The AST, validation, and query-plan types must contain no Tantivy, SQLx, Axum, or
filesystem types (rule A in the work package).

## Decision

### Pipeline

```text
input string
  -> tokenizer (MQL lexer)
  -> parser (recursive descent, precedence-climbing) -> pure MqlAst
  -> validator (field/role/operator checks, resource bounds) -> CatalogQueryPlan
  -> Tantivy translator (behind the search application boundary; not part of the pure AST)
```

### Lexical rules

- Tokens: `(`, `)`, `:`, comparison ops (`=`, `!=`, `<`, `<=`, `>`, `>=`), `..` (range), `AND`/`OR`/
  `NOT` (case-insensitive keywords), quoted strings (`"..."` or `'...'`), bare words, and `-field:`
  negation prefix.
- Whitespace separates tokens; unquoted value runs until the next delimiter.
- Quoted strings honor `\"`, `\\`, `\n`, `\t` escapes. A bare `\` at end-of-input or before an unknown
  escape is a syntax error with an exact span.

### Grammar (precedence, highest first)

```text
orExpr   := andExpr ( OR andExpr )*
andExpr  := unary  ( (AND | implicit) unary )*
unary    := NOT unary | primary
primary  := '(' orExpr ')' | clause
clause   := field ':' value
          | field compareOp value
          | field ':' value '..' value        # inclusive range
          | field compareOp value '..' value
          | value                              # bare term -> default field(s)
```

Implicit AND: two adjacent `primary` tokens with no operator bind as AND (legacy-compatible). Explicit
`AND`/`OR`/`NOT` are case-insensitive. `NOT` has higher precedence than `AND`; `AND` higher than `OR`
(standard precedence). Parentheses group.

### Fields and roles

- `artist` — matches any credited/participating Artist (display name, sort name, or alias) across the
  indexed entity. ID-resolved internally against `participant_artist_ids`/`artist_ids`.
- `primaryArtist` — `primary_artist_ids` (Release/Track primary billing).
- `featuredArtist` — featured/guest performance participants.
- `performer` — all performance-role participants (instrumentalist/vocalist/etc.).
- `title` / `name` / `release` / `track` — entity text fields per kind.
- `genre` / `tag` — multi-valued terms.
- `year` — `i64` comparison/range against `release_year`/`track` year.
- `date` — date comparison/range against `release_date`/`updated_at`.
- `duration` — `i64` ms comparison/range against `duration_ms`.
- `type` / `status` — Artist kind/status terms.
- `id` / `publicId` — exact internal/public ID term match.
- Default (bare term) targets `display_name`/`title` text plus alias text, per the legacy default
  field behavior.

### Operators and semantics

- Text equality `field:value` is a tokenized contains/term match in Tantivy (legacy "contains" flavor
  for text; exact term match for ID/enum fields).
- Comparisons `< <= > >= = !=` apply to numeric/date fields (`year`, `duration`, `date`).
- Range `a..b` is inclusive; `field:a..` and `field:..b` are open ranges. `!=` and negated clauses use
  Tantivy `NOT` semantics.
- Regex/pattern: support the legacy `/pattern/` or `field:/regex/` syntax using Tantivy's regex query
  on tokenized text where the analyzer permits; document that regex runs on indexed tokens, not raw
  bytes, and is bounded (see bounds).

### Error model

- Typed `MqlError` variants: `SyntaxError { message, span: (start, end) }`,
  `UnknownField { field, span }`, `UnknownRole { role, span }`, `TypeMismatch { field, expected, span }`,
  `RangeError { message, span }`, `RegexError { message, span }`, `ExceedsBound { kind, limit, span }`.
- Spans are byte offsets into the input; error messages are deterministic and testable.

### Resource bounds (hard limits, all configurable but with safe defaults)

| Bound | Default | Enforcement |
|---|---|---|
| Input length | 4 KiB | reject before tokenize |
| Token count | 1024 | counter during lex |
| AST depth | 32 | depth check in parser |
| Clauses (leaf predicates) | 256 | counter during plan build |
| Regex length | 256 chars | reject before compile |
| Result limit | 200 (max 1000) | enforced by translator/pager |
| Parse timeout | 100 ms | cooperative deadline in lexer/parser |

Exceeding any bound returns `ExceedsBound` with the offending span. Parsing is deterministic: same
input -> identical AST and plan (no hashmap iteration order dependence).

### Legacy compatibility and intentionally rejected bugs

- **Preserved:** implicit AND, `field:value`, `field<op>value`, `field:a..b` ranges, `AND`/`OR`/`NOT`,
  `artist:` as cross-participation search, quoted strings, `( )` grouping, and supported `/regex/`.
- **Intentional rejections (legacy bugs not preserved):** (1) silent truncation of over-long queries
  without an error — we return `ExceedsBound`; (2) case-sensitive `AND`/`OR`/`NOT` — we accept
  case-insensitively per modern clients; (3) ambiguous bare-term scoping that matched only the title in
  some releases — bare terms now match name/title/alias uniformly; (4) regex evaluated against raw
  untokenized bytes (locale/diacritic mismatches) — we evaluate against the Unicode-folded token stream
  for consistency with text search.
- Compatibility fixtures (golden request/response) prove preserved behavior; regression tests pin the
  rejected bugs.

### What is explicitly unsupported (documented, not silently broken)

- No ` fuzzy `~` operator in the initial contract (documented as planned/unsupported).
- No field wildcard `*` queries beyond the defined default-field expansion.
- No scripting/function queries.

## Alternatives considered

### Reuse the legacy C# MQL parser line-for-line in Rust

Rejected by the ground-up rewrite rule; legacy is evidence, not architecture. A clean AST/plan lets us
add ID-backed participation without string reconstruction.

### Use a Tantivy QueryParser directly

Rejected by PRD-SEARCH-004: it leaks Tantivy types into the domain and prevents SQL/other translators
and pure unit testing. We keep a pure AST and a separate translator.

### Expose only a single `artist` field

Rejected by PRD-SEARCH-005: it collapses participation semantics and forces string reconstruction.

## Rationale

A pure, bounded, span-aware AST satisfies PRD-SEARCH-004/005 and keeps the door open for SQL
translation and differential legacy compatibility tests. Role concepts make participation ID-backed and
testable.

## Consequences

### Positive

- No Tantivy/SQLx/Axum types in the AST or planner contract (domain-free boundary).
- Deterministic, bounds-enforced, span-typed errors.
- Explicit, tested legacy parity with named rejected bugs.

### Negative / trade-offs

- A new, small MQL crate (`melodee-mql`) plus a `melodee-search` application crate (per work package).
- Translator must stay behind the boundary; some legacy regex edge cases are intentionally redefined.

### Follow-up work

- `melodee-search` translator maps `CatalogQueryPlan` -> Tantivy `Query` over ADR-0121 schema.
- Autocomplete/admin-lookup services use the same AST/plan boundary.

## Validation

- MQL parser tests: precedence, parentheses, escaping, ranges, comparisons, regex; invalid syntax with
  exact spans; depth/token/clause/regex bounds; deterministic parse/plan output.
- Legacy compatibility corpus: field searches, numeric/date/range, boolean logic, supported regex, and
  the four explicitly rejected legacy bugs.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0107](ADR-0107-use-tantivy-as-search-projection.md)
- [ADR-0121](ADR-0121-search-projection-schema-and-lifecycle.md)
- [Phase 6 work package](../ROAD-TO-FEATURE-COMPLETION.md)
