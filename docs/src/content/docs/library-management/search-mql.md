---
title: Search and MQL
description: Catalog search and the Melodee Query Language (MQL).
sidebar:
  order: 6
---

Melodee search is backed by a dedicated index that is a rebuildable projection of the catalog —
rebuilding it never requires re-ingesting media. Search covers Artists, Releases, Tracks, aliases,
display credits, genres/tags, and ID-backed Artist participation. PostgreSQL remains authoritative;
the index can be discarded and regenerated without reading source or canonical audio.

**MQL** (Melodee Query Language) is the structured query syntax for precise catalog queries.

## Querying from the CLI

```bash
melodee search query 'artist:"Colin Hay" AND year>=2000' --limit 50
melodee search autocomplete artist Bjo --limit 10
melodee search parse '(genre:rock OR genre:jazz) AND year:1990..1999'
melodee search explain 'performer:"Guest Artist"'
```

`query` and `autocomplete` require an active search generation. `parse` prints the pure syntax tree;
`explain` also validates the query and prints its framework-independent catalog plan.

## Syntax

MQL supports:

- implicit `AND` between adjacent terms, plus case-insensitive `AND`, `OR`, and `NOT`;
- parentheses, quoted strings, and `-field:value` negation;
- `=`, `!=`, `<`, `<=`, `>`, and `>=` for numeric/date fields, including signed numeric values;
- inclusive ranges such as `year:1990..1999`, including open ranges;
- bounded token regex such as `track:/blue.*/`; and
- exact internal/public ID lookup.

`NOT` binds before `AND`, which binds before `OR`.

| Field | Meaning |
| --- | --- |
| `artist` | Any credited or participating Artist, resolved to Melodee Artist IDs. |
| `primaryArtist`, `featuredArtist`, `performer`, `other` | Specific structured participation roles. |
| `title`, `release`, `track`, `name`, `alias` | Entity text. |
| `genre`, `tag` | Indexed genre/trait terms. |
| `year`, `date`, `duration` | Comparable year, date, or duration in milliseconds. |
| `type`, `status` | Artist/Release type or status. |
| `id`, `publicId` | Exact internal or public identifier. |

Bare terms search Artist names/aliases and Release/Track titles consistently. Regex operates on
Unicode-tokenized, lowercased, ASCII-folded terms, not raw file bytes.

## Bounds and unsupported syntax

Queries are rejected, with a typed error and byte span, above 4 KiB, 1,024 tokens, depth 32, 256
clauses, or 256 regex characters. Empty regexes and leading Boolean operators are syntax errors.
Result pages default to 200 and cannot exceed 1,000; offsets above 10,000 are rejected. Fuzzy `~`,
wildcard `*`, scripts, and function queries are not supported.

## Rebuild and recovery

```bash
melodee search status
melodee search rebuild --actor administrator-id
```

Rebuild creates a separate owned generation, validates its schema/counts, catches up catalog changes,
and atomically switches readers. The prior generation remains active until that boundary succeeds.
Never delete index directories manually; use the rebuild command and inspect `search status` first.

See [Configuration](../installation/configuration/#search-projection) for required paths and worker
bounds, and [Troubleshooting](../troubleshooting/#search-index-checks) for recovery steps.
