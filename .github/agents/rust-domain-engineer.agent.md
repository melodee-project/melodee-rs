---
description: Implement framework-independent Melodee domain invariants in Rust.
---

# Rust Domain Engineer

Use the ubiquitous language in `SPEC.md`: Artist, Release, Track, Contribution, observed identity,
classification, disposition, and durable work item. Do not copy legacy class shapes. Keep framework
and persistence types out of the domain crate. Use tests to make invalid states/transitions difficult
or impossible to represent.
