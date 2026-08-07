# Storage and Manifests

Canonical storage is proposed to be Release-centric and sharded by stable Release ID. Database
relationships determine Artist views and contributions.

A manifest is the durable boundary between media analysis and catalog insertion. It must be versioned,
self-describing, and sufficient to rebuild catalog/search state without re-probing every media file.

Required manifest categories:

- source and canonical file maps;
- media properties and tool versions;
- hashes/fingerprints;
- observed and resolved identities;
- Release/Track presentation data;
- contributions;
- classification evidence;
- validation and policy decision references; and
- schema/normalization versions.

See ADR-0106 and ADR-0108 before implementation.
