# Phase 9 WP-08 — Legacy configuration inventory (the never-produced Phase 0 deliverable)

Generated 2026-08-14. Legacy oracle: `/home/steven/src/github/melodee` at frozen
commit `4028b58dbd3302ed431e2c5b32d9dde5634db218`. Ground-up rewrite rule applies:
the C# artifacts below are behavioral evidence, not an architecture template.
Governing authorities for the Rust implementation: PRD-CONFIG-001/003,
PRD-SETUP-002, PRD-AUTH-007, and ADR-0134 §3 (`server_settings` + `onboarding_state`).

## Source basis and method

Read-only extraction from three legacy sources, cross-checked against each other:

- `src/Melodee.Common/Constants/SettingRegistry.cs` — the authoritative key
  enumeration: 190 `public const string` declarations. This file contains keys
  only; no defaults, descriptions, or categories live here.
- `src/Melodee.Common/Data/MelodeeDbContext.cs` (lines 232–1819) — the EF Core
  `HasData` seed for the `Settings` table: 157 rows carrying `Value` (default),
  `Comment`, `Description`, and `Category`. Mirrored verbatim (as literal key
  strings) in `Migrations/20260524160636_InitialBaseline.cs` and
  `MelodeeDbContextModelSnapshot.cs`. The only later migration,
  `20260714155936_AddDeleteSourceResidueAfterIngestSetting.cs`, inserts exactly
  one key (`processing.deleteSourceResidueAfterIngest`) that is also in the
  DbContext seed — migrations add no keys beyond the seed.
- Code-level fallback defaults at consumption sites (`??` / `GetValue<T>` lambda
  fallbacks), plus `src/Melodee.Common/Constants/SettingDefaults.cs` and
  `src/Melodee.Common/Configuration/MelodeeConfiguration.cs` constants, covering
  the 33 registry keys that are never DB-seeded.

**Authoritative count: 190 registry constants = 190 unique key strings (no
duplicate constants, no duplicate key strings — verified by exact extraction).
Of these, 157 are DB-seeded and 33 exist only as constants with code-level (or
no) defaults.** The Phase 0 "190-key" claim is exactly accurate.

Categories come from `src/Melodee.Common/Enums/SettingCategory.cs`:
`General = 0` (the effective category of every seed row that leaves `Category`
unset), `Api`, `Conversion`, `Formatting`, `Imaging`, `Magic`, `MediaConvertor`,
`PluginProcess`, `Scripting`, `SearchEngine`, `Scrobbling`, `System`,
`Transcoding`, `Validation`, `Jobs`, `Podcast`, `Jukebox`.

Grouping below follows the structure of `SettingRegistry.cs` itself: the file has
no `#region`s, only comment-block headers (`// Jukebox settings`,
`// MPV Backend settings`, `// MPD Backend settings`, `// Podcast settings`,
`// Streaming settings`, `// User Device Profile settings`, `// Email settings`,
`// Security settings`, `// Jellyfin API settings`, `// Theme settings`) around
otherwise alphabetical runs.

In the tables: the **Default** column gives the seeded value; `code: X` means the
key is not seeded and X is the fallback at the consumption site; "not seeded"
with no `code:` value means no default exists anywhere. **Category** is the
seeded `SettingCategory`; "General (unset)" means the seed row left it null.
**[SECRET]** marks secret-sensitive keys (see the dedicated section).

## 1. Artist / conversion / defaults / encryption / filtering / formatting / imaging

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `artist.biographyPlaceHolderText` | `ArtistBiographyPlaceHolderText` | not seeded | — | Placeholder text when an artist has no biography (Blazor ArtistDetail) | |
| `conversion.bitrate` | `ConversionBitrate` | `384` | Conversion | Bitrate to convert non-mp3 media files during processing | |
| `conversion.enabled` | `ConversionEnabled` | `true` | Conversion | Enable conversion of non-mp3 media files during processing | |
| `conversion.samplingRate` | `ConversionSamplingRate` | `48000` | Conversion | Sampling rate for conversion | |
| `conversion.vbrLevel` | `ConversionVbrLevel` | `4` | Conversion | VBR level for conversion | |
| `defaults.batchSize` | `DefaultsBatchSize` | `250` | General (unset) | Processing batch size; allowed range 250–1000 (`BatchSizeDefault`/`BatchSizeMaximum`) | |
| `defaults.pagesize` | `DefaultsPageSize` | `100` | General (unset) | Default page size for paged views. Lowercase "pagesize" is verbatim | |
| `defaults.cacheDurationInMinutes` | `DefaultCacheDurationInMinutes` | code: `240` minutes (`< 0` disables) | — | Configuration cache duration | |
| `defaults.dashboard.latestPageSize` | `DefaultsDashboardLatestPageSize` | code: falls back to `defaults.pagesize`, then `50` | — | Dashboard "latest" list page size | |
| `encryption.privateKey` | `EncryptionPrivateKey` | `H+Kiik6VMKfTD2MesF1GoMjczTrD5RhuKckJ5+/UQWOdWajGcsEC3yEnlJ5eoy8Y` (hardcoded in seed) | General (unset) | AES key to encrypt/decrypt passwords for Subsonic authentication | [SECRET] |
| `filtering.lessThanDuration` | `FilteringLessThanDuration` | `720000` | General (unset) | Default filter: albums with this or less duration (ms) | |
| `filtering.lessThanSongCount` | `FilteringLessThanSongCount` | `3` | General (unset) | Default filter: albums with this or fewer songs | |
| `formatting.dateTimeDisplayActivityFormat` | `FormattingDateTimeDisplayActivityFormat` | `hh\:mm\:ss\.ffff` | Formatting | Format for activity-related dates (processing messages) | |
| `formatting.dateTimeDisplayFormatShort` | `FormattingDateTimeDisplayFormatShort` | `yyyyMMdd HH\:mm` | Formatting | Short format for full dates | |
| `imaging.doLoadEmbeddedImages` | `ImagingDoLoadEmbeddedImages` | `true` | Imaging | Include embedded media images in the Melodee data file | |
| `imaging.maximumNumberOfAlbumImages` | `ImagingMaximumNumberOfAlbumImages` | `25` | Imaging | Max images per album (all types), 0 = unlimited | |
| `imaging.maximumNumberOfArtistImages` | `ImagingMaximumNumberOfArtistImages` | `25` | Imaging | Max images per artist, 0 = unlimited | |
| `imaging.minimumImageSize` | `ImagingMinimumImageSize` | `300` | Imaging | Images under this size are invalid, 0 disables | |
| `imaging.smallSize` | `ImagingSmallSize` | `300` | Imaging | Small square image size | |
| `imaging.mediumSize` | `ImagingMediumSize` | `600` | Imaging | Medium square image size | |
| `imaging.largeSize` | `ImagingLargeSize` | `1600` | Imaging | Large image size / resize ceiling, blank disables | |
| `imaging.thumbnailSize` | `ImagingThumbnailSize` | code: `50` (`ImageSize.Thumbnail`) | — | Thumbnail square image size | |
| `imaging.duplicateThreshold` | `ImagingDuplicateThreshold` | code: `80` (`MelodeeConfiguration.DefaultImagingDuplicateThreshold`) | — | Perceptual-hash duplicate threshold | |

## 2. Jobs (cron schedules)

All seeded job rows are Category `Jobs`; empty value disables the job.

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `jobs.artistHousekeeping.cronExpression` | `JobsArtistHousekeepingCronExpression` | `0 0 0/1 1/1 * ? *` (hourly) | Jobs | Artist housekeeping job schedule (Quartz cron) | |
| `jobs.artistSearchEngineHousekeeping.cronExpression` | `JobsArtistSearchEngineHousekeepingCronExpression` | `0 0 0 * * ?` (daily 00:00) | Jobs | Artist search-engine housekeeping schedule | |
| `jobs.chartUpdate.cronExpression` | `JobsChartUpdateCronExpression` | `0 0 2 * * ?` (daily 02:00) | Jobs | Chart update job schedule | |
| `jobs.libraryProcess.cronExpression` | `JobsLibraryProcessCronExpression` | `0 */10 * ? * *` (every 10 min) | Jobs | Library process job schedule | |
| `jobs.libraryInsert.cronExpression` | `JobsLibraryInsertCronExpression` | `0 0 0 * * ?` (daily 00:00) | Jobs | Library scan/insert job schedule | |
| `jobs.nowPlayingCleanup.cronExpression` | `JobsNowPlayingCleanupCronExpression` | not seeded (scheduled only if configured; `ScheduleJobIfConfigured`) | — | Stale now-playing record cleanup schedule | |
| `jobs.stagingAutoMove.cronExpression` | `JobsStagingAutoMoveCronExpression` | `0 */15 * * * ?` (every 15 min) | Jobs | Staging auto-move schedule; also triggered after inbound processing | |
| `jobs.stagingAlbumRevalidation.cronExpression` | `JobsStagingAlbumRevalidationCronExpression` | not seeded (scheduled only if configured) | — | Re-validates staging albums with invalid artists | |
| `jobs.musicbrainzUpdateDatabase.cronExpression` | `JobsMusicBrainzUpdateDatabaseCronExpression` | `0 0 12 1 * ?` (1st of month, 12:00) | Jobs | MusicBrainz database housekeeping schedule | |
| `jobs.podcastRefresh.cronExpression` | `JobsPodcastRefreshCronExpression` | `0 */15 * ? * *` (every 15 min) | Jobs | Podcast refresh job schedule | |
| `jobs.podcastDownload.cronExpression` | `JobsPodcastDownloadCronExpression` | `0 */5 * ? * *` (every 5 min) | Jobs | Podcast download job schedule | |
| `jobs.podcastCleanup.cronExpression` | `JobsPodcastCleanupCronExpression` | `0 0 2 * * ?` (daily 02:00) | Jobs | Podcast cleanup job schedule | |
| `jobs.podcastRecovery.cronExpression` | `JobsPodcastRecoveryCronExpression` | `0 */30 * ? * *` (every 30 min) | Jobs | Podcast recovery job schedule | |

## 3. Lyrics / magic / OpenSubsonic / party mode

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `lyrics.filesEnabled` | `LyricFilesEnabled` | code: `false` | — | Enable lyric file processing plugin | |
| `magic.doRemoveFeaturingArtistFromSongArtist` | `MagicDoRemoveFeaturingArtistFromSongArtist` | `true` | Magic | Remove featured artists from song artist | |
| `magic.doRemoveFeaturingArtistFromSongTitle` | `MagicDoRemoveFeaturingArtistFromSongTitle` | `true` | Magic | Remove featured artists from song title | |
| `magic.doRemoveUnwantedTextFromAlbumTitle` | `MagicDoRemoveUnwantedTextFromAlbumTitle` | `true` | Magic | Remove unwanted text from album title | |
| `magic.doRemoveUnwantedTextFromSongTitles` | `MagicDoRemoveUnwantedTextFromSongTitles` | `true` | Magic | Remove unwanted text from song titles | |
| `magic.doRenumberSongs` | `MagicDoRenumberSongs` | `true` | Magic | Renumber songs during magic processing | |
| `magic.doReplaceSongsArtistSeparators` | `MagicDoReplaceSongsArtistSeparators` | `true` | Magic | Replace song artist separators with `/` | |
| `magic.doSetYearToCurrentIfInvalid` | `MagicDoSetYearToCurrentIfInvalid` | `false` | Magic | Set song year to current year if invalid/missing | |
| `magic.enabled` | `MagicEnabled` | `true` | Magic | Master magic-processing switch | |
| `openSubsonicServer.openSubsonicServer.index.artistLimit` | `OpenSubsonicIndexesArtistLimit` | `1000` | Api | Artist cap in getIndexes; 0 = 32k per index | |
| `openSubsonicServer.openSubsonicServerLicenseEmail` | `OpenSubsonicServerLicenseEmail` | `noreply@localhost.lan` | Api | Email used in License responses | |
| `openSubsonicServer.openSubsonic.serverSupportedVersion` | `OpenSubsonicServerSupportedVersion` | `1.16.1` | Api | Advertised Subsonic API version | |
| `openSubsonicServer.openSubsonicServer.type` | `OpenSubsonicServerType` | `Melodee` | Api | Server name in API responses | |
| `partyMode.enabled` | `PartyModeEnabled` | code: `false` | — | Party mode switch (party/jukebox waived per ADR-0133) | |

## 4. Jukebox — `// Jukebox settings` (waived per ADR-0133; inventoried for completeness)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `jukebox.enabled` | `JukeboxEnabled` | `false` | Jukebox | Enable server-side jukebox playback | |
| `jukebox.backendType` | `JukeboxBackendType` | `""` (`mpv`/`mpd`) | Jukebox | Backend type; empty = no backend | |

## 5. `// MPV Backend settings` (Category: Jukebox)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `mpv.path` | `MpvPath` | `""` | Jukebox | MPV executable path; empty = system PATH | |
| `mpv.audioDevice` | `MpvAudioDevice` | `""` | Jukebox | Audio device; empty = default | |
| `mpv.extraArgs` | `MpvExtraArgs` | `""` | Jukebox | Extra CLI arguments for MPV | |
| `mpv.socketPath` | `MpvSocketPath` | `""` | Jukebox | IPC socket path; empty = temp dir | |
| `mpv.initialVolume` | `MpvInitialVolume` | `0.8` | Jukebox | Initial volume 0.0–1.0 | |
| `mpv.enableDebugOutput` | `MpvEnableDebugOutput` | `false` | Jukebox | Verbose MPV debug output | |

## 6. `// MPD Backend settings` (Category: Jukebox)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `mpd.instanceName` | `MpdInstanceName` | `""` | Jukebox | Multi-instance identifier | |
| `mpd.host` | `MpdHost` | `localhost` | Jukebox | MPD server hostname/IP | |
| `mpd.port` | `MpdPort` | `6600` | Jukebox | MPD port | |
| `mpd.password` | `MpdPassword` | `""` | Jukebox | MPD auth password | [SECRET] |
| `mpd.timeoutMs` | `MpdTimeoutMs` | `10000` | Jukebox | TCP/operation timeout (ms) | |
| `mpd.initialVolume` | `MpdInitialVolume` | `0.8` | Jukebox | Initial volume 0.0–1.0 | |
| `mpd.enableDebugOutput` | `MpdEnableDebugOutput` | `false` | Jukebox | MPD command debug logging | |

## 7. Playlist / processing plugins

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `playlist.dynamicPlaylist.disabled` | `PlaylistDynamicPlaylistsDisabled` | code: `false` | — | Disable dynamic playlists | |
| `playlist.maximumAllowedPageSize` | `PlaylistMaximumAllowedPageSize` | code: `1000` | — | Max page size for playlist endpoints | |
| `plugin.blackbeard.enabled` | `PluginEnabledBlackbeard` | code: `true` (`?? true`) | — | Blackbeard metadata plugin | |
| `plugin.cueSheet.enabled` | `PluginEnabledCueSheet` | `true` | PluginProcess | Process CueSheet files | |
| `plugin.m3u.enabled` | `PluginEnabledM3u` | `true` | PluginProcess | Process M3U files | |
| `plugin.nfo.enabled` | `PluginEnabledNfo` | `true` | PluginProcess | Process NFO files | |
| `plugin.simpleFileVerification.enabled` | `PluginEnabledSimpleFileVerification` | `true` | PluginProcess | Process SFV files | |
| `processing.albumTitleRemovals` | `ProcessingAlbumTitleRemovals` | `['^', '~', '#']` | General (unset) | JSON array of fragments removed from album titles | |

## 8. `// Podcast settings` (Category: Podcast unless noted; cron keys are in §2)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `podcast.enabled` | `PodcastEnabled` | `true` | Podcast | Enable podcast support | |
| `podcast.http.allowHttp` | `PodcastHttpAllowHttp` | `false` | Podcast | Allow non-HTTPS feed URLs | |
| `podcast.http.timeoutSeconds` | `PodcastHttpTimeoutSeconds` | `30` | Podcast | HTTP timeout for feed requests | |
| `podcast.http.maxRedirects` | `PodcastHttpMaxRedirects` | `10` | Podcast | Max feed redirects (also `SettingDefaults.PodcastHttpMaxRedirects = 10`) | |
| `podcast.http.maxFeedBytes` | `PodcastHttpMaxFeedBytes` | `10485760` | Podcast | Max feed response size (10 MiB) | |
| `podcast.refresh.maxItemsPerChannel` | `PodcastRefreshMaxItemsPerChannel` | `500` | Podcast | Max episodes stored per channel | |
| `podcast.download.maxConcurrent.global` | `PodcastDownloadMaxConcurrentGlobal` | `2` | Podcast | Global concurrent episode downloads | |
| `podcast.download.maxConcurrent.perUser` | `PodcastDownloadMaxConcurrentPerUser` | `1` | Podcast | Per-user concurrent episode downloads | |
| `podcast.download.maxEnclosureBytes` | `PodcastDownloadMaxEnclosureBytes` | `2147483648` | Podcast | Max episode download size (2 GiB) | |
| `podcast.quota.maxBytesPerUser` | `PodcastQuotaMaxBytesPerUser` | `5368709120` (5 GB) | Podcast | Total podcast storage per user, 0 = unlimited | |
| `podcast.retention.downloadedEpisodesInDays` | `PodcastRetentionDownloadedEpisodesInDays` | `0` | Podcast | Days to keep downloads, 0 disables | |
| `podcast.retention.keepLastNEpisodes` | `PodcastRetentionKeepLastNEpisodes` | `0` | Podcast | Keep last N per channel, 0 disables | |
| `podcast.retention.keepUnplayedOnly` | `PodcastRetentionKeepUnplayedOnly` | `false` | Podcast | Delete downloads after play | |
| `podcast.recovery.stuckDownloadThresholdMinutes` | `PodcastRecoveryStuckDownloadThresholdMinutes` | `60` | Podcast | Stuck-download threshold | |
| `podcast.recovery.orphanedUsageThresholdHours` | `PodcastRecoveryOrphanedUsageThresholdHours` | `12` | Podcast | Orphaned temp-file threshold | |

## 9. Processing / registration / scripting

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `processing.artistNameReplacements` | `ProcessingArtistNameReplacements` | `{'AC/DC': ['AC; DC', 'AC;DC', 'AC/ DC', 'AC DC'] , 'Love/Hate': ['Love; Hate', 'Love;Hate', 'Love/ Hate', 'Love Hate'] }` | General (unset) | JSON dictionary of artist-name fragment replacements | |
| `processing.convertedExtension` | `ProcessingConvertedExtension` | `_converted` | General (unset) | Extension appended when converted; blank disables | |
| `processing.doContinueOnDirectoryProcessingErrors` | `ProcessingDoContinueOnDirectoryProcessingErrors` | `true` | General (unset) | Continue processing after directory errors | |
| `processing.doDeleteComments` | `ProcessingDoDeleteComments` | `true` | PluginProcess | Strip all comments from media files | |
| `processing.doDeleteOriginal` | `ProcessingDoDeleteOriginal` | `false` | General (unset) | Delete originals after processing (false = copy mode) | |
| `processing.deleteSourceResidueAfterIngest` | `ProcessingDeleteSourceResidueAfterIngest` | `true` | General (unset) | Delete residue from media-free source dirs after ingest (added 2026-07-14; own migration) | |
| `processing.dontDeleteExisitingMelodeeDataFiles` | `ProcessingDontDeleteExistingMelodeeDataFiles` | code: `false` | — | Keep existing Melodee data files. **Key string typo "Exisiting" is verbatim** | |
| `processing.fileExtensionsToDelete` | `ProcessingFileExtensionsToDelete` | `['log', 'lnk', 'lrc', 'doc']` | General (unset) | JSON array of extensions deleted on sight during processing | |
| `processing.doOverrideExistingMelodeeDataFiles` | `ProcessingDoOverrideExistingMelodeeDataFiles` | `true` | General (unset) | Overwrite existing Melodee data files when processing | |
| `processing.doUseCurrentYearAsDefaultOrigAlbumYearValue` | `ProcessingDoUseCurrentYearAsDefaultOrigAlbumYearValue` | `false` | General (unset) | Use current year when OrigAlbumYear is invalid | |
| `processing.duplicateAlbumPrefix` | `ProcessingDuplicateAlbumPrefix` | `_duplicate_ ` (trailing space in seed; comment claims `__duplicate_`) | General (unset) | Prefix marking duplicate album directories | |
| `processing.ignoredArticles` | `ProcessingIgnoredArticles` | `THE\|EL\|LA\|LOS\|LAS\|LE\|LES\|OS\|AS\|O\|A` | General (unset) | Pipe-delimited articles ignored when scanning | |
| `processing.ignoredPerformers` | `ProcessingIgnoredPerformers` | `""` | General (unset) | Performer names not turned into contributors | |
| `processing.ignoredProduction` | `ProcessingIgnoredProduction` | `['www.t.me;pmedia_music']` | General (unset) | Production names not turned into contributors | |
| `processing.ignoredPublishers` | `ProcessingIgnoredPublishers` | `['P.M.E.D.I.A','PMEDIA','PMEDIA GROUP']` | General (unset) | Publisher names not turned into contributors | |
| `processing.maximumAlbumDirectoryNameLength` | `ProcessingMaximumAlbumDirectoryNameLength` | `255` | General (unset) | Max album directory name length | |
| `processing.maximumArtistDirectoryNameLength` | `ProcessingMaximumArtistDirectoryNameLength` | `255` | General (unset) | Max artist directory name length | |
| `processing.maximumProcessingCount` | `ProcessingMaximumProcessingCount` | `0` | General (unset) | Max files to process, 0 = unlimited | |
| `processing.processedExtension` | `ProcessingProcessedExtension` | `_processed` | General (unset) | Extension appended when processed; blank disables | |
| `processing.songTitleRemovals` | `ProcessingSongTitleRemovals` | `[';', '(Remaster)', 'Remaster']` | General (unset) | JSON array of fragments removed from song titles | |
| `register.privateCode` | `RegisterPrivateCode` | not seeded (`""` = no code required) | — | Private/invite code required at registration | [SECRET-adjacent: access code] |
| `register.disabled` | `RegisterDisabled` | code: `false` | — | Disable open registration | |
| `scripting.enabled` | `ScriptingEnabled` | `false` | Scripting | Enable pre/post discovery scripts (Rust runtime: ADR-0135 Boa) | |
| `scripting.postDiscoveryScript` | `ScriptingPostDiscoveryScript` | `""` | Scripting | Script run after inbound processing; blank disables | |
| `scripting.preDiscoveryScript` | `ScriptingPreDiscoveryScript` | `""` | Scripting | Script run before inbound processing; blank disables | |

## 10. Scrobbling (Category: Scrobbling)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `scrobbling.enabled` | `ScrobblingEnabled` | `true` | Scrobbling | Master scrobbling switch | |
| `scrobbling.lastFm.apiKey` | `ScrobblingLastFmApiKey` | `""` | Scrobbling | Last.fm API key | [SECRET] |
| `scrobbling.lastFm.sharedSecret` | `ScrobblingLastFmSharedSecret` | `""` | Scrobbling | Last.fm shared secret | [SECRET] |
| `scrobbling.lastFm.Enabled` | `ScrobblingLastFmEnabled` | `false` | Scrobbling | Last.fm scrobbling switch. **Capital "E" segment is verbatim** | |

## 11. Search engines (Category: SearchEngine)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `searchEngine.defaultPageSize` | `SearchEngineDefaultPageSize` | `20` | SearchEngine | Default search page size | |
| `searchEngine.maximumAllowedPageSize` | `SearchEngineMaximumAllowedPageSize` | `1000` | SearchEngine | Max search page size | |
| `searchEngine.brave.enabled` | `SearchEngineBraveEnabled` | code: `false` | — | Brave image search switch | |
| `searchEngine.brave.apiKey` | `SearchEngineBraveApiKey` | not seeded (`""`) | — | Brave Search API key | [SECRET] |
| `searchEngine.brave.baseUrl` | `SearchEngineBraveBaseUrl` | code: `https://api.search.brave.com` | — | Brave API base URL | |
| `searchEngine.brave.imageSearchPath` | `SearchEngineBraveImageSearchPath` | not seeded (no default) | — | Brave image-search path | |
| `searchEngine.deezer.enabled` | `SearchEngineDeezerEnabled` | `true` | SearchEngine | Deezer search engine switch | |
| `searchEngine.itunes.enabled` | `SearchEngineITunesEnabled` | `true` | SearchEngine | iTunes search engine switch | |
| `searchEngine.lastFm.Enabled` | `SearchEngineLastFmEnabled` | `true` | SearchEngine | Last.fm metadata search switch. **Capital "E" is verbatim** | |
| `searchEngine.musicbrainz.enabled` | `SearchEngineMusicBrainzEnabled` | `true` | SearchEngine | MusicBrainz search engine switch | |
| `searchEngine.musicbrainz.importMaximumToProcess` | `SearchEngineMusicBrainzImportMaximumToProcess` | `0` | SearchEngine | Debug aid: max dump batches to import, 0 = unlimited | |
| `searchEngine.musicbrainz.importBatchSize` | `SearchEngineMusicBrainzImportBatchSize` | `50000` | SearchEngine | Records per commit during dump import | |
| `searchEngine.musicbrainz.storagePath` | `SearchEngineMusicBrainzStoragePath` | `/melodee_test/search-engine-storage/musicbrainz/` | SearchEngine | Local dump/database storage path. **Seed points at a test path** | |
| `searchEngine.musicbrainz.importLastImportTimestamp` | `SearchEngineMusicBrainzImportLastImportTimestamp` | `""` | SearchEngine | Last successful import timestamp | |
| `searchEngine.artistSearchDatabaseRefreshInDays` | `SearchEngineArtistSearchDatabaseRefreshInDays` | `14` | SearchEngine | Artist album refresh cadence, 0 disables | |
| `searchEngine.spotify.enabled` | `SearchEngineSpotifyEnabled` | `false` | SearchEngine | Spotify search engine switch | |
| `searchEngine.spotify.apiKey` | `SearchEngineSpotifyApiKey` | `""` | SearchEngine | Spotify API key | [SECRET] |
| `searchEngine.spotify.sharedSecret` | `SearchEngineSpotifyClientSecret` | `""` | SearchEngine | Spotify client secret. **Constant name ≠ key suffix** | [SECRET] |
| `searchEngine.spotify.accessToken` | `SearchEngineSpotifyAccessToken` | `""` | SearchEngine | Cached Spotify token JSON with expiry | [SECRET] |
| `searchEngine.discogs.enabled` | `SearchEngineDiscogsEnabled` | `false` | SearchEngine | Discogs search engine switch | |
| `searchEngine.discogs.userToken` | `SearchEngineDiscogsUserToken` | `""` | SearchEngine | Discogs user token | [SECRET] |
| `searchEngine.wikidata.enabled` | `SearchEngineWikiDataEnabled` | `false` | SearchEngine | WikiData search engine switch | |
| `searchEngine.metalApi.enabled` | `SearchEngineMetalApiEnabled` | `false` | SearchEngine | Metal API search engine switch | |
| `searchEngine.userAgent` | `SearchEngineUserAgent` | `Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0` | SearchEngine | User-Agent sent with search requests | |
| `searchResults.defaultPageSize` | `SearchResultsDefaultPageSize` | code: falls back to `defaults.pagesize`, then `50` | — | UI search-results page size | |

## 12. Security (blacklists) / system

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `security.blacklistedEmails` | `SecurityBlacklistedEmails` | not seeded (`""`; JSON array) | — | Registration/login email blacklist | |
| `security.blacklistedIPs` | `SecurityBlacklistedIPs` | not seeded (`""`; JSON array) | — | IP blacklist | |
| `system.baseUrl` | `SystemBaseUrl` | `** REQUIRED: THIS MUST BE EDITED **` (`MelodeeConfiguration.RequiredNotSetValue`) | System | Base URL for shareable links and image URLs; relative `/images/...` fallback when unset | |
| `system.siteName` | `SystemSiteName` | `Melodee` | System | Instance name for emails and UI branding (also `SettingDefaults.DefaultSiteName`) | |
| `system.isDownloadingEnabled` | `SystemIsDownloadingEnabled` | `true` | System | Enable downloads | |
| `system.maxUploadSize` | `SystemMaxUploadSize` | `5242880` (5 MiB) | System | UI upload cap in bytes | |
| `system.onboardingCompletedAt` | `SystemOnboardingCompletedAt` | not seeded (written by `OnboardingStateService` at completion; unset = onboarding required) | — | Onboarding completion marker | |

## 13. `// Streaming settings` / transcoding / user device profile / UI / validation

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `streaming.useBufferedResponses` | `StreamingUseBufferedResponses` | code: `false` | — | Fall back to buffered stream responses | |
| `streaming.maxConcurrentStreams.global` | `StreamingMaxConcurrentStreamsGlobal` | code: `0` (≤ 0 = unlimited) | — | Global concurrent stream cap | |
| `streaming.maxConcurrentStreams.perUser` | `StreamingMaxConcurrentStreamsPerUser` | code: `0` (≤ 0 = unlimited) | — | Per-user concurrent stream cap | |
| `transcoding.command.aac` | `TranscodingCommandAac` | `{ 'format': 'Aac', 'bitrate: 256, 'command': 'ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -c:a aac -f adts -' }` | Transcoding | AAC transcode command. **Malformed quotes in seed are verbatim** | |
| `transcoding.command.mp3` | `TranscodingCommandMp3` | `{ 'format': 'Mp3', 'bitrate: 192, 'command': 'ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -f mp3 -' }` | Transcoding | MP3 transcode command. **Malformed quotes verbatim** | |
| `transcoding.command.opus` | `TranscodingCommandOpus` | `{ 'format': 'Opus', 'bitrate: 128, 'command': 'ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -c:a libopus -f opus -' }` | Transcoding | Opus transcode command. **Malformed quotes verbatim** | |
| `transcoding.default` | `TranscodingDefault` | `raw` | Transcoding | Default transcoding format | |
| `userDeviceProfile.enabled` | `UserDeviceProfileEnabled` | `true` | System | Per-user/per-device transcoding profiles. **Never consumed in legacy** (see WP-10 inventory) | |
| `userinterface.toastAutoCloseTime` | `UserInterfaceToastAutoCloseTime` | `2000` | General (unset) | Toast auto-close time (ms) | |
| `validation.maximumAlbumYear` | `ValidationMaximumAlbumYear` | `2150` | Validation | Max valid album year | |
| `validation.maximumSongNumber` | `ValidationMaximumSongNumber` | `9999` | Validation | Max song number. NB: `SettingDefaults` code constant = 1000 | |
| `validation.minimumAlbumYear` | `ValidationMinimumAlbumYear` | `1860` | Validation | Min album year. NB: `SettingDefaults` = 1900 | |
| `validation.minimumSongCount` | `ValidationMinimumSongCount` | `3` | Validation | Min songs for a valid album, 0 disables. NB: `SettingDefaults` = 1 | |
| `validation.minimumAlbumDuration` | `ValidationMinimumAlbumDuration` | `10` | Validation | Min album duration (minutes), 0 disables. NB: `SettingDefaults` = 1 | |

## 14. `// Email settings` (Category unset → General)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `email.enabled` | `EmailEnabled` | `false` | General (unset) | Enable SMTP sending (password resets, notifications) | |
| `email.fromName` | `EmailFromName` | `Melodee` | General (unset) | From display name | |
| `email.fromEmail` | `EmailFromEmail` | `""` (required for sending) | General (unset) | From address | |
| `email.smtpHost` | `EmailSmtpHost` | `""` (required for sending) | General (unset) | SMTP hostname | |
| `email.smtpPort` | `EmailSmtpPort` | `587` | General (unset) | SMTP port (587 StartTLS / 465 SSL / 25 plain) | |
| `email.smtpUsername` | `EmailSmtpUsername` | `""` | General (unset) | SMTP auth username | [SECRET-adjacent: credential] |
| `email.smtpPassword` | `EmailSmtpPassword` | `""` (seed comment directs admins to env var `email_smtpPassword`) | General (unset) | SMTP auth password | [SECRET] |
| `email.smtpUseSsl` | `EmailSmtpUseSsl` | `false` | General (unset) | SSL connection (port 465) | |
| `email.smtpUseStartTls` | `EmailSmtpUseStartTls` | `true` | General (unset) | StartTLS (port 587) | |
| `email.resetPassword.subject` | `EmailResetPasswordSubject` | `Reset your Melodee password` | General (unset) | Password-reset email subject | |
| `email.resetPassword.textBodyTemplate` | `EmailResetPasswordTextBodyTemplate` | not seeded (no default) | — | **Dead key: never seeded, never consumed in `src`** | |
| `email.resetPassword.htmlBodyTemplate` | `EmailResetPasswordHtmlBodyTemplate` | not seeded (no default) | — | **Dead key: never seeded, never consumed in `src`** | |

## 15. `// Security settings`

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `security.secretKey` | `SecuritySecretKey` | not seeded (auto-generated and persisted by `StartupConfigurationUpdater` when absent; ≥ 32 chars enforced by `SecretProtector`) | — | Server auth/token signing secret | [SECRET] |
| `security.passwordResetTokenExpiryMinutes` | `SecurityPasswordResetTokenExpiryMinutes` | `60` | General (unset) | Password-reset link validity | |

## 16. `// Jellyfin API settings` (Category unset → General)

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `jellyfin.enabled` | `JellyfinEnabled` | `true` | General (unset) | Enable Jellyfin API compatibility | |
| `jellyfin.routePrefix` | `JellyfinRoutePrefix` | `/api/jf` | General (unset) | Internal Jellyfin route prefix | |
| `jellyfin.token.expiresAfterHours` | `JellyfinTokenExpiresAfterHours` | `168` (7 days) | General (unset) | Jellyfin token expiry | |
| `jellyfin.token.maxActivePerUser` | `JellyfinTokenMaxActivePerUser` | `10` | General (unset) | Max active Jellyfin tokens per user | |
| `jellyfin.token.allowLegacyHeaders` | `JellyfinTokenAllowLegacyHeaders` | `true` | General (unset) | Allow X-Emby-*/X-MediaBrowser-* auth headers | |
| `jellyfin.token.pepper` | `JellyfinTokenPepper` | `ChangeThisPepperInProduction` | General (unset) | Server-side pepper for token hashing. **Ships with a known placeholder default** | [SECRET] |
| `jellyfin.rateLimit.apiRequestsPerPeriod` | `JellyfinRateLimitApiRequestsPerPeriod` | `200` | General (unset) | API requests per rate-limit period | |
| `jellyfin.rateLimit.apiPeriodSeconds` | `JellyfinRateLimitApiPeriodSeconds` | `60` | General (unset) | Rate-limit period length | |
| `jellyfin.rateLimit.streamConcurrentPerUser` | `JellyfinRateLimitStreamConcurrentPerUser` | `2` | General (unset) | Concurrent streams per user | |

## 17. `// Theme settings`

| Key | Constant | Default | Category | Description | Secret |
|---|---|---|---|---|---|
| `theme.libraryPath` | `ThemeLibraryPath` | not seeded (no default) | — | **Effectively dead key: never read via `SettingRegistry`; `ThemeService` resolves the path from the Themes `Library` row** | |
| `system.defaultTheme` | `SystemDefaultTheme` | code: `melodee-dark` / `dark` | — | Default UI theme | |
| `theme.maxUploadSizeMb` | `ThemeMaxUploadSizeMb` | code: `50` (`ThemeService.MaxThemeUploadSizeMb`) | — | Theme pack upload cap (MB) | |
| `theme.enforceContrastValidation` | `ThemeEnforceContrastValidation` | code: `false` | — | Reject themes failing contrast checks | |

## Secrets and credential-adjacent keys

These drive the redaction rules for the Rust import/export path (ADR-0134 §3:
`server_settings.is_secret`; secret values are write-only over the API and
redacted in every export, per PRD-AUTH-007). Legacy logging already treats them
specially (`ConfigurationLogRedactor` redacts values; `GetSettingValue`
deliberately avoids logging the key name on parse errors).

Hard secrets (12):

| Key | Notes |
|---|---|
| `encryption.privateKey` | **Ships with a hardcoded, publicly visible default AES key in the seed** — treat every migrated value as compromised; regenerate on import |
| `security.secretKey` | Auto-generated at startup when absent; ≥ 32 chars enforced |
| `jellyfin.token.pepper` | Ships with known placeholder `ChangeThisPepperInProduction`; regenerate on import |
| `email.smtpPassword` | Seed comment steers admins to env var `email_smtpPassword` |
| `mpd.password` | MPD backend auth (jukebox, waived per ADR-0133) |
| `scrobbling.lastFm.apiKey` | |
| `scrobbling.lastFm.sharedSecret` | |
| `searchEngine.spotify.apiKey` | |
| `searchEngine.spotify.sharedSecret` | Spotify client secret (constant `SearchEngineSpotifyClientSecret`) |
| `searchEngine.spotify.accessToken` | Cached token JSON containing expiry |
| `searchEngine.discogs.userToken` | |
| `searchEngine.brave.apiKey` | |

Credential-adjacent (2): `email.smtpUsername` (auth credential), and
`register.privateCode` (registration access code — sensitive in effect if not in
kind).

## Consumption pattern (read/write layering)

- **Storage.** EF Core `Setting` entity → `Settings` table. Columns: `Id` (int
  PK), `ApiKey` (Guid), `Key` (string, unique-enforced in service code),
  `Value` (string — **all values are strings**; booleans/ints/JSON arrays are
  parsed at read), `Category` (int enum, nullable → General), `Comment`,
  `Description`, `IsLocked`, `SortOrder`, `Tags`, NodaTime audit timestamps.
  Seed Ids are grouped by category block (1xx Api, 2xx Conversion, 3xx
  Formatting, 4xx Imaging, 5xx Magic, 7xx PluginProcess, 9xx SearchEngine,
  10xx Scrobbling, 11xx System, 12xx Transcoding, 13xx Validation, 14xx Jobs,
  15xx Email, 16xx Security, 17xx Jellyfin/late additions, 18xx Podcast,
  19xx Jukebox); `ApiKey`s are deterministic `SeedGuid("Setting", id)` values.
- **Read path.** `MelodeeConfigurationFactory` (singleton behind
  `IMelodeeConfigurationFactory`) loads **all** rows into a case-insensitive
  `Dictionary<string, object?>`, then overlays **every process environment
  variable** with `_`→`.` key mapping (`UpdateWithEnvironmentVariables` — so
  `email_smtpPassword` overrides `email.smtpPassword`, and unknown env vars are
  *added* as settings), caches the result, and raises `ConfigurationChanged` on
  `Reset()`. Services inject `IMelodeeConfiguration` and call
  `GetValue<T>(SettingRegistry.X)` — typed conversion via `SafeParser`, with
  `??`/lambda fallback defaults at the call site. Unseeded keys therefore behave
  as if their code fallback were the default. `MelodeeConfiguration.AllSettings`
  builds the full key universe by reflecting over `SettingRegistry` public
  constants, so the registry file is the runtime-authoritative enumeration.
  `SettingService.ListAsync` additionally overlays env vars onto paged admin
  reads.
- **Write path.** `SettingService`
  (`src/Melodee.Common/Services/SettingService.cs`): admin CRUD over the table
  with a per-key cache (`urn:setting:{key}`); `SetAsync` upserts and
  **auto-creates unknown keys** with comment `Auto-created setting for {key}`
  (this is how unseeded keys enter the DB when first set from UI/onboarding).
  Every update/delete clears the cache and calls
  `IMelodeeConfigurationFactory.Reset()`, so the next read rebuilds the merged
  dictionary.
- **Runtime writers.** `StartupConfigurationUpdater` (Blazor host startup)
  generates and persists `security.secretKey` when absent. `OnboardingStateService`
  writes `system.onboardingCompletedAt` on wizard completion;
  `OnboardingRequirements`/`SetupCheckService` gate setup on `system.baseUrl`,
  `system.siteName`, and `security.secretKey` (the only three "required"
  onboarding keys). The onboarding import path whitelists known
  `SettingRegistry` keys plus keys already present in the DB.

## Parity quirks to preserve (or consciously retire with an ADR)

1. **Key-string casing inconsistencies** — `defaults.pagesize` (lowercase "s"),
   `scrobbling.lastFm.Enabled` and `searchEngine.lastFm.Enabled` (capital "E"
   final segment), `openSubsonicServer.openSubsonic.serverSupportedVersion`
   (segment repeated differently across the four OpenSubsonic keys), and
   `searchEngine.spotify.sharedSecret` whose constant is
   `SearchEngineSpotifyClientSecret`. Any migration/import mapping must key off
   the exact strings, not the constant names.
2. **The `dontDeleteExisitingMelodeeDataFiles` typo** — "Exisiting" is the
   actual key string in `ProcessingDontDeleteExistingMelodeeDataFiles`.
   Preserving it verbatim is required for DB migration parity; a cleanup rename
   needs its own ADR and migration plan.
3. **Malformed transcoding seed strings** — `transcoding.command.aac/mp3/opus`
   seed values contain unbalanced quotes (`'bitrate: 192, 'command': ...`) and
   are not valid JSON. Do not "fix" them silently; Phase 8 captured surfaces
   already treat transcoding defaults as captured contract.
4. **Hardcoded `encryption.privateKey` default** — the seed ships a real,
   publicly visible AES key. The Rust import must regenerate/replace it, never
   carry it over (PRD-AUTH-007); same for the placeholder
   `jellyfin.token.pepper`.
5. **Dead keys** — `email.resetPassword.textBodyTemplate` and
   `email.resetPassword.htmlBodyTemplate` are declared in the registry but never
   seeded and never consumed anywhere in `src` (templates are hardcoded in the
   email sender). `theme.libraryPath` is declared but never read via
   `SettingRegistry` (`ThemeService` uses the Themes `Library` row). These exist
   in the 190 count but carry no behavior.
6. **Seed vs `SettingDefaults` divergence** — validation code constants
   (`ValidationMinimumAlbumYear = 1900`, `MaximumSongNumber = 1000`,
   `MinimumSongCount = 1`, `MinimumAlbumDuration = 1`) disagree with the seeded
   values (1860 / 9999 / 3 / 10); the seeded values win at runtime because
   `GetValue` reads the DB first. `processing.duplicateAlbumPrefix` seeds
   `_duplicate_ ` with a trailing space while its comment claims `__duplicate_`.
7. **Test-path seed** — `searchEngine.musicbrainz.storagePath` seeds
   `/melodee_test/...`, a development artifact.

## Rust-side mapping (current coverage vs WP-08 net-new)

Verified against `crates/`, `apps/melodee-server/src/`, and
`crates/melodee-persistence/migrations/` on 2026-08-14. **No `server_settings`
table exists yet**; ADR-0134 §3 assigns it (plus `onboarding_state`) to WP-08.

- **Already partially covered (not DB-backed settings):**
  - `jellyfin.token.*`: `jellyfin_access_tokens` table (migration
    `0021_phase8_jellyfin.sql`) stores tokens; the pepper comes from env
    `MELODEE__JELLYFIN__TOKEN_PEPPER` (fixture default in `main.rs`), while
    `token_expiry_hours: 168` and `max_active_tokens: 10` are **hard-coded** in
    `JfState` construction. `jellyfin.enabled`, `jellyfin.routePrefix`, and the
    three `jellyfin.rateLimit.*` keys have no Rust setting equivalent.
  - `podcast.*`: data tables in `0013_phase7_native_podcasts.sql`; worker
    tunables (enabled, root, retention) come from
    `podcast_worker::PodcastConfig::from_env()`. None of the 15 podcast keys are
    DB-backed.
  - `openSubsonicServer.*`: server type (`Melodee`), supported version
    (`1.16.1`), license email, and index artist limit are fixed captured-contract
    values in the Phase 8 OpenSubsonic surface, not settings.
  - `userDeviceProfile.enabled`: WP-10 landed `device_profiles` (migration
    `0026_phase9_device_profiles.sql`) per ADR-0134 §5; the legacy flag itself
    was never consumed and has no Rust equivalent by design.
  - Themes: `native_themes`/`native_theme_assets` (migration
    `0015_phase7_native_discovery.sql`) store theme content; `system.defaultTheme`
    and the `theme.*` keys have no Rust setting equivalent.
- **Env convention differs by design:** the Rust server already uses
  `MELODEE__SECTION__KEY` (double-underscore, config-crate style), whereas legacy
  maps *every* environment variable with `_`→`.`. WP-08 must define the layered
  precedence (env/file over `server_settings`, per PRD-CONFIG-003) rather than
  replicate the legacy all-env-vars overlay.
- **Net-new for WP-08's `server_settings` (ADR-0134 §3):** everything else —
  all `processing.*`, `magic.*`, `imaging.*`, `validation.*`, `conversion.*`,
  `filtering.*`, `formatting.*`, `defaults.*`, `jobs.*` cron keys, `scripting.*`
  (Boa runtime per ADR-0135), `scrobbling.*`, `searchEngine.*`, `searchResults.*`,
  `security.*`, `email.*`, `streaming.*`, `transcoding.*`, `playlist.*`,
  `plugin.*`, `register.*`, `partyMode.*`, `lyrics.*`, `system.*`, and the
  jukebox/mpv/mpd groups (waived per ADR-0133 but still importable as inert
  rows if migration parity requires the key set). Jukebox keys should be
  imported-but-inert at most; no Rust consumer should be built for them.

Uncertainty notes: which of the net-new groups get first-class typed config
versus opaque key/value rows is a WP-08 design decision not yet made; the
checklist `design/evidence/phase-9/checklists/10-onboarding-config-notifications.md`
tracks the acceptance criteria (settings manageable via CLI; env/file + DB
layering).
