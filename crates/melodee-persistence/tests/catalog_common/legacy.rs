//! Frozen Phase 0 compatibility IDs used by Phase 4 migration proofs.
//!
//! These UUIDs are the exact deterministic public identifiers emitted by
//! `scripts/legacy_oracle.py` for `compatibility/fixtures/catalog-v1.json`.
//! Keeping the expected values literal ensures that migration tests detect a
//! namespace or mapping change instead of recomputing the same wrong value.

pub const ARTIST_PUBLIC_IDS: [(&str, &str); 10] = [
    ("aurora-fields", "813c11e0-61cd-5204-ae0f-ce68386eacec"),
    ("dune-plain", "5bf951d5-7afe-5c50-8941-145b7095902b"),
    ("dune-diacritic", "503f045f-1bd6-5ea7-9e32-14632344680c"),
    ("various-artists", "89eb2dc1-fd4c-5a57-99e5-480c96b292fc"),
    ("guest-vocalist", "eebcf6e2-4cbb-5177-be07-42a2dacec3b5"),
    ("harbour-lights", "4f6a5272-9529-5a24-92a0-a3ede6a73d59"),
    ("nora-beacon", "f993419e-f96f-5383-9c60-7f075c806943"),
    ("saeglopur", "8bf87c33-f669-5a85-a315-654f79fb895b"),
    (
        "obsidian-collective",
        "0bdebd7d-8597-5891-b6e4-88f7762370b9",
    ),
    ("cassette-ghost", "708790f0-57b5-5a45-a746-dd36634e8872"),
];

pub const RELEASE_PUBLIC_IDS: [(&str, &str); 10] = [
    ("northern-lights", "69d97699-74c5-5403-878f-87f42d9d9fbb"),
    ("dune-desert-rock", "263ebba5-739c-5cb9-987f-b2d2bbfbd0ad"),
    (
        "dune-danish-sessions",
        "b6716af3-7988-52bd-8e12-91fca722cf56",
    ),
    (
        "phase-zero-compilation",
        "97d569aa-9133-5351-b8c5-097cf57f96f9",
    ),
    ("hl-beacon", "c359d116-92ab-5380-a6dd-e97b65cce905"),
    ("duet-dialogues", "009e8915-175d-5689-943e-7b5e8cd10a62"),
    ("nb-solo-ledger", "4243b96d-1302-5cc9-82bf-395d76bf3ac8"),
    ("saeglopur-unicode", "7bbfff2a-36f2-5739-9a7a-e5b0d34af3b0"),
    (
        "anthology-of-echoes",
        "645680d3-704a-5b83-a734-42a3fa88c3ed",
    ),
    ("untitled-demos", "de0f93a5-2979-5654-9afe-16453caa49af"),
];

pub const TRACK_PUBLIC_IDS: [(&str, &str); 19] = [
    ("nl-01-solarwind", "2b8eab4b-235d-5892-99f8-c0f5f54e8ab4"),
    ("nl-02-polarveil", "ea26fc59-a545-51c2-94c4-702e1f97b80a"),
    ("nl-03-midnightsun", "e068e9f1-b7a1-58b9-9aaa-47c954694634"),
    ("dd-01-sandstone", "c418d383-13f5-598d-94a8-c15e5d2df348"),
    ("dd-02-caravan", "3e185e31-88b5-519b-b830-1ae915e80ba5"),
    ("dds-01-heltalrigt", "4c4e6173-b091-566a-82b5-3b69e8b043a4"),
    ("pzc-01-guestvocal", "828f864f-0d5a-572e-a6c4-69e96fa6983f"),
    (
        "pzc-02-stringsection",
        "781ea0e7-2cc5-5c7d-9c23-bcb486b6e2b1",
    ),
    ("pzc-03-saxsolo", "20cf62c1-fae9-5966-8193-4c4ca99ed749"),
    ("hlb-01-lantern", "d493e627-df71-50ed-93fb-c29b9f6ec96a"),
    (
        "dlg-01-callresponse",
        "ab76dea7-9b0d-5b70-930a-8e6c2824d164",
    ),
    ("nbs-01-tidepools", "e43ff3e7-3e4a-5168-9c31-cf2676eb6cb0"),
    ("su-01-neon", "b3297a8f-5e2b-5ade-98ea-f899d38059e0"),
    ("su-02-eple", "8c1a08ab-06dc-5438-a3b8-c1347a68791f"),
    ("aoe-101-gatefold", "0892324b-d44d-5bf5-a46c-8fe22ce5a38d"),
    ("aoe-102-marble", "e952d1c4-9de7-51cb-9954-83185eece2ba"),
    ("aoe-201-coda", "a39f05c4-2783-53c1-aa3c-e4e9d6a6f0a3"),
    (
        "aoe-202-recessional",
        "c28e96d1-04d4-5e6a-a984-1544a9a5ebd6",
    ),
    ("ud-01-hiss", "b666e766-49f7-54a2-8a13-5c265c6d8208"),
];
