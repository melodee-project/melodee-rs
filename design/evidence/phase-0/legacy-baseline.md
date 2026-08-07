# Phase 0 Evidence — Legacy Baseline Freeze

**Issue:** FIRST-10-ISSUES.md Issue 2 — Freeze and document the legacy baseline
**Requirements:** PRD-TEST-001, PRD-TEST-003
**Recorded:** 2026-08-07
**Recorded by:** Phase 0 implementation agent

## 1. Frozen baseline identity

| Attribute | Value |
|---|---|
| Repository | https://github.com/melodee-project/melodee |
| Local checkout | `/home/steven/src/github/melodee` (read-only oracle; never mutated by capture tooling) |
| Branch | `sph.2026-07-11.01` (project-owner working branch) |
| Commit | `4028b58dbd3302ed431e2c5b32d9dde5634db218` |
| Commit date | 2026-07-14 15:41:03 -0500 |
| Commit subject | `chore(deps): upgrade Microsoft.OpenApi to 2.10.0 and document 2.x pin decision` |
| `git describe` | `v2.1.4-19-g4028b58d` (19 commits after tag `v2.1.4`) |
| Relationship to `origin/main` | Baseline is **7 commits ahead** of `origin/main` (`d91342e5`); `origin/main` is an ancestor of the baseline. The baseline is the owner's current development line and reflects the newest observable behavior. |
| Working tree at capture | Clean (no uncommitted changes) |

### Discovery — baseline is not `main`

The frozen commit is on the working branch `sph.2026-07-11.01`, ahead of `origin/main`. This is
intentional: the PRD defines the baseline as "current `main` behavior and Melodee 2.2.x
documentation", and this branch is the source of current observable 2.2.x behavior (the project
version stamp is already `2.2.0`). All captures must record commit `4028b58d` so a later
`main`-vs-branch divergence is detectable.

## 2. Version metadata

| Component | Value | Source |
|---|---|---|
| Product version prefix | `2.2.0` | `src/Melodee.Common/Melodee.Common.csproj` (`VersionPrefix`) |
| Informational version format | `2.2.0+build<yyyyMMddHHmmss>` (+ source revision for the Blazor app) | `Melodee.Common.csproj`, `Melodee.Blazor.csproj` |
| Runtime version endpoint | `GET /api/v1/system/info` reports major/minor/patch from the assembly version | `src/Melodee.Blazor/Controllers/Melodee/SystemController.cs` |
| .NET SDK | `10.0.104` pinned, `rollForward: latestPatch`; host has `10.0.110` installed | `global.json` |
| OpenAPI endpoint | `/openapi/v1.json` (default `MapOpenApi` route) | `src/Melodee.Blazor/Program.cs:707` |

Because the informational version embeds a build timestamp, it is **volatile** and must be
normalized in compatibility comparisons. Commit `4028b58d` is the stable baseline identifier.

## 3. Database baseline

| Attribute | Value | Source |
|---|---|---|
| Engine | PostgreSQL 17 (`docker.io/library/postgres:17` in legacy `compose.yml`) | `compose.yml` |
| Access layer | EF Core with code-first migrations | `src/Melodee.Common/Migrations/` |
| Initial migration | `20260524160636_InitialBaseline` | migrations directory |
| Latest migration at freeze | `20260714155936_AddDeleteSourceResidueAfterIngestSetting` | migrations directory |
| Model snapshot | `ArtistSearchEngine/MelodeeDbContextModelSnapshot.cs` | migrations directory |

The migration pair list is the schema-version fingerprint. A baseline database is "the same schema"
when `__EFMigrationsHistory` ends at `20260714155936_AddDeleteSourceResidueAfterIngestSetting`.

## 4. Configuration and feature flags

- `src/Melodee.Common/Constants/SettingRegistry.cs` defines **190** setting keys at the frozen
  commit; these are the authoritative feature-flag/config inventory input for the Phase 0
  configuration inventory (Issue: configuration inventory, captured separately).
- Deployment configuration is environment-variable driven (`example.env`: database pool sizes,
  `MELODEE_PORT`, `MELODEE_AUTH_TOKEN`, Brave Search integration disabled by default).
- A full key-by-key inventory with defaults is a separate Phase 0 output
  (`configuration-inventory.md`); this document only freezes where it comes from.

## 5. Public API surfaces present at the frozen commit

| Surface | Location | Notes |
|---|---|---|
| Melodee API v1 | `src/Melodee.Blazor/Controllers/Melodee/` (32 controllers: Albums, Analytics, ArtistLookup, Artists, AudioFeatures, Auth, Charts, EqualizerPresets, Genres, Party*, Playback*, Playlists, Podcasts, Queue, Recommendations, Requests, Scrobble, Search, Shares, SmartPlaylists, Songs, System, User(s), UserStats) | Native first-party API; route-level capture is Issue 3 |
| OpenSubsonic | `src/Melodee.Blazor/Controllers/OpenSubsonic/` | Matrix doc: `docs/pages/opensubsonic-matrix.md` (Melodee 2.2.0) |
| Jellyfin | `src/Melodee.Blazor/Controllers/Jellyfin/` | Music-focused subset; doc: `docs/pages/api-jellyfin.md` |
| Themes | `src/Melodee.Blazor/Controllers/ThemesController.cs` | Unversioned |

## 6. Recreating the baseline without production data

The baseline must be reproducible by another developer with **no** access to the production
library, database, or secrets. The procedure below was validated on 2026-08-07.

1. **Check out the exact commit.**

   ```bash
   git clone https://github.com/melodee-project/melodee
   cd melodee
   git checkout 4028b58dbd3302ed431e2c5b32d9dde5634db218
   ```

2. **Provide dependencies.** .NET SDK `10.0.104` or later patch (`global.json` rolls forward),
   the `dotnet-ef` 10.0.9 tool (`dotnet tool install --global dotnet-ef --version 10.0.9`,
   matching the legacy Dockerfile), and a disposable PostgreSQL (the melodee-rs dev Postgres on
   port 5433 is used here; the legacy `compose.yml` ships PostgreSQL 17).

3. **Create and migrate a disposable database.** The legacy server does **not** apply EF
   migrations on startup; it crashes if the schema is missing. Apply them explicitly:

   ```bash
   dotnet ef database update \
     --project src/Melodee.Common/Melodee.Common.csproj \
     --startup-project src/Melodee.Blazor/Melodee.Blazor.csproj \
     --context MelodeeDbContext \
     --connection 'Host=127.0.0.1;Port=5433;Database=melodee_phase0;Username=melodee;Password=<dev-password>;SSL Mode=Disable'
   ```

   A baseline database is "the same schema" when `__EFMigrationsHistory` ends at
   `20260714155936_AddDeleteSourceResidueAfterIngestSetting`.

4. **Launch with a scrubbed environment.** Two discoveries make this critical:

   - `MelodeeConfigurationFactory` imports **every** process environment variable into the server
     configuration. Launching from a developer shell absorbs host secrets into the running server.
     Always launch with `env -i` and only the variables below.
   - `dotnet run` applies `Properties/launchSettings.json`, which forces
     `ASPNETCORE_ENVIRONMENT=Development` (loading `appsettings.Development.json` with
     developer-machine paths). Run the built assembly directly instead. The built entry assembly is
     named `server.dll` (not `Melodee.Blazor.dll`), it must run with its output directory as
     content root, and that directory needs `wwwroot` (symlinked from the project) because
     `Program.cs` reads `wwwroot/images/avatar.png` when constructing controllers.

   ```bash
   dotnet build src/Melodee.Blazor/Melodee.Blazor.csproj
   ln -sfn ../../../../../wwwroot src/Melodee.Blazor/bin/Debug/net10.0/wwwroot
   cd src/Melodee.Blazor/bin/Debug/net10.0
   env -i HOME="$HOME" PATH=/usr/local/bin:/usr/bin:/bin \
     ASPNETCORE_ENVIRONMENT=Production ASPNETCORE_URLS=http://127.0.0.1:5157 \
     QuartzDisabled=true \
     ConnectionStrings__DefaultConnection='Host=127.0.0.1;Port=5433;Database=melodee_phase0;Username=melodee;Password=<dev-password>;SSL Mode=Disable' \
     ConnectionStrings__MusicBrainzConnection='Data Source=/tmp/legacy-storage/_search-engines/musicbrainz/musicbrainz.ddb;' \
     ConnectionStrings__ArtistSearchEngineConnection='Data Source=/tmp/legacy-storage/_search-engines/artistSearchEngine.db;Cache=Shared' \
     MelodeeAuthSettings__Token='<random 64+ char fixture token>' \
     Jwt__Key='<random 64+ char fixture token>' \
     Jwt__Issuer=MelodeeApi Jwt__Audience=MelodeeClient \
     dotnet server.dll
   ```

   `QuartzDisabled=true` keeps scheduled jobs quiet during capture; the API surface is unaffected.

5. **Complete onboarding and create a fixture user.** Unauthenticated API requests redirect to the
   login page while onboarding is incomplete. On a disposable database:

   ```sql
   INSERT INTO "Settings" ("ApiKey","Category","CreatedAt","Description","IsLocked","Key","LastUpdatedAt","SortOrder","Value")
   VALUES (gen_random_uuid(), 0, now(), 'Phase 0 capture fixture', false,
           'system.onboardingCompletedAt', now(), 0, '2026-08-07T00:00:00.0000000Z')
   ON CONFLICT ("Key") DO UPDATE SET "Value" = EXCLUDED."Value";
   ```

   Create the fixture account with the legacy CLI (`mcli`), which hashes the password with the
   server's current scheme:

   ```bash
   env -i HOME="$HOME" PATH=/usr/local/bin:/usr/bin:/bin \
     ConnectionStrings__DefaultConnection='...' \
     ConnectionStrings__MusicBrainzConnection='...' \
     ConnectionStrings__ArtistSearchEngineConnection='...' \
     dotnet mcli.dll user create -u phase0-fixture -e phase0-fixture@example.test -p '<fixture-password>' -f
   ```

   Do **not** use the legacy `scripts/create-demo-user.py`: its AES key derivation predates the
   current C# `EncryptionHelper` (it derives key/IV as `SHA256(privateKey+publicKey)`/`MD5(publicKey)`
   while the server uses `SHA256(privateKey)` and the per-user public key as the IV), and its INSERT
   omits the now-required `SortOrder` column. Users it creates cannot authenticate.

6. **Verify the baseline before capturing.**

   - `GET /api/v1/system/info` reports version `2.2.0`;
   - `GET /rest/ping.view?f=json&v=1.16.1&c=check` returns a `subsonic-response` JSON document;
   - `POST /api/v1/auth/authenticate` with the fixture credentials returns HTTP 200 with a token.

7. **Run captures** with this repository's tooling (`scripts/capture_baseline.py`), which writes
   output under `compatibility/captures/` (Git-ignored) and redacts sensitive query parameters
   (`p`, `password`, `t`, `s`, `jwt`, `apikey`, `token`). Use `compare_captures.py` to verify
   reproducibility before promoting anything to `golden/`.

### Dataset counts

Fixture dataset counts are recorded with each capture run. The current disposable database
contains: 1 admin fixture user (`phase0-fixture`), 0 libraries, 0 Artists/Releases/Tracks.
Catalog-bearing captures (Issues 5–8) must seed synthetic fixtures first. The production library
(~35k Artists / ~50k Releases / ~560k Tracks / ~5 TB) is a scale reference only, not a capture
source.

## 7. Secret and privacy review

- This document contains no credentials, tokens, hostnames of private systems, user data, or
  private filesystem paths beyond the developer-local oracle checkout path.
- Production media and the production database were not accessed for this document.
- Legacy demo-reset scripts (`scripts/reset-demo-server.sh` in the legacy repo) document
  sanitization operations for the public demo server and are evidence that fixture-only datasets
  are an established pattern; they are not part of this baseline.

## 8. Open items for later Phase 0 issues

- The published OpenAPI document and route manifests are captured (Issue 3); runtime golden
  scenarios for the three surfaces are Issues 5–7.
- The disposable capture environment is live and validated per section 6; public and
  authenticated scenarios compare reproducibly (Issue 4).
- Per-capture fixture dataset counts are recorded with each capture run, not here.
- `configuration-inventory.md` (190 SettingRegistry keys with defaults) is a separate deliverable.
