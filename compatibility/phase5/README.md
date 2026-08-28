# Phase 5 classification and policy compatibility corpus

This corpus separates frozen legacy behavior from synthetic Phase 5 truth.

The legacy baseline proves only that directory names matching collection/type
keywords (including `greatest hits`) were classified as non-studio and held in
staging. It did **not** delete Releases because of those keywords. It also had
no authoritative AudioIdentity-based Track novelty model, so it cannot be an
oracle for `ExactDuplicate`, `KnownAudioIdentity`, `AlternateRecording`,
`NewTrack`, or `Uncertain` outcomes. The withdrawn `design/evidence/phase-0/media-baseline.md`
report recorded that baseline (see
[Withdrawn evidence reports](../../design/adr/README.md#withdrawn-evidence-reports)).

`greatest-hits-safety.json` is therefore a synthetic truth table governed by
ADR-0120, the ordinary-Release fallback correction in ADR-0177, and
PRD-POLICY-003 through PRD-POLICY-008. Tests may use it to prove:

- complete catalog novelty, rather than a title or path, controls the result;
- new, alternate, and uncertain Tracks never receive the redundant-collection
  disposition;
- directory-only and title-only evidence cannot authorize deletion; and
- an ordinary Release that matches no configured rule uses the non-destructive
  `Accept` fallback; and
- the legacy same-size-file deletion bug (documented in the withdrawn media-baseline
  report and ADR-0115) is not preserved.

The expected outcomes use the safe default preferences (`Ignore` redundant,
`Review` novel, `Review` uncertain). An explicit, audited Administrator
override is a separate test dimension and is not encoded as ambient actor data
in these cases.
