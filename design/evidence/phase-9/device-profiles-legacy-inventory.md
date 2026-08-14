# Phase 9 WP-10 — Device/transcoding profile legacy inventory

Legacy oracle: `/home/steven/src/github/melodee` at frozen commit
`4028b58dbd3302ed431e2c5b32d9dde5634db218` (verified 2026-08-13). Ground-up
rewrite rule applies: the C# code below is behavioral evidence, not an
architecture template. Governing authorities for the Rust implementation:
PRD-DEVICE-001/002/003 and ADR-0134 §5.

## Where the feature lives in legacy

- `src/Melodee.Common/Data/Models/UserDeviceProfile.cs` — EF model.
- `src/Melodee.Common/Services/UserDeviceProfileService.cs` — CRUD + default
  management + effective-profile resolver.
- `src/Melodee.Blazor/Components/Pages/Account/Profile.razor` and
  `src/Melodee.Blazor/Components/Dialogs/DeviceProfileDialog.razor` — the only
  management UI (per-user account page; Blazor server, not a public API).
- `src/Melodee.Common/Migrations/20260524160636_InitialBaseline.cs` — table
  shape and the seeded `userDeviceProfile.enabled` setting.
- `src/Melodee.Common/Constants/SettingRegistry.cs` —
  `UserDeviceProfileEnabled = "userDeviceProfile.enabled"`.

## Model fields, defaults, validation (proving files above)

`UserDeviceProfile` columns: `UserId` (int, required FK), `PlayerId`
(int?, nullable FK — null means user default), `IsDefaultProfile` (bool),
`Name` (required, `MaxLength(MaxGeneralInputLength)` = 255), `DirectPlay`
(bool), `TargetCodec` (string?, ≤ 255), `MaxBitrate` (int?, kbps; dialog
bounds 1–9999), `ResampleRate` (int?, Hz; dialog bounds 1000–192000),
`Priority` (int, default 0; dialog bounds 0–100), plus `DataModelBase`
audit fields. Indexes: unique `(UserId, PlayerId)`; non-unique
`(UserId, IsDefaultProfile)`.

Service validation (`CreateAsync`/`UpdateAsync`, identical rules):

1. `DirectPlay == true` with `TargetCodec != null || MaxBitrate != null` →
   validation failure ("DirectPlay profiles should not have TargetCodec or
   MaxBitrate set").
2. `DirectPlay == false` with blank `TargetCodec` → validation failure
   ("Transcoding profiles must have TargetCodec set").

Default management: on create/update with `IsDefaultProfile == true`, the
service unsets every other default row for that user before saving
(service-level, not a database constraint; the EF index is non-unique).

Listing order: default first, then `Priority` descending, then `Name`.

## User-visible behavior to preserve (PRD-DEVICE-001)

- Profile CRUD with name, direct-play/codec/bitrate/resample/priority
  fields and a default flag.
- At most one default profile; setting a new default clears the previous.
- The two validation rules above.
- Bounded name (255) and bounded numeric fields.

## Deliberately not preserved / not wired

- **Streaming does not consume profiles, in legacy either.**
  `GetEffectiveProfileAsync` (per-player > per-user default > global
  direct-play precedence) has **zero call sites** in the frozen tree, and the
  seeded `userDeviceProfile.enabled` setting is never read outside its
  definition/seed. This is exactly the PRD §35 "preview/partial" statement
  and PRD-DEVICE-002: the rewrite must not claim profile-driven transcoding.
  Profile-driven stream selection stays post-parity (PRD-DEVICE-003) and
  needs its own ADR.
- **Per-user/per-player ownership is not carried into the Rust schema.**
  ADR-0134 §5 fixes the Phase 9 table as server-wide operator metadata
  (`id`, `name`, `client_match` jsonb, `profile` jsonb, `is_default`,
  timestamps) managed on the admin surface; the legacy per-user Blazor
  account-page dialog has no public API contract to preserve (no captured
  route exists — the feature never crossed a frozen compatibility surface).
- **Caching-region invalidation mechanics** are an EF/IMemoryCache
  implementation detail, not user-visible behavior.
- The non-atomic legacy default-flip (load-modify-save in the service) is
  replaced by a transactional update plus a partial unique index — a bug
  class deliberately not preserved.
