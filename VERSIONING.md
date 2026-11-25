# Melodee versioning

Melodee uses one `MAJOR.MINOR.PATCH` version for the Rust server, administrative CLI, and bundled Admin UI. Choose the version once per release according to the largest change a supported client or operator will experience since the previous release. The project follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).

## Choosing the next version

The Rust workspace starts at `0.1.0`. Read `[workspace.package].version` for the version in any checkout. During `0.x`, Melodee reserves patch releases for compatible fixes. New features, deprecations, and approved breaking changes advance the minor version. Call out each breaking change and its migration steps in the release notes, even during `0.x`. [Workspace manifest](Cargo.toml)

| Change shipped | During development, from `0.1.0` | After 1.0, from `1.4.2` |
| --- | --- | --- |
| Compatible fix, such as correcting a range-streaming bug to restore the documented response | `0.1.1` | `1.4.3` |
| Compatible feature, such as a new CLI subcommand, or deprecation of a supported option that still works | `0.2.0` | `1.5.0` |
| Incompatible change, such as removing a documented CLI flag or requiring an existing deployment to replace a configuration key | `0.2.0`, with a breaking-change notice | `2.0.0`, with a migration guide |

Use the same rules for security fixes and dependency updates. A compatible security fix qualifies for a patch. A change that drops support for a documented PostgreSQL version is breaking. Documentation, tests, and internal refactoring can accumulate under **Unreleased** until there is a product release to ship.

`1.0.0` marks the first stable product contract. The project owner qualifies the exact candidate against the release gates, including data preservation, real-client compatibility, upgrade and recovery, and evidence for both supported image architectures. [Release gates](PRD.md#57-release-gates) · [Readiness overview](README.md#compatibility-and-v100-readiness)

## What compatibility covers

- **Client protocols.** Native Melodee API v1, OpenSubsonic, and the supported Jellyfin music routes each have their own contracts. These include authentication, public IDs, payloads, status codes, and streaming behavior. Changing an existing external contract at any product version requires a compatibility test, a product decision, an accepted ADR, and a client migration plan. The Admin API has its own version under `/api/admin/v1`. [API contracts](SPEC.md#19-external-api-surfaces) · [Compatibility decision](design/adr/ADR-0102-preserve-public-apis-as-separate-adapters.md)
- **Operator interfaces.** Count documented CLI commands and flags, machine-readable output, configuration keys, and supported deployment and upgrade paths. Deprecate an interface while it still works, name its replacement, and document the release that removes it.
- **Persistent data.** PostgreSQL migration numbers, canonical media manifest versions, and portable export schema versions describe their respective formats. They advance when those formats require it. An additive migration that preserves existing data and a supported upgrade path takes the bump appropriate to the feature or fix. Losing support for a documented import format is breaking. Describe backup, migration, and restore requirements for each affected release. [Upgrade guide](docs/src/content/docs/installation/upgrading.md)

## Prereleases, tags, and installed versions

Use a numbered suffix for a prerelease, such as `1.0.0-beta.1` or `1.0.0-rc.2`. A changed candidate gets the next number. Use a leading `v` on Git release tags and the bare version in Cargo and GHCR. Official release tags omit SemVer’s optional `+build` metadata because the release validator accepts only the version core and an optional prerelease suffix.

| Example GitHub Release tag | GitHub prerelease setting | Resulting tags on `ghcr.io/melodee-project/melodee` |
| --- | --- | --- |
| `v1.4.2` | Off | `1.4.2`, `1.4`, `latest` |
| `v1.5.0-rc.1` | On | `1.5.0-rc.1` only |

Exact published versions are immutable. Use a new version for changed artifacts. Pin an exact image tag or manifest digest for a repeatable deployment. For a source-built installation, record `git rev-parse HEAD` from the clean build checkout alongside the version. Several commits on `main` can share the same Cargo version. Check the running server version in the expanded Admin UI navigation footer or in the `version` field of `GET /healthz`. Check the CLI with `melodee-cli --version`. [Container tags and provenance](docs/src/content/docs/installation/docker.md)

## Preparing a release

1. Review the **Unreleased** entries and the changes since the previous release. Choose the bump, give the release a dated section in the changelog, and spell out deprecations, breaking changes, and any operator action. [Changelog](CHANGELOG.md)
2. Set `[workspace.package].version` in the root Cargo manifest. The Rust workspace members inherit it. Update the version requirements on internal `melodee-*` path dependencies to match the release, including any prerelease suffix. Refresh and commit `Cargo.lock` and verify dependency resolution with `cargo metadata --locked --format-version 1`. The private Admin UI and docs npm package versions are tooling metadata. [Workspace manifest](Cargo.toml)
3. Run `python3 scripts/do-prechecks.py` on the release commit and complete the applicable qualification gates. Publish the GitHub Release for that commit on `main` with its matching tag and prerelease setting. Publishing the GitHub Release triggers the official image workflow. The workflow checks both Cargo application versions, qualifies the Linux amd64 and arm64 images, and publishes the scanned artifacts to GHCR. [Maintainer release flow](CONTRIBUTING.md#maintainer-release-flow) · [Release workflow](.github/workflows/release-container.yml)
4. Verify the published manifest digest, image version and revision labels, and provenance attestation. Keep the GitHub Release as the version and release-note record. [Official distribution policy](design/adr/ADR-0144-publish-official-releases-as-ghcr-oci-images.md)
