---
title: Policies
description: Classification and policy rules governing inbound Releases.
sidebar:
  order: 3
---

Policies decide what happens to inbound Releases — accept, review, reject, quarantine — with
explainable, versioned rules. Policy decisions are audited *before* any destructive action, and
directory-name evidence alone can never trigger deletion.

:::caution[Planned documentation]
The policy engine is implemented in the rewrite's classification/policy phase. Rule syntax,
dry-run behavior, and override flows will be documented from the shipped implementation.
:::
