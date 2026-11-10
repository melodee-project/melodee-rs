#!/usr/bin/env python3
"""Regenerate `apps/melodee-server/src/settings_inventory.rs`.

The settings inventory answers "does this registered setting actually do
anything?" for operators and troubleshooting agents. Keeping it derived from
the code (rather than hand-maintained prose) prevents silent drift.

Classification for each key in the persistence registry:

1. ``consumer`` — a Rust module in this repository references the key literal,
   or one of the documented dynamic families below owns it. (Dynamic families
   are keys resolved through a constructed name, e.g. provider timeouts or the
   FNV env-only plumbing keys.)
2. ``inert`` — a curated, reviewed list of keys with no runtime behavior and
   the reason (retired capability, frozen compatibility constant, replaced by
   configuration, C# also ignores it).
3. neither — unverified: the script cannot prove a consumer and there is no
   recorded retirement decision. The admin diagnostics selftest reports these.

The curated inert list below is the reviewed part; edit it deliberately.
A test (`every_registered_key_is_listed_exactly_once`) keeps the generated
table in lock-step with the registry.
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
SETTINGS_RS = REPO / "crates/melodee-persistence/src/settings.rs"
OUTPUT = REPO / "apps/melodee-server/src/settings_inventory.rs"

#: Keys whose consumer resolves the name dynamically (constructed key strings).
DYNAMIC_CONSUMERS = {
    "searchEngine.": "providers",
    "email.": "notifications",
    "mpd.": "playback",
    "mpv.": "playback",
    "podcast.http.": "podcast_worker",
    "podcast.refresh.": "podcast_worker",
    "podcast.download.": "podcast_worker",
    "podcast.retention.": "podcast_worker",
    "jellyfin.token.": "jellyfin",
    "transcoding.": "transcode",
}

#: Reviewed inert keys. Every entry needs a concrete reason.
INERT = {
    "partyMode.enabled": "Party mode is retired (ADR-0133/ADR-0141); the setting is retained for legacy import only.",
    "userDeviceProfile.enabled": "Per-user device profiles are a reference-only disposition (ADR-0134 section 5, ADR-0148); global admin metadata replaces it.",
    "imaging.thumbnailSize": "Thumbnail sizing is derived from the publication artwork variants, not a runtime setting.",
    "imaging.duplicateThreshold": "Duplicate detection is deterministic policy, not a tunable threshold.",
    "system.defaultTheme": "Theme selection is per-user persisted state in the Rust Admin UI; there is no server-wide default theme.",
    "theme.libraryPath": "Legacy Theme-library filesystem packs are imported once (MEL-7); packs are database-owned afterwards.",
    "theme.maxUploadSizeMb": "Theme pack uploads are bounded by the fixed package limits of the theme importer.",
    "theme.enforceContrastValidation": "Contrast validation is enforced unconditionally by the Admin UI theme pipeline.",
    "lyrics.filesEnabled": "Sidecar lyrics are served when present; the legacy toggle has no captured client behavior.",
    "defaults.pagesize": "Native-v1 pagination defaults are frozen constants (ApiDefaults.DefaultPageSize = 50).",
    "searchResults.defaultPageSize": "Native-v1 search paging is a frozen constant contract.",
    "playlist.maximumAllowedPageSize": "Native-v1 playlist paging is a frozen constant contract.",
    "userinterface.toastAutoCloseTime": "Admin UI toast timing is a client-side UI constant.",
    "jobs.chartUpdate.cronExpression": "Scheduling is expressed by the Rust scheduler intervals, not legacy Quartz cron strings.",
    "jobs.libraryInsert.cronExpression": "Scheduling is expressed by the Rust scheduler intervals, not legacy Quartz cron strings.",
    "jobs.libraryProcess.cronExpression": "Scheduling is expressed by the Rust scheduler intervals, not legacy Quartz cron strings.",
    "jobs.podcastRefresh.cronExpression": "Podcast refresh cadence is per-channel (refresh_interval_hours) plus the worker poll interval.",
    "jobs.podcastDownload.cronExpression": "Podcast downloads are lease-claimed continuously by the worker.",
    "jobs.podcastCleanup.cronExpression": "Podcast retention runs after each worker pass (no legacy Quartz cron).",
    "jobs.podcastRecovery.cronExpression": "Podcast orphan/stuck recovery runs after each worker pass.",
    "jobs.nowPlayingCleanup.cronExpression": "Now-playing cleanup runs inside the discovery worker pass.",
    "jobs.stagingAutoMove.cronExpression": "Staging promotion is event-driven by the ingestion pipeline.",
    "jobs.stagingAlbumRevalidation.cronExpression": "Staging revalidation is event-driven by the ingestion pipeline.",
    "jobs.artistHousekeeping.cronExpression": "Artist housekeeping is part of the policy worker pass.",
    "jobs.artistSearchEngineHousekeeping.cronExpression": "Provider cache housekeeping is part of the enrichment worker pass.",
    "jobs.musicbrainzUpdateDatabase.cronExpression": "MusicBrainz imports are operator/CLI-triggered, not scheduled.",
    "plugin.blackbeard.enabled": "Legacy scanner plugins are replaced by the ingestion pipeline (parity matrix).",
    "plugin.cueSheet.enabled": "Legacy scanner plugins are replaced by the ingestion pipeline (parity matrix).",
    "plugin.m3u.enabled": "Playlist import is an explicit user/API action, not a scanner plugin.",
    "plugin.nfo.enabled": "Legacy scanner plugins are replaced by the ingestion pipeline (parity matrix).",
    "plugin.simpleFileVerification.enabled": "Structural validation is always-on in the ingestion pipeline.",
    "streaming.useBufferedResponses": "Direct-play streams are always streamed from disk with bounded admission; there is no buffered mode.",
    "email.resetPassword.htmlBodyTemplate": "Password-reset bodies are operator-authored notification templates in notification_templates (MEL-6 import), not settings.",
    "email.resetPassword.textBodyTemplate": "Password-reset bodies are operator-authored notification templates in notification_templates (MEL-6 import), not settings.",
    "openSubsonicServer.openSubsonicServer.type": "OpenSubsonic server identity is a frozen compatibility constant.",
    "openSubsonicServer.openSubsonicServerLicenseEmail": "OpenSubsonic license metadata is a frozen compatibility constant.",
    "openSubsonicServer.openSubsonicServer.index.artistLimit": "Index artist paging is a frozen compatibility constant.",
    "openSubsonicServer.openSubsonic.serverSupportedVersion": "OpenSubsonic supported-version is a frozen compatibility constant.",
    "filtering.lessThanDuration": "Legacy staging filter heuristics are replaced by explicit policy decisions.",
    "filtering.lessThanSongCount": "Legacy staging filter heuristics are replaced by explicit policy decisions.",
    "processing.dontDeleteExisitingMelodeeDataFiles": "The Rust pipeline never deletes existing Melodee data files; behavior is unconditional.",
    "artist.biographyPlaceHolderText": "Biography placeholders are Admin UI presentation strings.",
    "formatting.dateTimeDisplayFormatShort": "Admin UI date formatting is a client concern.",
    "formatting.dateTimeDisplayActivityFormat": "Admin UI date formatting is a client concern.",
    "jellyfin.rateLimit.apiRequestsPerPeriod": "The current C# build registers this key without enforcing it (no captured client behavior).",
    "jellyfin.rateLimit.apiPeriodSeconds": "The current C# build registers this key without enforcing it (no captured client behavior).",
    "jellyfin.rateLimit.streamConcurrentPerUser": "Jellyfin per-user stream limits are enforced by streaming.maxConcurrentStreams.perUser.",
    'jellyfin.routePrefix': 'Jellyfin routes are mounted at the captured paths; the current C# build registers this key without using it.',
    'scrobbling.enabled': 'The current C# build does not consume this key (per-user scrobble capability governs behavior); per-user links and dispatch are settings-driven separately.',
    'system.onboardingCompletedAt': 'Onboarding completion is the durable onboarding_state marker plus migration evidence, not a copied timestamp setting.',
    'system.isDownloadingEnabled': 'Consumed by the OpenSubsonic download gate.',
    'encryption.privateKey': 'Legacy C# settings-encryption key. The migration reads legacy values under the documented contract and never persists this key; Rust secrets use the dedicated secret settings.',
    'playlist.dynamicPlaylist.disabled': 'Smart playlists are a supported capability; the legacy kill switch has no captured client behavior.',
    'podcast.recovery.stuckDownloadThresholdMinutes': 'Stuck-download recovery is lease-expiry based (the lease duration is the threshold), surfaced by `melodee-cli podcasts stuck/recover`.',
    'podcast.recovery.orphanedUsageThresholdHours': 'Orphaned owned files are reconciled after every worker pass, not by an age threshold.',
    'scripting.preDiscoveryScript': 'Legacy script bodies survive import as settings values but are not executed; re-register through the script registry (documented migration path, PRD-SCRIPT-002).',
    'scripting.postDiscoveryScript': 'Legacy script bodies survive import as settings values but are not executed; re-register through the script registry (documented migration path, PRD-SCRIPT-002).',
    'imaging.largeSize': 'Compatibility image derivative dimensions are frozen constants derived from captured image contracts (ADR-0143); they are not operator-tunable.',
 'imaging.mediumSize': 'Compatibility image derivative dimensions are frozen constants derived from captured image contracts (ADR-0143); they are not operator-tunable.',
 'imaging.smallSize': 'Compatibility image derivative dimensions are frozen constants derived from captured image contracts (ADR-0143); they are not operator-tunable.',
 'imaging.maximumNumberOfAlbumImages': 'Artwork selection is bounded deterministically by the publication/override pipeline, not by this legacy count.',
 'imaging.maximumNumberOfArtistImages': 'Artwork selection is bounded deterministically by the publication/override pipeline, not by this legacy count.',
 'imaging.minimumImageSize': 'Artwork validation uses decoded-image bounds in the bounded image pipeline, not this legacy pixel threshold.',
 'defaults.cacheDurationInMinutes': 'Caching is provider/catalog cache policy with explicit freshness settings, not a global default duration.',
    'defaults.dashboard.latestPageSize': 'Admin dashboard window sizes are server-side bounded constants.',
    'imaging.doLoadEmbeddedImages': 'Embedded artwork extraction is always attempted within the bounded artwork pipeline.',
    # family conversion. handled in classify()
    # family magic. handled in classify()
    # family processing. handled in classify()
    # family validation. handled in classify()
}


#: Prefix families whose legacy knobs are replaced by the redesigned pipeline.
INERT_FAMILIES = {
    "conversion.": "Legacy MediaConvertor pipeline: Rust normalization is policy-defined and bounded (PRD-MEDIA-002); these knobs do not apply.",
    "magic.": "Legacy metadata \"magic\" fixups are replaced by deterministic catalog/policy rules; operators correct metadata through the admin metadata surface.",
    "processing.": "Legacy directory-processing options are replaced by the deterministic ingestion pipeline and its policy decisions.",
    "validation.": "Legacy staging thresholds are replaced by structural validation and policy decisions (PRD-INGEST).",
}


def env_var_name(key: str) -> str:
    """Mirror of `melodee_persistence::env_var_name` for literal scanning."""
    return "MELODEE__" + key.replace(".", "__").replace("-", "_").upper()


def registered_keys() -> list[str]:
    source = SETTINGS_RS.read_text()
    return re.findall(r'KnownSetting \{ key: "([^"]+)"', source)


def rust_sources() -> dict[str, str]:
    files = subprocess.run(
        ["git", "ls-files", "apps", "crates"],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.splitlines()
    sources: dict[str, str] = {}
    for name in files:
        if (
            name.endswith(".rs")
            and "/tests/" not in name
            and not name.endswith("melodee-persistence/src/settings.rs")
            and not name.endswith("settings_inventory.rs")
        ):
            path = REPO / name
            try:
                sources[name] = path.read_text(errors="ignore")
            except OSError:
                continue
    return sources


def module_for(name: str) -> str:
    trimmed = (
        name.replace("apps/melodee-server/src/", "")
        .replace("apps/melodee-cli/src/", "cli:")
        .replace("crates/", "")
    )
    without_suffix = trimmed.rsplit(".rs", 1)[0]
    return without_suffix.replace("/src/", ":").replace("/mod", "")


def classify(key: str, sources: dict[str, str], settings_source: str) -> tuple[str, str]:
    if key in INERT:
        return "", INERT[key]
    for name, text in sources.items():
        if name.endswith("settings_inventory.rs"):
            continue
        if key in text or env_var_name(key) in text:
            return module_for(name), ""
    for prefix, consumer in DYNAMIC_CONSUMERS.items():
        if key.startswith(prefix):
            return consumer, ""
    for prefix, reason in INERT_FAMILIES.items():
        if key.startswith(prefix):
            return "", reason
    return "", ""


def render(entries: list[tuple[str, str, str]]) -> str:
    lines = [
        "//! Settings inventory: which runtime component consumes each registered",
        "//! setting, or why it is deliberately inert.",
        "//!",
        "//! GENERATED by `scripts/generate_settings_inventory.py`; edit the script's",
        "//! curated inert list rather than this file. This is the machine-readable",
        "//! answer to \"does this setting actually do anything?\", which operators and",
        "//! diagnostic agents need when a configured value appears to have no effect.",
        "//! A row with an empty `consumer` and an empty `inert_reason` is *unverified*:",
        "//! the generator could not prove a consumer and no retirement decision is",
        "//! recorded. The admin diagnostics selftest reports unverified keys.",
        "//!",
        "//! `tests::every_registered_key_is_listed_exactly_once` keeps this table in",
        "//! lock-step with the registry so a new setting needs an explicit decision.",
        "",
        "/// One registered setting and its runtime consumer (or inert rationale).",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub struct SettingInventoryEntry {",
        "    pub key: &'static str,",
        "    /// Rust module that reads this key, when known.",
        "    pub consumer: &'static str,",
        "    /// Why the key has no runtime consumer (retired/accepted difference).",
        "    pub inert_reason: &'static str,",
        "}",
        "",
        "pub const SETTING_INVENTORY: &[SettingInventoryEntry] = &[",
    ]
    for key, consumer, inert_reason in sorted(entries):
        lines.extend(
            [
                "    SettingInventoryEntry {",
                f"        key: {json.dumps(key)},",
                f"        consumer: {json.dumps(consumer)},",
                f"        inert_reason: {json.dumps(inert_reason)},",
                "    },",
            ]
        )
    lines.extend(
        [
            "];",
            "",
            "/// Inventory lookup by key.",
            "#[must_use]",
            "pub fn setting_inventory(key: &str) -> Option<&'static SettingInventoryEntry> {",
            "    SETTING_INVENTORY.iter().find(|entry| entry.key == key)",
            "}",
            "",
            "#[cfg(test)]",
            "mod tests {",
            "    #![allow(clippy::expect_used)]",
            "",
            "    use super::*;",
            "    use melodee_persistence::{KNOWN_SETTINGS, known_setting};",
            "",
            "    #[test]",
            "    fn every_registered_key_is_listed_exactly_once() {",
            "        for known in KNOWN_SETTINGS {",
            "            let matches = SETTING_INVENTORY",
            "                .iter()",
            "                .filter(|entry| entry.key == known.key)",
            "                .count();",
            "            assert_eq!(matches, 1, \"setting {} must be inventoried once\", known.key);",
            "        }",
            "        assert_eq!(SETTING_INVENTORY.len(), KNOWN_SETTINGS.len());",
            "    }",
            "",
            "    #[test]",
            "    fn inventory_keys_are_registered_and_unique() {",
            "        let mut seen = std::collections::BTreeSet::new();",
            "        for entry in SETTING_INVENTORY {",
            "            assert!(known_setting(entry.key).is_some(), \"unknown key {}\", entry.key);",
            "            assert!(seen.insert(entry.key), \"duplicate key {}\", entry.key);",
            "            assert!(",
            "                entry.consumer.is_empty() || entry.inert_reason.is_empty(),",
            "                \"{} cannot be both consumed and inert\",",
            "                entry.key",
            "            );",
            "        }",
            "    }",
            "}",
        ]
    )
    return "\n".join(lines) + "\n"


def main() -> int:
    sources = rust_sources()
    settings_source = SETTINGS_RS.read_text()
    keys = registered_keys()
    entries = [(key, *classify(key, sources, settings_source)) for key in keys]
    OUTPUT.write_text(render(entries))
    honored = sum(1 for _, consumer, _ in entries if consumer)
    inert = sum(1 for _, consumer, reason in entries if not consumer and reason)
    print(
        f"wrote {OUTPUT.relative_to(REPO)}: {len(entries)} keys "
        f"({honored} consumed, {inert} inert, {len(entries) - honored - inert} unverified)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
