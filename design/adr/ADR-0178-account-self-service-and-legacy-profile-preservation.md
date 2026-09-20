# ADR-0178: Account self-service and legacy profile preservation

- **Date:** 2026-09-20
- **Status:** Accepted
- **Decision owners:** User-directed MEL-2 implementation
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-AUTH-002, PRD-AUTH-003, PRD-USER-001, PRD-MIG-003,
  PRD-MIG-004

## Context

The legacy C# server exposes account registration and an authenticated profile page. The profile
surface stores username, email, `Description`, `TimeZoneId`, preferred language, preferred theme,
and an avatar named from the legacy numeric user id in the User Images library. The Rust server
must provide the equivalent capability for MeloAmp while keeping the frozen Native API v1,
OpenSubsonic, and Jellyfin contracts stable and leaving listener-facing web workflows to MeloAmp
(ADR-0161).

Migration must read the legacy database and User Images files without modifying them. A legacy
filesystem path is an input location only, never an identity. Invalid or unsafe profile/avatar
data must become an actionable migration issue rather than being silently discarded.

## Decision

1. The existing `register.disabled` and `register.privateCode` settings control native
   self-registration. When registration is enabled, `POST /api/v1/auth/register` accepts the
   C#-equivalent username, email, and password fields. The first successfully registered account
   is the only account bootstrapped as administrator; later registrations are ordinary users.
2. Additive authenticated routes `GET` and `PATCH /api/v1/user/me/profile` expose and update only
   the authenticated user's profile. The request has no role, administrator, enabled, locked,
   public-ID, or password fields; unknown fields are rejected. Validation is bounded by the C#
   storage contracts: username/email 255 characters, description 62,000 characters, timezone 64
   characters, language 10 characters, and theme 20 characters. Timezone identifiers are checked
   for safe canonical syntax and blank values resolve to `UTC`, matching the C# profile behavior.
3. Profile mutations write an append-only, value-free audit record in the same PostgreSQL
   transaction as the profile update. Existing access sessions remain valid; identity and role
   authority are not changed by a profile mutation.
4. Legacy user import maps `Description` and `TimeZoneId` without changing public IDs or password
   hashes. A malformed but bounded timezone is preserved and reported; an over-bound value is an
   issue and is not silently truncated.
5. Legacy User Images import accepts only the exact `<legacy-user-id padded to 8>.gif` ownership
   convention, direct regular files, bounded bytes, and decodable images. It normalizes accepted
   images through the existing bounded avatar pipeline to PNG (`image/png`, at most 1 MiB), keeps
   the source read-only, records unsafe/unmapped/unsupported files as issues, and skips an update
   when the normalized target bytes already match so retries are idempotent. Device-profile data
   remains reference-only under ADR-0148 and is not recreated here.

## Alternatives considered

### Reuse the frozen `/api/v1/user/me` DTO for profile updates

Rejected. That DTO is a frozen Native API v1 compatibility contract. A separate additive profile
resource lets MeloAmp evolve without changing existing property names or authorization behavior.

### Store legacy avatars as arbitrary files or retain GIF bytes

Rejected. Filesystem paths are not semantic identity, arbitrary files expand the trust boundary,
and the current target avatar store intentionally accepts only normalized JPEG/PNG bytes. GIF
decoding is performed once by the established image pipeline, with the resulting PNG as the
authoritative target representation.

## Rationale

This preserves the user-visible C# capability while respecting the modular Rust boundaries and
the MeloAmp ownership model. PostgreSQL remains authoritative, the migration is restartable and
auditable, and existing client contracts remain frozen. The first-admin rule is serialized in the
database so concurrent registrations cannot create two bootstrap administrators.

## Consequences

### Positive

- Users can register and manage their own supported profile fields without an elevation path.
- Migration retains representable profile values and produces actionable evidence for everything
  it cannot safely map.
- Existing Native API v1, OpenSubsonic, and Jellyfin wire contracts remain unchanged.

### Negative / trade-offs

- MeloAmp must use the new additive registration/profile routes; the Rust server does not recreate
  the legacy Blazor listener SPA.
- Profile audit rows intentionally contain changed-field names and actor identity, never profile
  values or passwords.

### Follow-up work

- Browser password-reset UI remains MEL-3.
- Per-user device-profile target semantics remain MEL-5.

## Validation

Migration 0042 adds the durable profile fields and audit table. PostgreSQL integration tests cover
registration settings, first-admin serialization, self-only mutation, validation, audit, and
restart durability. Legacy fixtures cover C# profile fields, GIF avatar normalization, unsafe and
unsupported files, issue reporting, and idempotent reruns. Native, OpenSubsonic, and Jellyfin
regression suites remain green with their existing wire contracts.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0124](ADR-0124-native-authentication-session-and-refresh-token-compatibility.md)
- [ADR-0134](ADR-0134-phase-9-parity-domain-schema-for-radio-device-profiles-onboarding-notifications-scripts-and-providers.md)
- [ADR-0148](ADR-0148-legacy-migration-contract.md)
- [ADR-0161](ADR-0161-meloamp-owns-listener-workflows-and-the-server-exposes-only-admin-and-narrow-html-surfaces.md)
