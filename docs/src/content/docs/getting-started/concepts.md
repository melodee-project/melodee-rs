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

A Track is one playable occurrence within a Release (what compatibility APIs may call a "song").
Two Tracks may refer to the same underlying recording or performance through `AudioIdentity`.

## Audio identity

An `AudioIdentity` represents one underlying audible recording or performance. It is separate from
a Track occurrence and from the encoded bytes in a MediaAsset. A new catalog import creates
provisional identities conservatively; a byte hash or fingerprint is evidence, not an automatic
identity key.

## Contributions and credits

Artists relate to Releases and Tracks through explicit credits (`ReleaseArtist`, `TrackArtist`,
`Contribution`), so "everything an artist appears on" includes solo work, group work, and guest
appearances according to role.

## Media assets and manifests

A stable inbound candidate is first acquired into durable, Melodee-owned staging with a verified
handoff (atomic move on one filesystem; streamed hash-verified copy across filesystems), and only
then is the inbound candidate directory removed. The staged source is inspected and normalized into
attempt-isolated temporary files with a versioned manifest describing content hashes, fingerprints,
and metadata. Structural validation upgrades an accepted result to manifest schema v3, then atomic,
retry-safe publication creates an immutable-ID canonical Release bundle while retaining the durable
staged source. The catalog worker builds normalized PostgreSQL rows from that ratified manifest and
its publication receipt, not from repeated reads of audio files.

Published manifest v3 remains immutable. Resolved Artist, contribution, and AudioIdentity facts are
captured in an immutable semantic-overlay revision in PostgreSQL, so later corrections preserve the
original media and publication evidence.

## Compatibility surfaces

Melodee exposes three separate API products — the native Melodee API, OpenSubsonic, and a
Jellyfin-compatible API. They intentionally keep their own wire formats and terminology; `Album`
and `Song` appear only in those compatibility layers.

:::note[Terminology]
New documentation uses **Artist / Release / Track**. `Album` and `Song` are used only when
documenting compatibility APIs or quoting legacy client behavior.
:::
