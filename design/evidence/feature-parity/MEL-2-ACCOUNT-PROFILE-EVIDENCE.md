# MEL-2 account self-service and profile preservation evidence

- C# behavioral oracle: `4028b58dbd3302ed431e2c5b32d9dde5634db218`
- Rust audit candidate: `7ab01c515295d9f883585157287436f05d37f422`
- Decision: [ADR-0178](../../adr/ADR-0178-account-self-service-and-legacy-profile-preservation.md)

## Preserved behavior

The C# registration page uses `register.disabled` and an optional `register.privateCode`, requires
an email and password of at least six characters, and promotes the first registered account to
administrator. The authenticated profile page updates the same user's username, email,
`Description`, `TimeZoneId`, preferred language, preferred theme, and avatar. The avatar source is
the User Images library (`LibraryType.UserImages = 4`) and the filename is the zero-padded legacy
numeric user ID with a `.gif` suffix (`ToAvatarFileName`).

Rust keeps frozen `/api/v1/user/me`, OpenSubsonic, and Jellyfin DTOs/routes unchanged. MeloAmp uses
the additive `POST /api/v1/auth/register` and `GET/PATCH /api/v1/user/me/profile` routes. The
profile request has no role, administrator, account-state, public-ID, or password fields, and the
authenticated principal is the only subject addressable by the route. Profile and avatar changes
produce value-free audit rows. Existing access sessions remain valid and authority flags are not
changed by profile edits.

## Migration safety

Migration 0042 adds bounded durable `description` and `time_zone_id` fields and the append-only
profile audit table. User import preserves representable C# profile values and emits explicit
`profile.*` issues for malformed or over-bound values. User Images import:

- opens the source directory read-only and never uses a filesystem path as identity;
- accepts only direct regular `<id padded to 8>.gif` files owned by an imported source user;
- bounds source bytes, decodes through the existing bounded image pipeline, and stores normalized
  `image/png` bytes no larger than 1 MiB;
- reports missing/ambiguous libraries, unsafe/unmapped/unsupported files, read failures, and decode
  failures as durable migration issues; and
- checkpoints the phase and skips writes when normalized target bytes already match, making retry
  idempotent.

Per-user device-profile semantics remain the ADR-0148 reference-only disposition and are not
silently recreated by MEL-2.

## Evidence commands

The focused evidence was run with PostgreSQL on the documented development port:

```text
cargo fmt --all
cargo test -p melodee-persistence --all-features --test user_profile_mel2 -- --nocapture
cargo test -p melodee-server --all-features --test native_auth_api mel2_registration_profile_is_self_only_and_setting_controlled -- --nocapture
cargo test -p melodee-cli --all-features --test legacy mel2_profile_fields_and_user_images_avatar_are_safe_and_idempotent -- --nocapture
```

The tests cover disabled/enabled registration and private-code behavior, first-admin and ordinary
registration, anonymous/self-only/privilege-field authorization, bounded timezone validation,
audit and restart durability, C# profile fixture fields, normalized PNG content type/bytes,
unsupported and unmapped avatar issues, read-only source behavior, and an idempotent rerun.

## Cutover checks

Before a real cutover, the operator must mount the legacy User Images directory read-only at the
source `Libraries.Path`, run the import and final delta with the same run, inspect all `users` and
`user_avatars` issues, verify representative profile/avatar rows after restart, and run native,
OpenSubsonic, Jellyfin, and MeloAmp client regression probes. Production access, write freeze,
traffic switch, rollback, and cleanup remain operator-approved external gates.
