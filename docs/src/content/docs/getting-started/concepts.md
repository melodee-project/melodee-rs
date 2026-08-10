---
title: Concepts
description: Core Melodee concepts — Artists, Releases, Tracks, and the catalog model.
sidebar:
  order: 3
---

Melodee's catalog uses a small set of precise concepts.

## Artist

An Artist is an identity Melodee owns, not merely a name string. Two different bands with the same
name are two different Artists. A name found in file metadata is an *observation* that resolves to
zero, one, or multiple Artists through evidence and aliases.

## Release

A Release is a curated, release-centric unit of the catalog (what legacy clients and compatibility
APIs may call an "album"). Releases are what inbound processing, review, and policy decisions act
on.

## Track

A Track is a single recording within a Release (what compatibility APIs may call a "song").

## Contributions and credits

Artists relate to Releases and Tracks through explicit credits (`ReleaseArtist`, `TrackArtist`,
`Contribution`), so "everything an artist appears on" includes solo work, group work, and guest
appearances according to role.

## Media assets and manifests

Inbound media is inspected and normalized into attempt-isolated temporary files with a versioned
manifest describing content hashes, fingerprints, and metadata. Verification and atomic canonical
library publication are the next pipeline stage and are not implemented yet. The eventual catalog
is built from ratified manifests, not from repeated reads of audio files.

## Compatibility surfaces

Melodee exposes three separate API products — the native Melodee API, OpenSubsonic, and a
Jellyfin-compatible API. They intentionally keep their own wire formats and terminology; `Album`
and `Song` appear only in those compatibility layers.

:::note[Terminology]
New documentation uses **Artist / Release / Track**. `Album` and `Song` are used only when
documenting compatibility APIs or quoting legacy client behavior.
:::
