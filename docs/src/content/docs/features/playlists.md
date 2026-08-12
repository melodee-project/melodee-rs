---
title: Playlists
description: Playlists and smart playlists in Melodee.
sidebar:
  order: 1
---

Native Melodee clients can create, list, read, update, and delete playlists. Playlist items keep a
stable explicit order. Adding multiple Tracks and reordering a playlist are atomic: an invalid or
unknown Track does not leave a partially updated playlist.

Playlist reads are available to the owner and, when marked public, to other authenticated users.
Only the owner can change metadata, items, ordering, or artwork. Deleting a playlist removes only
the playlist and its derived item rows; it never deletes Tracks or media.

The native-v1 add-items route accepts both compatibility request shapes used by Melodee clients:
the canonical JSON array of Track IDs and the MeloAmp `{ "songIds": [...] }` object. Both forms
have the same bounded, ordered behavior.

## Import and artwork

M3U playlist import is available through the native API. Imports are size- and line-bounded and
resolve entries against catalog Track filenames; unmatched entries are reported rather than
inventing catalog identities.

Custom playlist artwork accepts bounded JPEG, PNG, or WebP bytes. Melodee verifies the content's
magic bytes instead of trusting the uploaded filename or content type. Removing artwork does not
affect playlist items.

## Smart playlists

Smart playlists store an owner-scoped, bounded [Melodee Query Language](../library-management/search-mql/)
expression and an entity type. Evaluating one uses the active search projection, returns a bounded
deterministic result, and records the latest result count. A missing or unhealthy search projection
fails closed; it does not turn the expression into an unbounded PostgreSQL or filesystem scan.
