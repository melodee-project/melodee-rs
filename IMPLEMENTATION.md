# Melodee implementation model

## Purpose

Melodee is a modular Rust server that replaces the C# Melodee server while preserving supported
observable behavior. Work is organized by product capabilities and requirement IDs, not by numbered
implementation stages.

Authority remains, in order: accepted ADRs, `SPEC.md`, `PRD.md`, this document, an assigned work
program, established implementation patterns, and the C# behavior oracle.

## Current status

The Rust server has functional feature parity with the current C# product, subject to the approved
differences recorded in `design/ROAD-TO-FEATURE-PARITY.md`. MeloAmp owns listener-facing web
workflows. Melodee serves the full administrative UI and the native, OpenSubsonic, and Jellyfin
interfaces.

Runtime feature implementation is complete. Remaining non-terminal requirements concern release
qualification: immutable multi-architecture artifacts, migration verification against private data,
performance and scale acceptance, differential validation, fault recovery, cutover, and rollback.

## Capability map

| Capability | Implementation boundary | Completion authority |
|---|---|---|
| Runtime, configuration, health, and diagnostics | Server and CLI composition roots | PRD runtime, configuration, and operations requirements |
| Inbound discovery and durable processing | Ingestion, persistence, media inspection, and normalization crates | Ingestion and storage requirements plus state-machine invariants |
| Catalog, identity, and curation | Domain, catalog persistence, policy engine, and Admin UI | Catalog, identity, metadata, and policy requirements |
| Search and MQL | Rebuildable Tantivy projection over PostgreSQL authority | Search and scale requirements |
| Native Melodee API | Dedicated native compatibility adapter | Native contract captures and approved compatibility decisions |
| OpenSubsonic | Dedicated OpenSubsonic compatibility adapter | Frozen captures, differential tests, and approved differences |
| Jellyfin music compatibility | Dedicated Jellyfin compatibility adapter | Frozen captures, differential tests, and approved subset boundary |
| Administrative workflows | Carbon Admin UI and admin API | Admin workflow, accessibility, localization, and operational requirements |
| Listener workflows | MeloAmp using supported server APIs | ADR-0161 and client acceptance evidence |
| Migration and release operation | Migration tooling, OCI packaging, runbooks, and operator controls | Migration, deployment, testing, and release-readiness requirements |

## Assignment model

A substantive assignment owns a complete outcome and all safe internal workstreams needed to meet
it. Small issues remain valid review units, but they do not narrow a program-level assignment. Use
`design/prompts/IMPLEMENT-WORK.md` for complete outcomes and
`design/prompts/IMPLEMENT-ISSUE.md` only for explicitly narrow work.

## Completion gates

A capability is complete only when its applicable requirements, production wiring, tests,
compatibility evidence, observability, data safety, and documentation satisfy
`design/agents/DEFINITION-OF-DONE.md`.

Operator-controlled actions are not runtime feature gaps. They remain open release-readiness gates
until an authorized operator executes and records them.
