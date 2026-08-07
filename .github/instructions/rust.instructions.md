---
applyTo: "**/*.rs,**/Cargo.toml"
---

# Rust implementation instructions

- Use Rust 2024 and the pinned toolchain.
- Prefer clear ownership and explicit types over clever lifetime/generic designs.
- Production code must not use `unsafe`, `unwrap`, `expect`, `panic`, `todo`, or `unimplemented`.
- Use structured error types at domain/application boundaries and add context at infrastructure edges.
- Domain code must remain independent of web, async-runtime, database, and filesystem frameworks.
- Async code must be cancellation-safe. Bound concurrency and document backpressure.
- Avoid blocking Tokio workers; isolate CPU/blocking work.
- Public functions and non-obvious invariants require concise rustdoc.
- Add unit tests for domain invariants and integration/fault tests at side-effect boundaries.
- Run `cargo fmt`, clippy with warnings denied, and workspace tests.
