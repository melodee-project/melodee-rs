---
title: Party Mode
description: Party mode and shared playback in Melodee.
sidebar:
  order: 5
---

The native-v1 playback endpoint registry is available for clients that select a browser or
server-side playback endpoint. Users see their own endpoints; session owners and DJs may also see
shared system endpoints. Only an owner or DJ can attach an endpoint, and users cannot detach another
user's endpoint. A recently active endpoint must be detached before another replaces it; stale
endpoints can be recovered without leaving the session permanently blocked.

:::caution[Remaining Party Mode scope]
The broader collaborative Party and Queue controller families are not compiled into the frozen
legacy server and remain intentionally unavailable. The endpoint registry described above is a
separately compiled native-v1 contract; it does not activate those absent route families.
:::
