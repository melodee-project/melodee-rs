# ADR-0188: last-played history feed and chart list compatibility aliases

- **Date:** 2026-11-12
- **Status:** Proposed
- **Decision owners:** native API maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-001, PRD-UI-REF-002, PRD-POLICY-002

## Context

Two native routes disagreed with the reference client (MeloAmp) in ways that made
a *populated* catalogue look empty or undefined. Both were found by seeding real
data and driving the client, which is the only way these surface: the endpoints
answer `200` with a well-formed envelope either way.

1. **`GET /api/v1/user/last-played` counted plays and returned none.** The frozen
   C# projection selected its rows without eager-loading the Song navigation and
   filtered `x.Song != null` at serialization time, so a catalogue with plays could
   still serialize `data: []` while `meta.totalCount` reflected the bounded row
   count. melodee-rs reproduced the empty array literally
   (`data: Vec::new()`), documented as captured parity. The reference client
   renders a history feed — `{id, song, playedAt, duration}` — from this route, so
   "History" showed *"No play history yet"* for an account with 546 plays.
2. **Chart lists use `title`/`itemCount`.** The frozen `ChartListModel` is
   `{Id, ApiKey, Slug, Title, …, ItemCount, ImageUrl}`; the client models a chart
   as `{id, name, description, trackCount, imageUrl}` and rendered
   `undefined tracks` with blank names.

Neither shape can be satisfied by the other without a change on one side. The
client cannot be modified for this work, and a screen that cannot render its data
is a parity failure regardless of which side "owns" the mismatch.

## Decision

1. **`/user/last-played` returns the plays it counts.** Each item is
   `{id, song, playedAt, duration}`: `id` is the durable play-event id (so a Track
   played twice appears twice), `song` is the same `Song` DTO every other native
   route emits (including `streamUrl`, so "play again" works), `playedAt` is
   RFC 3339, and `duration` is the recorded play duration in seconds. The envelope
   stays `{data, meta}` with the same paging semantics. Rows are restricted to
   client-visible Releases and the caller's library access, exactly like the other
   user-scoped routes.
2. **Chart list and detail responses mirror `title`/`itemCount` into
   `name`/`trackCount`.** The frozen keys remain authoritative and always equal the
   aliases; the aliases are derived, never selected, so they cannot drift.
3. Both additions are additive or corrective on keys the frozen contract already
   promised; no existing route, method, status code, or error shape changes.

## Alternatives considered

### Alternative A — keep the empty last-played array (literal captured parity)

Leaves the History screen permanently empty and contradicts the route's own
`totalCount`. Rejected: the capture reflects a serialization bug in the frozen
service, not a contract a client can use.

### Alternative B — return only the compact `{songId, name, playedAt}` projection

Cheaper, but the client needs a Song DTO to render artwork and replay an entry, and
a second round trip per row is worse than one bounded join.

### Alternative C — change the client's models

Out of scope: the client is the parity target and must not be edited for this work.

## Rationale

Both changes make an existing route tell the truth about its own data. The
history feed reuses the DTO the rest of the API emits, so it inherits every
existing guarantee (stream URLs, visibility, access control); the chart aliases
are a one-field mirror with a test that pins equality.

## Consequences

- History shows real plays, with replay working from the entry itself.
- Chart cards show names and counts; the frozen keys are unchanged.
- `meta.totalCount` and `data.len()` agree for last-played, so pagination is
  consistent.
- Analogue for other clients: any client that read the frozen `Song[]` projection
  from last-played now sees history items. No shipped client did (that projection
  could not be consumed), and the change is recorded here rather than silently.

## Validation

- `apps/melodee-server/tests/native_catalog_api.rs` — after a scrobble,
  `/user/last-played` returns one item whose `song.id`/`song.title`/`streamUrl`
  match the played Track and whose `duration` matches the submitted play.
- `apps/melodee-server/tests/native_discovery_api.rs` — chart list and detail
  expose `name == title` and `trackCount == itemCount`.
- Live: `design/evidence/meloamp-parity/MELOAMP-VERIFY.md` records every screen
  rendering seeded data.
