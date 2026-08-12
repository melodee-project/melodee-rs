---
title: Requests
description: User requests in Melodee.
sidebar:
  order: 3
---

Authenticated native Melodee clients can create, browse, edit, complete, and delete requests.
Requests may describe an Artist, Release, Track, or other catalog request and may carry stable
Melodee public IDs when the requested item is already known.

Request activity, comments, unread activity, and per-participant seen state are available through
the native API. Lists are bounded and deterministic. A user may mark a request seen only when they
created it or participated in its comments; unauthorized participant actions fail closed.

The request creator can complete a request explicitly. AddRelease and AddTrack requests are also
matched when a new catalog fact commits: stable public IDs take precedence, and an exact
Artist/title/year match can complete an unresolved request. Automatic completion writes the
resolved catalog public ID and one system activity comment in the same database transaction, so a
retry cannot duplicate its activity history.

These routes use native-v1 bearer authentication. See the [Melodee API](../api/melodee-api/) for
the compatibility contract.
