# ADR-0177: Accept ordinary Unknown Releases and reconcile the obsolete default review rule

- **Date:** 2026-08-28
- **Status:** Accepted
- **Decision owners:** User-directed catalog visibility correction
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-CLASS-001 through PRD-CLASS-003, PRD-POLICY-001 through
  PRD-POLICY-008, PRD-UI-005, PRD-UI-006, PRD-UI-007

## Context

ADR-0120 defines classification as belief and policy as action. Its built-in rules cover explicit
administrator overrides, corrupt media, and Greatest Hits novelty; it explicitly gives other traits
no guessed default. The implementation later added `POL-UNKNOWN`, which routed every Release whose
classification had no stronger evidence to `Review`.

Most ordinary Releases legitimately have no structured release-type evidence. The catch-all
therefore moved almost an entire newly imported library to `NeedsReview`, projected those Releases
as the internal catalog status `deprecated`, and withheld their Track streams and canonical artwork
from the shared client visibility boundary. The Admin UI then displayed the storage projection as
“Deprecated” instead of the actionable workflow state. This was both a policy-default defect and a
misleading operator experience.

Policy rule sets and decision audit are immutable. Correcting the default therefore requires a new
rule-set version and a narrowly attributed forward reconciliation rather than editing v1 or blindly
changing every reviewed Release.

## Decision

1. Built-in policy rule-set v2 removes `POL-UNKNOWN`. An ordinary Unknown classification with no
   corrupt media, explicit trait rule, Greatest Hits condition, or administrator override falls
   through to the existing non-destructive `Accept` result.
2. `UnknownClassification` remains an authorable rule type with `Review` as its only safe
   disposition. Operators who require review of all unclassified Releases can opt into it through
   an immutable custom rule-set version.
3. Rule-set v1 is obsolete and cannot be selected by current configuration. Startup reports the
   exact minimum version instead of silently continuing the defective default. The example and
   public configuration documentation select version 2.
4. Migration 0041 reconciles only the exact system-created v1 decision shape: stable built-in
   rule-set identity, sole matched rule `POL-UNKNOWN`, `Review`/pending decision, untouched system
   actor, `NeedsReview` work item, and `deprecated` Release. It changes that current decision to a
   completed `Accept`, restores the Release to `active`, routes the work item to `Indexing`, and
   appends work-item and decision-audit evidence.
5. The reconciliation actor is the typed `SystemUpgrade { migration, policy_rule_set_version }`.
   It is neither represented as an administrator action nor allowed to erase the original decision
   audit image. Operator overrides and every nonmatching policy outcome remain untouched.
6. Admin catalog Release payloads expose workflow state, policy disposition, and evidence summary
   alongside the internal catalog status. The UI presents `Needs review`, `Quarantined`, or
   `Ignored` when those workflow facts exist; it does not label all withheld Releases
   “Deprecated.”
7. Playback and artwork keep their existing `active` visibility fence. The fix restores ordinary
   Releases to that accepted state rather than bypassing access control for withheld Releases.

## Alternatives considered

### Serve media for deprecated Releases in the Admin UI

Rejected. It would mask the bad policy default and create a second visibility rule for media. A
signed URL authenticates a principal but must not bypass a Release disposition or current library
access.

### Edit the immutable v1 rule set or bulk-update every reviewed Release

Rejected. Editing v1 destroys the meaning of historical decisions. Updating all reviewed Releases
would overwrite deliberate custom-policy and administrator outcomes. The exact v1 system-decision
predicate gives the forward repair a closed, testable scope.

### Keep Unknown review as the built-in default

Rejected. Missing release-type evidence is normal, not evidence of corruption or risk. It also
conflicts with ADR-0120’s no-guessed-default rule for ordinary traits and creates an unbounded
operator queue without improving catalog safety.

## Rationale

The change restores the separation between observation and action: “Unknown” remains honest
classification evidence, while the absence of a configured policy outcome is the safe
non-destructive `Accept` fallback. Versioning and exact reconciliation preserve auditability. Using
the same `active` status fence also fixes Admin playback and artwork without weakening native,
OpenSubsonic, or Jellyfin authorization behavior.

## Consequences

### Positive

- Ordinary imported Releases become visible and playable without manual review solely because
  release-type metadata was absent.
- Operators can still choose Unknown review explicitly.
- Existing incorrect v1 decisions are repaired on upgrade with precise audit attribution.
- The Admin UI reports actionable workflow language rather than an internal storage projection.

### Negative / trade-offs

- Existing `.env` files pinned to version 1 must change the value to `2` or later; startup fails
  with an actionable configuration error until they do.
- The system-upgrade actor adds one internal policy-audit vocabulary value.

### Follow-up work

- None required for the accepted scope.

## Validation

- Pure policy tests prove the v2 default has no Unknown rule, ordinary Unknown falls through to
  `Accept`, and an explicitly authored Unknown rule still routes to `Review`.
- A forward-migration integration test seeds both the exact obsolete built-in decision and a custom
  reviewed decision, then proves only the built-in decision becomes active/indexing with retained
  audit history.
- Policy-worker integration proves an ordinary cataloged Release becomes active under v2.
- Admin API/native integration proves the resulting detail supplies a signed Track preview that
  streams canonical bytes and its canonical artwork is served, while existing tests retain the
  non-active media fences.
- Admin UI tests prove reviewed Releases render `Needs review` with the policy evidence.
- Full Rust, Admin UI, documentation, and end-to-end gates are required.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0120](ADR-0120-phase5-classification-novelty-and-policy-schema.md)
