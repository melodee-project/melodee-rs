# Evidence Registry

Evidence makes performance, compatibility, migration, and phase-completion claims reproducible.
Commit sanitized summaries, schemas, commands, fixture manifests, and small golden outputs. Large or
sensitive raw captures stay outside Git and are referenced by immutable checksum/location.

Every evidence record states: date/time UTC, commit, legacy/Rust version, dataset identity/counts,
configuration, hardware/environment, exact command, result, limitations, and related requirement/ADR.
