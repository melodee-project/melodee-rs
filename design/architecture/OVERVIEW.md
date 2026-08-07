# Architecture Overview

The target is a modular monolith with explicit protocol adapters, a framework-independent domain,
PostgreSQL authoritative state, canonical media/manifests, and rebuildable projections.

```mermaid
flowchart TB
    Clients[Clients]
    Adapters[API adapters]
    Application[Application use cases]
    Domain[Domain invariants]
    Persistence[PostgreSQL persistence]
    Workers[Durable workers]
    Media[Media tools + storage]
    Search[Tantivy]

    Clients --> Adapters --> Application --> Domain
    Application --> Persistence
    Workers --> Application
    Workers --> Media
    Application --> Search
    Persistence --> Search
```

Dependencies point inward. Domain code never imports transport, persistence, or filesystem crates.
