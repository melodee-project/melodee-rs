---
title: Settings
description: Server settings, notifications, scripts, radio, device profiles, and themes in the admin UI.
sidebar:
  order: 8
---

Open **System → Settings** (`/admin/system/settings`). The screen is organized into tabs, each
backed by the corresponding Admin API or native API surface. The full key-by-key configuration
contract lives in [Configuration](../installation/configuration/).

## Configuration

The configuration tab shows the effective server configuration with its source layer. There is
**no per-key editing** in the UI by design — settings change through environment/configuration
files and restart, or through the validated import flow:

- **Export** writes the redacted export document. Secrets render as a fixed redaction marker and
  are never shown or editable.
- **Import** is preview-first: the server validates the document and shows the proposed changes
  before anything is applied. Applying rejects the redaction marker as a value, unknown keys
  (unless explicitly allowed), and invalid values with stable error codes. Re-entering secrets
  happens only through an import document you author, never in the browser.

The CLI equivalents are `melodee-cli config list|validate|export|import`; the API contract is
[Admin API — Configuration export/import](../api/admin-api/#configuration-exportimport) and the
secret-handling rules are in
[Configuration — Import and export](../installation/configuration/#import-and-export).

### Does this setting actually do anything?

`GET /api/admin/v1/diagnostics/settings` lists every effective setting with its winning layer, its
registered kind, and either the runtime consumer that reads it or the reason it is deliberately
inert (retired capability, frozen compatibility constant, and so on). Use it when you changed a
value and saw no effect: a key with an `inertReason` never had a runtime effect; a key with a
`consumer` names the module that reads it. See
[Admin API — Runtime diagnostics](../api/admin-api/#runtime-diagnostics).

## Notifications

Lists the stored notification templates (key, locale, version, subject) and renders a **test
send** through the configured email transport, showing the honestly recorded audit row —
including `skipped` when delivery is disabled. The headless equivalents are
`melodee-cli notifications templates list` and `melodee-cli notifications test-send`; see
[Users and Access — Operator CLI](./users-and-access/#operator-cli).

## Scripts

Lists the installed JavaScript hooks with their wired/inert events, and provides
enable/disable, source editing (a plain textarea — no code editor is claimed), and **test run**
against an operator-supplied `ctx` mock. The sandbox contract, the `directoryProcessingStart`
hook payload, audit behavior, and the circuit breaker are documented in
[Scripting Hooks](./scripting-hooks/).

## Radio

Manages the Internet radio station directory: list, create, update, and delete stations. These
are the administrator routes documented under
[Admin API — Internet radio stations](../api/admin-api/#internet-radio-stations).

## Device profiles

Lists client device profiles as **operator-managed metadata only**. Nothing on this tab affects
streaming or transcoding behavior — the profiles are descriptive records, and the screen states
this contract explicitly.

## Theme

Lists the theme packs served by the native theme endpoints and applies one to your admin UI.
Administrators can also rescan the theme directory, import a pack (ZIP upload), export a pack,
and delete non-built-in packs. See
[Appearance and Localization](./appearance-and-localization/) for the two-axis theming model and
its token-mapping limitation.
