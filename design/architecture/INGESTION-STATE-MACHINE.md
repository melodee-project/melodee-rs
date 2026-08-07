# Ingestion State Machine

The durable state machine is per Release. Classification and disposition are separate facts.

```mermaid
stateDiagram-v2
    [*] --> Discovered
    Discovered --> Settling
    Settling --> Ready
    Ready --> Inspecting
    Inspecting --> Normalizing
    Normalizing --> Validating
    Validating --> Staged
    Validating --> NeedsReview
    NeedsReview --> Staged
    NeedsReview --> Quarantined
    NeedsReview --> Ignored
    NeedsReview --> Deleted
    Staged --> Publishing
    Publishing --> Published
    Published --> Cataloging
    Cataloging --> Cataloged
    Cataloged --> Indexing
    Indexing --> Indexed
    Inspecting --> Failed
    Normalizing --> Failed
    Validating --> Failed
    Publishing --> Failed
    Cataloging --> Failed
    Indexing --> Failed
    Failed --> Ready
```

## Transition requirements

- Every transition is durable and timestamped.
- A worker holds a renewable lease while performing a stage.
- Lease expiry never implies success.
- Repeat execution is safe.
- Failure stores a machine category plus operator-safe diagnostic.
- Delete requires a prior durable authorized decision.
- Reprocess is explicit and records who/why reset the item.
