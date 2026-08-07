---
title: Configuration
description: Melodee server configuration sources and settings reference.
sidebar:
  order: 2
---

:::caution[Planned documentation]
The Rust server's configuration surface is still being implemented (configuration layering is a
Phase 1 deliverable). Settings documented here will be validated against the shipped server.
:::

Melodee configuration is layered, in increasing order of precedence:

1. built-in defaults;
2. configuration file;
3. environment variables;
4. database-backed settings, where a setting is runtime-editable.

The configuration reference will document every supported setting, its default, and its effect.
New settings introduced by a change must be documented here in the same change.
