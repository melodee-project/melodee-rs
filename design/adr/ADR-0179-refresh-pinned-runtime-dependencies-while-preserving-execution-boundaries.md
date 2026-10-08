# ADR-0179: Refresh pinned runtime dependencies while preserving execution boundaries

- **Date:** 2026-10-08
- **Status:** Accepted
- **Decision owners:** User-directed repository dependency maintenance
- **Supersedes:** ADR-0168
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-012, PRD-EXT-001, PRD-EXT-003, PRD-SCRIPT-003,
  PRD-SCRIPT-004, PRD-ARCH-006, PRD-SEC-001, PRD-TEST-008

## Context

The user requested current dependency versions and a complete passing precheck. Registry and
upstream release metadata were checked on 2026-10-08. The refresh includes security fixes,
new compiler diagnostics, and updated frontend tooling. DecentDB is pinned by an accepted ADR,
so its exact revision change must be recorded without rewriting the earlier decision.

## Decision

1. Pin DecentDB 2.17.2 at commit `b15ee763def5d696feb07d3ff29d467b7914a603`, superseding
   the version decision in ADR-0168. Preserve every read-only, bounded-query, blocking-worker,
   and disabled-Lua boundary from ADR-0163. Never open the configured replica for mutation.
2. Upgrade Boa to 0.22.0 with default features disabled. Preserve ADR-0135's capability-less
   host, bounded execution, timeout, output limits, and existing script-result semantics.
3. Refresh other direct dependencies to stable registry releases and transitive dependencies
   within upstream compatibility ranges. Retain exact direct pins and generated lockfiles;
   do not force unrelated transitive major versions beyond their parents' supported ranges.
4. Use Rust 1.99.0, pnpm 12.10.1, and Node 26.11.1 in the reference build environment.
   Resolve new compiler diagnostics without suppressing quality checks or changing wire DTOs.
5. Keep the docs checker on TypeScript 6.0.3: the latest `@astrojs/check` 0.9.10 and Astro
   7.3.7 explicitly reject TypeScript 7.0.2. Admin UI compilation uses 7.0.2; its existing
   TypeScript 6 lint-toolchain alias remains required. Revisit these compatibility constraints
   when upstream supports the stable TypeScript 7 compiler API.
6. Override vulnerable `postcss-selector-parser` versions below 7.1.6 to 7.1.6 in docs.
   Expressive Code still resolves postcss-nested 6 and parser 6. This removes
   GHSA-rj75-hqrm-r3gf without suppressing the audit. Remove the override when upstream permits
   the patched parser. Remove obsolete source-map-js, js-yaml, and nanoid overrides now that
   updated upstream dependencies resolve safe versions.

## Alternatives considered

### Retain outdated versions or suppress audit findings

Rejected. This leaves the reported failures unresolved and weakens the precheck contract.

### Force every transitive dependency to its newest major release

Rejected. A parent may depend on APIs absent from a newer major version. Upgrade the parent and
its supported graph, with a narrow documented security override where necessary.

### Replace the Astro checker with experimental TypeScript 7.1 tooling

Rejected. The alternative currently requires a prerelease compiler and would trade a working
stable documentation gate for experimental tooling.

## Rationale

Current stable dependencies and explicit compatibility exceptions keep builds reproducible and
maintain the FOSS, data-safety, and public-contract boundaries already accepted by this project.
The user-authorized version refresh adds no external service or new runtime capability.

## Consequences

### Positive

- Removes the known dependency-audit failures without ignoring advisories.
- Updates the compiler, frontend tooling, Python UI library, and runtime patch releases.
- Preserves exact source provenance for the replica reader.

### Negative / trade-offs

- The compiler refresh requires internal error-storage adjustments and full regression coverage.
- Astro and typescript-eslint still require the stable TypeScript 6 JavaScript API.
- The docs parser override needs review after future upstream upgrades.

### Follow-up work

Re-evaluate the documented compatibility exceptions during the next refresh. Supersede this
ADR before changing the DecentDB revision or expanding Boa's host capabilities.

## Validation

Run `scripts/do-prechecks.py` end to end: frozen installs, both JavaScript audits, Python tests,
Clippy, all Rust tests including the real DecentDB fixture and script sandbox tests, cargo-deny,
and the docs checker/link/build gate. Run Admin UI Playwright and visual tests for Carbon/React
changes. Synchronize tests with observable job completion and rendered controls instead of
relying on timing or accepting retries as a passing full gate.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [Technology baseline](../requirements/TECHNOLOGY-BASELINE.md)
- [ADR-0168](ADR-0168-upgrade-pinned-decentdb-replica-reader-to-2-17-1.md)
- [ADR-0163](ADR-0163-rust-native-musicbrainz-decentdb-replica-reads.md)
- [ADR-0135](ADR-0135-boa-javascript-runtime-and-sandboxing-for-scripting-hooks.md)
- [DecentDB 2.17.2](https://github.com/sphildreth/decentdb/releases/tag/v2.17.2)
- [PostCSS parser advisory](https://github.com/advisories/GHSA-rj75-hqrm-r3gf)
