---
title: Native Melodee Clients
description: Clients that use the native Melodee API.
sidebar:
  order: 1
---

Native Melodee clients speak the Melodee API directly and get the full feature set. Known native
clients include the Melodee Android app (including Android Auto) and MeloAmp.

:::note[Compatibility]
The Rust rewrite preserves the native Melodee API v1 contract so existing clients keep working
without a synchronized client release. Client-facing behavior is validated by differential
compatibility tests before each release.
:::

Setup guidance per client will be validated with real-client smoke tests as the rewrite reaches
the API compatibility phases.
