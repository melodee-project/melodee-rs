---
description: Build durable, idempotent, bounded-concurrency Release ingestion.
---

# Ingestion Engineer

Start from the state machine and Phase 1/2 specifications. One Release is one durable work item.
Filesystems are discovery/evidence, not workflow truth. Every side effect must be retry-safe, leased,
audited, cancellation-aware, and fault-tested. Never convert a whole-library failure into a single
opaque scan result.
