# Artist identity parity: reuse instead of duplicate-then-merge

Run `m4` closed the defect this repository's own Identity Review queue could never clear, and it did
so on a live database with real media rather than only in fixtures.

## The defect, measured

The development catalogue, built by importing real music:

| Observation | Value |
|---|---|
| Duplicate normalized-name groups | 57 |
| Artist rows inside those groups | 132 |
| Groups with byte-identical canonical names | 54 |
| Groups with conflicting external identities | 0 |
| Queued Identity Review proposals | 2,817 |
| Artists each holding one `confirmed` per-Release decision | 2,177 |

Every one of those proposals was bookkeeping for a duplicate the importer had just created. The
replaced C# server cannot produce that shape: `LibraryInsertJob` resolves each credit against
`existingArtistLookup.TryAdd($"name:{artist.NameNormalized}", existingArtist)` and reuses the row
across albums.

Root cause: `import_manifest_v3` created a provisional Artist per Release and left cross-Release
consolidation to Identity Review, because ADR-0117 had recorded "creates provisional Artists when no
credible or locked match exists rather than merging by name" as a decision rather than as a bug. No
test asserted the C# behaviour, so nothing caught the divergence.

## The fix

[ADR-0189](../../adr/ADR-0189-reuse-an-existing-artist-by-exact-normalized-name-at-import.md)
supersedes that import rule and keeps every identity invariant: a name is a *bounded reuse hint*,
never a key.

- `reuse_existing_artists` remaps a credit onto an existing Artist when the normalized name matches
  exactly, exactly one non-tombstoned candidate carries it, that Artist has no disambiguation hint,
  and any embedded MusicBrainz Artist identifier agrees.
- Imports of one name serialize on `pg_advisory_xact_lock(hashtextextended(name, 0))`, because a
  check-then-insert decision races when a batch imports several Releases at once. Locks are taken in
  ascending name order so two imports sharing several names cannot deadlock.
- Reused Artists are excluded from the artist insert and the aggregate refresh recomputes over the
  Artist's whole association set, so counters stay truthful.
- The post-import reconciliation sweep now converges legacy duplicates for **every** credit form
  (primary Release and Track credits) with the unchanged conflict gates, predicate
  `repeated_credit`, and a reversible merge tombstone.

## Verified on a live instance

Two new corpus fixture pairs, ingested into a freshly reset dev database (release build, default
worker concurrency, four Releases in one batch):

| Fixture pair | Declared outcome | Observed |
|---|---|---|
| `12-artist-name-reuse-a` + `13-artist-name-reuse-b` | one Artist, credits from both Releases | **1** Artist (`provisional`), **2** Release credits, **0** merge events, 2 `reusing an existing Artist` log lines |
| `14-homonym-provider-a` + `15-homonym-provider-b` | two Artists, never merged | **2** Artists (`provisional`), one Release each, different MusicBrainz Artist ids preserved, no convergence candidate |

The negative fixture also proves the tag path end to end: ffmpeg writes `TXXX:MusicBrainz Artist Id`,
lofty surfaces it as `ItemKey::MusicBrainzArtistId`, the manifest carries
`raw.musicbrainz_ids.artist`, and the import refuses reuse on the conflict.

## What the live run caught that fixtures did not

The first live attempt produced **two** rows and one `artist_merge_events` row instead of one reused
Artist. The cause was a genuine race — the batch imported four Releases concurrently, so two imports
of the same name both took the "no candidate" branch. The reconciliation sweep then cleaned it up,
which is exactly the behaviour that hid the defect for so long: a converging system looks correct
after the fact while still creating, tombstoning, and auditing a public identity that should never
have existed.

`same_name_imports_serialize_on_the_artist_name_lock` now proves the serialization deterministically:
it holds the name lock in a separate transaction, asserts the importer blocks in
`pg_stat_activity.wait_event_type='Lock'`, and only then releases it. Removing the lock from
`reuse_existing_artists` makes that test fail with `Elapsed(())` wait timeouts, so it cannot rot into
a passing no-op.

## Documentation impact

- `docs/src/content/docs/library-management/artists.md` — reuse rule, its guards, and what the
  convergence pass still repairs.
- `docs/src/content/docs/admin/identity-review.md` — what import resolves on its own versus what the
  queue is genuinely for.
- `design/testing/UI-AND-WORKFLOW-AUDIT.md` — fixture matrix rows for the two new pairs.
- `design/adr/ADR-0189-…` supersedes ADR-0117's import rule and extends ADR-0175 to every credit form.
- `design/requirements/REQUIREMENTS-INDEX.md` — traceability for `PRD-ARTIST-010`,
  `PRD-ARTIST-011`, and `PRD-CATALOG-001`.

## What this does not prove

- A full state-level differential against the C# server (same media tree into both, diff the
  normalized entity graph) is still the structural warranty that a decision like ADR-0117 cannot
  silently diverge again. It is proposed, not built.
- Two real homonyms with no tag-level evidence are still unknowable: import reuses one row, exactly as
  the C# server does, and an administrator splits it if the catalogue reveals the mistake.
