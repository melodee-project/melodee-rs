# Definition of Done

A task is done when all applicable items below are satisfied.

## Requirements and scope

- [ ] The issue links to one or more PRD requirement IDs.
- [ ] Work stays within the current phase.
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

- [ ] Requirement traceability is updated.
- [ ] ADR status/references are updated.
- [ ] User/operator docs are updated when behavior changes.
- [ ] Evidence is reproducible from a clean checkout.
