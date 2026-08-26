# Coding-Agent Prompt — Road to Full C# Melodee Feature Parity

## Assignment

Create or update the following single authoritative audit document:

```text
design/ROAD_TO_FEATURE_PARITY.md
```

The document may already exist. Several coding agents, potentially using different models, will
review and update it over time. **Never assume an existing document is accurate merely because it
is detailed or cites tests.** Read it completely, independently validate its claims against both
codebases, correct inaccurate statements, add missing capabilities, remove duplicate rows, and
retain evidence that you have personally verified.

Do not ask the user to manually enumerate features. Discover the feature set from the C# source,
composition roots, UI, configuration, jobs, tests, and documentation, then independently discover
the operational feature set in the Rust rewrite and compare them.

This assignment may modify only `design/ROAD-TO-FEATURE-PARITY.md`. Do not implement missing
features, change requirements, edit ADRs, or adjust completion statuses elsewhere. Do not modify
the C# repository under any circumstances.

## Objective and definition of parity

The product objective is **full user-visible and operational feature parity with the C# Melodee
application in place today**, while retaining the Rust architecture and the accepted principle
that Melodee owns semantic identities. This is broader than API response-shape compatibility.

A C# capability does not cease to be a parity obligation merely because:

- no frozen API capture exercised it;
- a first-party client did not use it during an earlier acceptance run;
- it is implemented in an admin UI, background job, ingestion pipeline, or provider plugin rather
  than a public API;
- an ADR deferred, waived, or excluded it from an earlier delivery milestone;
- the Rust repository preserved its setting names or database schema;
- a Rust unit test proves an interface or pure function in isolation.

An accepted deferral or waiver explains why a difference exists. It does **not** prove feature
parity. Keep such differences visible and identify the governing decision.

Audit behavior, not C# architecture. Do not recommend translating C# classes or service
boundaries line-for-line. Map legacy `Album` behavior to the Rust `Release` domain and legacy
`Song` behavior to the Rust `Track` domain where appropriate.

## Repositories and baselines

Use these defaults unless the invoking user supplies different locations:

- Rust rewrite: the repository containing this prompt.
- Current C# Melodee: `/home/steven/src/github/melodee`.

At the start of every audit:

1. Read the Rust repository's `AGENTS.md` completely and follow it.
2. Confirm both repository roots without changing either checkout.
3. Record the exact `HEAD` commit, current branch or detached state, and whether each worktree is
   clean or dirty.
4. Treat the C# repository's current `HEAD` as the primary product baseline unless the user named
   a release or commit explicitly.
5. Record the Rust `HEAD` and audit the files actually present in its current worktree, including
   pre-existing uncommitted changes. Never reset, discard, overwrite, or attribute unrelated
   changes to yourself.
6. Treat the frozen C# commit used by the compatibility oracle as historical evidence, not as a
   substitute for reviewing the current C# baseline.

If the C# repository is missing or unreadable, do not fabricate an inventory from Rust planning
documents. Record the audit as blocked and report the missing prerequisite.

The C# repository is a **read-only behavior oracle**. Do not edit it, run formatters over it,
install packages into it, update lockfiles, create migrations, or run commands that rewrite
tracked source. Prefer static inspection. If executing legacy tests would create build artifacts
or otherwise mutate that checkout, do not run them unless the user separately authorizes it.

## Mandatory context

Before judging parity, inspect at least:

- `AGENTS.md`
- `PRD.md`
- `SPEC.md`
- `IMPLEMENTATION.md`
- every current outstanding/feature-parity document
- `design/requirements/FEATURE-PARITY-MATRIX.md`
- `design/requirements/REQUIREMENTS-INDEX.md`
- applicable accepted ADRs, especially any ADR cited as a waiver, deferral, exclusion, or
  intentional compatibility difference
- the existing `design/ROAD_TO_FEATURE_PARITY.md`, when present

These Rust documents are claims and governing context, not proof that code is operational. Source
wiring, realistic tests, and observable behavior are the implementation evidence.

## Multi-agent update protocol

Because several agents may run this prompt, follow these rules whenever the target document
already exists:

1. Read the entire latest file before investigating or editing.
2. Note its recorded baseline commits and compare them with the current checkouts.
3. Independently verify every existing row you change. Do not merely restate a previous agent's
   conclusion.
4. Preserve already verified evidence unless current source disproves it.
5. Correct false positives aggressively, especially claims based only on scaffolding, mocks,
   schemas, settings, DTOs, or isolated unit tests.
6. Use the existing stable feature ID when updating a row. Do not create a second row for the same
   capability just because you disagree with its status.
7. When evidence conflicts and cannot be resolved, mark the row `DISPUTED` or `NOT ASSESSED`,
   explain the conflict, and do not choose the more optimistic interpretation.
8. Re-read the target immediately before applying changes so that you do not replace newer work
   with an older snapshot.
9. Append an audit-log entry identifying the baselines, domains reviewed, corrections made, and
   remaining unreviewed scope. Do not claim a comprehensive audit when you reviewed only selected
   domains.
10. Never replace a more complete document with a shorter summary. Reconcile and improve it in
    place.

## Evidence standard

Inventory the C# application first, independently of the Rust implementation. This avoids shaping
the legacy feature list around what the rewrite happens to contain. Only then inventory Rust and
build the crosswalk.

For each claimed C# feature, establish all applicable facts:

1. **Reachability:** where a user, administrator, client, worker, scheduler, or CLI invokes it.
2. **Composition:** where the concrete service/plugin/handler is registered and enabled.
3. **Behavior:** the implementation path and material success/failure semantics.
4. **Configuration:** relevant settings and whether code actually consumes them.
5. **Persistence or side effects:** data written, files produced, external services called, or
   visible results returned.
6. **Evidence:** tests, UI routes, API captures, or documentation corroborating the behavior.

Apply the same standard to Rust. A feature is not operational merely because a similarly named
type or endpoint exists.

### Evidence that is insufficient by itself

Never mark parity proven solely from any of the following:

- a trait, interface, DTO, domain model, enum, or error type;
- a database table or migration with no production writer and reader;
- a recognized configuration key with no runtime consumer;
- a fake, stub, mock-only, or test-only implementation;
- a unit test that invokes an otherwise unwired component directly;
- a hard-coded provider/capability list;
- an API route that returns placeholder, empty, compatibility-shaped, or pre-seeded data;
- a cache exercised only by cache tests;
- successful compilation or a large total test count;
- an implementation report, completion report, checklist, ADR waiver, or requirement status;
- preservation of imported provider identifiers without live provider behavior;
- one compatibility surface when the same capability has separate native, OpenSubsonic,
  Jellyfin, admin, CLI, worker, or UI behavior.

### Proof required for `PROVEN PARITY`

Use `PROVEN PARITY` only when the relevant behavior:

- exists in the current C# baseline;
- has a reachable and configured Rust production path;
- preserves the material user-visible result and failure behavior, or documents an explicitly
  approved improvement;
- persists and retrieves the required state where applicable;
- is covered by a meaningful test, capture, or reproducible end-to-end scenario; and
- is not merely represented by dormant infrastructure.

If implementation exists but realistic runtime proof is absent, use `IMPLEMENTED — UNPROVEN`,
not `PROVEN PARITY`.

## Status vocabulary

Use exactly these statuses in the detailed matrix:

| Status                   | Meaning                                                                                                        |
| ------------------------ | -------------------------------------------------------------------------------------------------------------- |
| `PROVEN PARITY`          | Reachable Rust production behavior has evidence matching the current C# capability.                            |
| `IMPLEMENTED — UNPROVEN` | A plausible production path exists, but end-to-end or differential proof is incomplete.                        |
| `PARTIAL`                | Only part of the C# workflow exists, or supporting infrastructure is present but material behavior is missing. |
| `MISSING`                | No operational Rust equivalent was found.                                                                      |
| `DIFFERENT — UNAPPROVED` | Rust behavior materially differs and no applicable product approval was found.                                 |
| `DIFFERENT — APPROVED`   | A material difference is governed by an explicit accepted decision; it remains visibly non-identical.          |
| `DISPUTED`               | Available evidence conflicts and further investigation is required.                                            |
| `NOT ASSESSED`           | The capability has been inventoried but not adequately evaluated in Rust.                                      |

`IMPLEMENTED — UNPROVEN`, `PARTIAL`, `MISSING`, `DIFFERENT — UNAPPROVED`, `DISPUTED`, and
`NOT ASSESSED` are outstanding work for a feature-parity/code-complete claim. Keep
`DIFFERENT — APPROVED` separate in summary counts so the product owner can see exactly which C#
features are intentionally absent or changed. Do not silently count it as identical parity.

Dead, unreachable, or unregistered C# code is not automatically a product feature. Put it in a
separate “Legacy code investigated but not active” appendix, with evidence showing why it is not
reachable.

## How to discover the complete C# feature inventory

Use `rg`/`rg --files` first. Trace actual composition and callers rather than relying only on
names. Inspect, as applicable:

- solution and project structure;
- application startup and dependency-injection registration;
- controllers, route attributes, endpoint maps, OpenAPI output, and authorization policies;
- Blazor pages/components, navigation, dialogs, buttons, forms, and administrator actions;
- CLI commands, options, and maintenance entry points;
- hosted services, queues, schedulers, Quartz jobs, event handlers, and message consumers;
- ingestion/scanning/staging/publication flows;
- metadata readers/writers, sidecar formats, and filename/folder conventions;
- search, identity, image, metadata, scrobbling, and other plugin registrations;
- setting registries, seeded defaults, secret settings, and each setting's runtime consumers;
- database entities/migrations together with production services that use them;
- import/export, backup/restore, migration, and repair workflows;
- API clients and first-party client expectations;
- tests that reveal success, failure, retry, authorization, and compatibility behavior;
- operator and user documentation, checked against source rather than trusted blindly.

Group low-level implementation details into user-meaningful capabilities, but keep rows atomic
enough that one status describes the whole row. For example, Last.fm linking/scrobbling and
Last.fm metadata lookup are separate features; iTunes podcast discovery and iTunes music metadata
search are separate features.

## Required audit domains

The document must cover every applicable domain below. Add domains discovered in C# rather than
forcing all behavior into this initial taxonomy.

1. Installation, deployment, startup, onboarding, health, and diagnostics.
2. Configuration layering, settings administration, secrets, import/export, and defaults.
3. Authentication, users, roles, groups, permissions, password recovery, and linked identities.
4. Library/source configuration, filesystem layout, storage policies, and reconciliation.
5. Discovery, settling, acquisition, inspection, normalization, validation, publication, retry,
   quarantine, review, and deletion workflows.
6. Audio/tag metadata parsing and writing, CUE/NFO/playlist/lyrics/sidecar support, embedded IDs,
   and file/path naming behavior.
7. Artist observation, identification, provider matching, aliases, relationships, memberships,
   contributions, merge/split, and negative decisions.
8. Release identification, classification, external identities, duplicate/reissue/edition
   handling, metadata editing, and curation.
9. Track and AudioIdentity creation, embedded recording IDs, fingerprints, duplicate/alternate
   recording handling, technical metadata, and metadata editing.
10. Metadata/search/enrichment providers, including all configured C# provider purposes,
    credentials, enablement, rate limiting, caching, provenance, refresh, and failure degradation.
11. Embedded, sidecar, provider-discovered, generated, uploaded, overridden, and served artwork.
12. Catalog browsing, filtering, sorting, search, MQL, statistics, analytics, and discovery.
13. Playback, byte ranges, streaming, transcoding, downloads, media authorization, and client
    profiles.
14. Native Melodee API, OpenSubsonic, and Jellyfin behavior as separate compatibility products.
15. Playlists, smart playlists, play queues, favorites, ratings, hates, pins, bookmarks, play
    history, and other user state.
16. Scrobbling, now-playing, Last.fm account lifecycle, and Last.fm metadata behavior.
17. Podcasts, episodes, downloads, refresh, cleanup, progress, and podcast discovery providers.
18. Internet radio, charts, recommendations, top content, and scheduled discovery refresh.
19. Shares, requests, comments, notifications, email, and other social/operator workflows.
20. Scripting, extension hooks, metadata plugins, validation hooks, and sandbox behavior.
21. Scheduled jobs, background maintenance, cache cleanup, index rebuild, and operational
    controls.
22. Admin UI pages and every material read/mutate workflow, including confirmation, error,
    localization, accessibility, responsive, and theme behavior where present in C#.
23. Audit history, logging, metrics, diagnostics, retry visibility, and failure explanation.
24. Legacy migration, public-ID preservation, configuration/provider migration, backup, restore,
    verification, rollback, and repair tooling.
25. Packaging, upgrades, release artifacts, and documented local-production operation.

## Known high-risk claims to validate independently

Do not copy the following as conclusions. Treat them as mandatory investigation targets because
prior reviews found evidence of misleading completion claims:

- Whether C# ingestion invokes configured Artist and Release metadata providers while Rust
  ingestion does not.
- Whether MusicBrainz Artist, Release, Release Group, Track, and Recording identifiers embedded in
  media are retained and used.
- Whether the Rust local Artist resolver and provider-neutral authority boundary have any concrete
  production caller or provider implementation.
- Whether `external_provider_cache` receives live metadata lookup traffic or is test-only.
- Whether native `/api/v1/artist-lookup` calls providers or only projects already stored rows.
- Whether Artist identities are reused across Releases or only within one exact singleton
  Release/Track context.
- Whether Chromaprint fingerprints produce durable cross-Release AudioIdentity matches or are
  merely retained observations.
- Whether Release matching uses provider/catalog evidence or always allocates a new Release per
  publication.
- Whether provider settings retained from C# are operational or inert.
- Whether provider-discovered Artist/Release artwork works, distinct from embedded artwork and
  administrator upload overrides.
- Whether full metadata-edit forms and their persistence/audit behavior exist.
- Whether provider functionality was labeled complete through API-only captures or ADR waivers.

## Required structure of `design/ROAD_TO_FEATURE_PARITY.md`

The target document must remain useful to a product owner and to implementation agents. Use this
structure:

### 1. Purpose and honest current verdict

State what “feature parity” means, whether the audit is complete, and whether v1.0.0 code
completeness can currently be asserted. Never claim code complete while any required domain is
unassessed or any blocking row is unresolved.

### 2. Audited baselines

Include a table containing:

- repository;
- filesystem location used for the audit;
- exact commit SHA;
- branch/tag when available;
- clean/dirty state;
- audit timestamp; and
- role of the baseline (current C# product, Rust candidate, or historical oracle).

Do not embed credentials, environment values, private media paths, or unrelated diff content.

### 3. Method and status legend

Include the status definitions from this prompt and explain that ADR waivers are reported as
differences rather than proof of parity.

### 4. Executive summary

Include counts for every status, the number of v1 blockers, the number of intentionally approved
differences, and the number of unassessed/disputed rows. Summarize the highest-risk missing
workflows without hiding them in prose.

### 5. Domain coverage summary

For every required audit domain, record whether its C# inventory and Rust comparison are complete,
the row count, blocking-gap count, and last reviewer/date. An incomplete domain prevents a
“complete inventory” claim.

### 6. Detailed feature-parity matrix

Use one canonical table with at least these columns:

| Feature ID | Domain | C# Melodee feature and observable behavior | C# evidence | Rust status | Rust implementation evidence | Runtime/test evidence | Exact gap or difference | Requirement/ADR | v1 blocker |
| ---------- | ------ | ------------------------------------------ | ----------- | ----------- | ---------------------------- | --------------------- | ----------------------- | --------------- | ---------- |

Requirements for each row:

- Assign a durable, domain-oriented ID such as `INGEST-001` or `PROVIDER-004`. Do not use temporary
  phase, wave, work-package, `R3`, or `R4` labels.
- Describe the user-visible or operational behavior, not merely a class name.
- Give precise C# evidence as a path relative to the C# repository, current line number, and
  stable type/member name, for example:
  `src/Melodee.Common/.../Example.cs:120 (ExampleService.DoWorkAsync)`.
- Give precise Rust evidence as repository-relative file, current line number, crate/module, and
  symbol, for example:
  `crates/melodee-example/src/lib.rs:88 (ExampleStore::do_work)`.
- When a path has several relevant call sites, include the composition root and core
  implementation, separated by `<br>`.
- Do not use a documentation claim as the sole Rust implementation evidence.
- Put black-box, differential, integration, or realistic workflow evidence in the runtime/test
  column. Identify isolated unit tests as such.
- State exactly what must be implemented or proven next. Avoid “finish parity” or similarly vague
  gap descriptions.
- Link applicable PRD IDs and ADRs, but do not let an ADR waiver change `MISSING` into
  `PROVEN PARITY`.
- Mark `v1 blocker` as `Yes`, `No`, or `Product decision required`, with a short reason.

Use inline code for source locations rather than fragile Markdown links outside this repository.
Line numbers are audit-time evidence; stable symbol names must accompany them because lines drift.

### 7. Ordered v1 parity backlog

Derive this section mechanically from all blocking matrix rows. Group work into coherent,
dependency-ordered deliverables. For each deliverable list:

- included Feature IDs;
- concrete outcome;
- prerequisites and governing decisions that must be superseded or added;
- expected code areas;
- required unit/integration/differential/end-to-end evidence; and
- completion test from a user's perspective.

Do not invent effort estimates unless the user requests them. Do not call a feature implemented in
this section; it is outstanding work.

### 8. Approved and proposed differences

List every C# behavior intentionally changed or excluded, its rationale, governing decision, user
impact, and whether the product owner has explicitly accepted it for v1. Keep approved differences
visible even when they do not block the agreed release scope.

### 9. Disputed and unassessed areas

Record evidence conflicts, missing repositories/data/configuration, tests that could not run, and
domains not yet audited. Optimistic assumptions are prohibited.

### 10. End-to-end v1 acceptance journeys

Define executable acceptance journeys that collectively prove the product, including at minimum:

1. clean installation and empty-database onboarding;
2. administrator creation and configuration;
3. configuration of providers available in the C# baseline;
4. ingestion of representative tagged and untagged Releases;
5. Artist/Release/Track identification, external-ID and artwork verification;
6. ambiguous identity review and metadata correction;
7. browsing and streaming with MeloAmp;
8. supported OpenSubsonic and Jellyfin clients;
9. restart/failure/retry convergence during ingestion;
10. backup, restore, search rebuild, and catalog verification; and
11. upgrade/migration from supported C# data when that is part of the release scope.

Map every journey back to Feature IDs. A journey may be planned until the implementation exists,
but the document must clearly distinguish planned proof from passing proof.

### 11. Legacy code investigated but not active

Record apparent features that were excluded from the main matrix because the current C# baseline
does not register, call, expose, or enable them. Include the evidence supporting that conclusion.

### 12. Rust-only capabilities

Record material Rust improvements or new features separately. They are valuable context but do not
offset missing C# behavior.

### 13. Audit log

Append one entry per agent run with:

- timestamp;
- agent/model identifier when known;
- C# and Rust commit SHAs;
- domains and Feature IDs reviewed;
- rows added, corrected, downgraded, upgraded, or disputed;
- validation commands run; and
- remaining scope.

Do not include chain-of-thought or private reasoning. Record concise evidence and conclusions.

## Cross-checks before calling the inventory complete

Before the document may state that its feature inventory is complete, verify all of the following:

- Every reachable C# UI page and material action maps to at least one matrix row.
- Every C# controller/endpoint group maps to a row or to the inactive-code appendix.
- Every C# CLI command and scheduled/background job maps to a row.
- Every registered C# provider/plugin and each distinct purpose maps to a row.
- Every materially consumed C# setting family maps to its owning feature.
- Every persistent user-facing C# domain maps to a row.
- Native, OpenSubsonic, Jellyfin, admin, worker, and CLI behavior are not conflated.
- C# success paths, disabled/unconfigured behavior, and material failure/retry behavior are
  represented.
- Rust production wiring has been traced from an entry point or composition root to the concrete
  implementation.
- Rust-only scaffolding has not been reported as parity.
- All matrix rows contain both C# and Rust source evidence or are explicitly `NOT ASSESSED`.
- Summary counts exactly match the detailed rows.
- Every blocking row appears in the ordered parity backlog.
- Every accepted omission appears in the differences section.
- Every unreviewed domain appears in the coverage and unassessed sections.

Also perform a reverse review of material Rust capabilities. Record genuine Rust-only improvements
in their own section, but never use them to cancel or numerically offset a missing C# feature.

## Handling conflicts and decisions

- When current C# behavior conflicts with `PRD.md`, `SPEC.md`, or an accepted ADR, document the
  conflict. Do not silently decide which behavior to ship.
- When an ADR deferred a real C# feature, retain the feature's factual `PARTIAL` or `MISSING`
  status and cite the ADR as the explanation.
- When preserving a C# bug would violate security, data safety, or an accepted do-not-preserve
  decision, describe the behavior as `DIFFERENT — APPROVED` or `DIFFERENT — UNAPPROVED` based on
  actual evidence.
- Never edit decision history to make the matrix appear green. A future implementation task may
  supersede an ADR through the normal process.
- Do not treat the legacy architecture as binding. The goal is equivalent behavior under Rust's
  domain and safety invariants.

## Audit workflow

Use this order:

1. Capture both baselines and repository state.
2. Read mandatory context and the entire existing target document.
3. Build or validate the C# capability inventory without consulting Rust matches.
4. Build or validate the Rust operational inventory from production entry points inward.
5. Cross-reference features and assign conservative statuses.
6. Verify high-risk claims and all rows whose existing status is optimistic.
7. Reconcile the detailed matrix, summaries, backlog, acceptance journeys, and audit log.
8. Recalculate all counts from the final rows.
9. Run the smallest appropriate documentation validation, at minimum:

   ```text
   python3 scripts/validate_repository.py
   ```

10. Inspect the final diff and ensure only `design/ROAD_TO_FEATURE_PARITY.md` changed as part of
    this assignment.

If available context or time is insufficient for a complete audit, deliver a truthful incremental
update. Mark untouched areas `NOT ASSESSED`, update coverage, and report exactly what remains. A
partial evidence-backed audit is preferable to a fabricated comprehensive one.

## Final response requirements

Report:

- the file changed;
- exact C# and Rust baseline commits;
- domains reviewed;
- feature counts by status;
- rows whose status was materially corrected;
- newly discovered v1 blockers;
- disputed or unassessed areas;
- commands and tests run with results;
- compatibility and product-decision impacts;
- whether any ADR must eventually be superseded, without editing it in this audit;
- remaining risks; and
- documentation impact using the repository-required wording.

Do not state that full feature parity or v1.0.0 code completeness has been achieved unless the
document is comprehensive, every required row is `PROVEN PARITY` or an explicitly product-owner-
accepted difference, all acceptance journeys pass, and no blocking, disputed, or unassessed item
remains.
