# Definition of Done

An assigned issue, work package, or work program is done when all applicable items below are satisfied.
For a program-level assignment, this checklist applies to the whole phase as well as to its internal
implementation slices. Completing one slice does not complete the assignment.

## program ownership

- [ ] The program objective, in-scope requirements, entry conditions, and exit criteria are named.
- [ ] The agent decomposed and ordered the program's internal workstreams without requiring a new
      user prompt for each workstream.
- [ ] Every in-scope workstream is implemented and evidenced, or has an explicit terminal
      disposition authorized by the applicable product decision or ADR.
- [ ] Prerequisite cleanup, ADR drafting, tests, documentation, traceability, and closeout evidence
      were treated as parts of the work program rather than reasons to reduce the assignment to a
      microtask.
- [ ] The completion report records the exit decision and any genuinely external follow-up; it does not
      present a passing intermediate checkpoint as phase completion.

## Requirements and scope

- [ ] The issue links to one or more PRD requirement IDs.
- [ ] Work stays within the current scope.
- [ ] Any scope change is explicit and reviewed.
- [ ] No product capability was silently removed or redefined.

## Architecture

- [ ] Accepted ADRs are followed.
- [ ] A new durable decision has an ADR.
- [ ] Core domain boundaries remain framework-independent.
- [ ] No speculative abstraction or empty crate was introduced.

## Compatibility

- [ ] External behavior was captured before changing it.
- [ ] Public IDs, routes, DTOs, errors, auth, and negative behavior are preserved where required.
- [ ] Approved differences are documented and tested.
- [ ] Real-client impact is stated.

## Code quality

- [ ] Formatting and clippy pass with warnings denied.
- [ ] No unsafe/unwrap/expect/panic/todo/unimplemented in production code.
- [ ] Errors preserve useful context without leaking secrets.
- [ ] Cancellation/backpressure/idempotency are addressed where relevant.

## Tests

- [ ] Lowest useful layer has automated tests.
- [ ] Integration/compatibility/fault/performance tests are added where applicable.
- [ ] Tests fail meaningfully when behavior regresses.
- [ ] Commands and results are recorded in the PR.

## Data and operations

- [ ] Schema changes include migration and integrity tests.
- [ ] Destructive behavior includes authorization, audit, and recovery/rollback.
- [ ] Observability and operator-facing failure messages are sufficient.
- [ ] Documentation/runbooks are updated.

## Dependencies and security

- [ ] New dependencies pass the FOSS gate and are justified.
- [ ] Lockfiles and dependency audits are current.
- [ ] Inputs, paths, child processes, tokens, and logs follow security rules.

## Documentation and evidence

User-facing documentation is a completion gate, not a future work. Public documentation lives
in `/docs`; internal engineering documentation lives in `/design`.

- [ ] Documentation impact has been evaluated.
- [ ] All affected user-facing documentation under `/docs` has been created or updated in this
      change. If documentation is genuinely unaffected, a short reason is recorded in the PR
      ("Documentation impact: None — <reason>").
- [ ] Configuration reference is updated for new/changed settings and defaults.
- [ ] API documentation is updated for new/changed API behavior.
- [ ] Upgrade/migration guidance is updated when behavior or storage changes require it.
- [ ] Examples and commands in documentation are validated against the implementation.
- [ ] Screenshots are updated when the UI they depict materially changed.
- [ ] Release notes/changelog are updated when the change is release-relevant.
- [ ] Requirement traceability is updated.
- [ ] ADR status/references are updated.
- [ ] Evidence is reproducible from a clean checkout.
