-- Deterministic legacy (C# Melodee) source-schema fixture for
-- migration tests (ADR-0148).
--
-- Derivation: dumped from the live frozen-baseline database (legacy oracle
-- checkout 4028b58dbd3302ed431e2c5b32d9dde5634db218, migration head
-- 20260714155936_AddDeleteSourceResidueAfterIngestSetting) via
-- information_schema: exact column sets, nullability, and primary keys,
-- with EF identity/sequence defaults stripped (tests seed explicit ids) and
-- character varying widths normalized to text (the tooling never depends on
-- widths). Quoted PascalCase identifiers match the real legacy schema
-- exactly. The __EFMigrationsHistory rows are the schema fingerprint gate:
-- the importer fails closed unless the final row is
-- 20260714155936_AddDeleteSourceResidueAfterIngestSetting.
--
-- Regenerate only against the frozen oracle checkout; never hand-edit to add
-- behavior. Test datasets are seeded by the test suite, not by this file.

CREATE TABLE "__EFMigrationsHistory" (
    "MigrationId" text PRIMARY KEY,
    "ProductVersion" text NOT NULL
);
INSERT INTO "__EFMigrationsHistory" ("MigrationId", "ProductVersion") VALUES
    ('20260524160636_InitialBaseline', '10.0.0'),
    ('20260714155936_AddDeleteSourceResidueAfterIngestSetting', '10.0.0');

CREATE TABLE "Albums" (
    "Id" integer NOT NULL,
    "ArtistId" integer NOT NULL,
    "Name" text NOT NULL,
    "NameNormalized" text NOT NULL,
    "SortName" text,
    "AlbumStatus" smallint NOT NULL,
    "MetaDataStatus" integer NOT NULL,
    "ImageCount" integer,
    "AlbumType" smallint NOT NULL,
    "OriginalReleaseDate" date,
    "ReleaseDate" date NOT NULL,
    "IsCompilation" boolean NOT NULL,
    "SongCount" smallint,
    "Duration" double precision NOT NULL,
    "Genres" text[],
    "Moods" text[],
    "Comment" text,
    "ReplayGain" double precision,
    "ReplayPeak" double precision,
    "Directory" text NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    "AlternateNames" text,
    "LastPlayedAt" timestamptz,
    "LastMetaDataUpdatedAt" timestamptz,
    "PlayedCount" integer NOT NULL,
    "ItunesId" text,
    "AmgId" text,
    "DeezerId" integer,
    "DiscogsId" text,
    "WikiDataId" text,
    "MusicBrainzId" uuid,
    "LastFmId" text,
    "SpotifyId" text,
    "CalculatedRating" numeric NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "ArtistRelation" (
    "Id" integer NOT NULL,
    "ArtistId" integer NOT NULL,
    "RelatedArtistId" integer NOT NULL,
    "ArtistRelationType" integer NOT NULL,
    "RelationStart" timestamptz,
    "RelationEnd" timestamptz,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Artists" (
    "Id" integer NOT NULL,
    "Name" text NOT NULL,
    "NameNormalized" text NOT NULL,
    "SortName" text,
    "RealName" text,
    "Directory" text NOT NULL,
    "Roles" text,
    "AlbumCount" integer NOT NULL,
    "SongCount" integer NOT NULL,
    "LibraryId" integer NOT NULL,
    "Biography" text,
    "ImageCount" integer,
    "MetaDataStatus" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    "AlternateNames" text,
    "LastPlayedAt" timestamptz,
    "LastMetaDataUpdatedAt" timestamptz,
    "PlayedCount" integer NOT NULL,
    "ItunesId" text,
    "AmgId" text,
    "DeezerId" integer,
    "DiscogsId" text,
    "WikiDataId" text,
    "MusicBrainzId" uuid,
    "LastFmId" text,
    "SpotifyId" text,
    "CalculatedRating" numeric NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Bookmarks" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "SongId" integer NOT NULL,
    "Comment" text,
    "Position" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    "AlternateNames" text,
    "LastPlayedAt" timestamptz,
    "LastMetaDataUpdatedAt" timestamptz,
    "PlayedCount" integer NOT NULL,
    "ItunesId" text,
    "AmgId" text,
    "DeezerId" integer,
    "DiscogsId" text,
    "WikiDataId" text,
    "MusicBrainzId" uuid,
    "LastFmId" text,
    "SpotifyId" text,
    "CalculatedRating" numeric NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "ChartItems" (
    "Id" integer NOT NULL,
    "ChartId" integer NOT NULL,
    "Rank" integer NOT NULL,
    "ArtistName" text NOT NULL,
    "AlbumTitle" text NOT NULL,
    "ReleaseYear" integer,
    "LinkedArtistId" integer,
    "LinkedAlbumId" integer,
    "LinkStatus" smallint NOT NULL,
    "LinkConfidence" numeric,
    "LinkNotes" text,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Charts" (
    "Id" integer NOT NULL,
    "Slug" text NOT NULL,
    "Title" text NOT NULL,
    "SourceName" text,
    "SourceUrl" text,
    "Year" integer,
    "IsVisible" boolean NOT NULL,
    "IsGeneratedPlaylistEnabled" boolean NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Contributors" (
    "Id" integer NOT NULL,
    "Role" text NOT NULL,
    "SubRole" text,
    "ArtistId" integer,
    "ContributorName" text,
    "MetaTagIdentifier" integer NOT NULL,
    "SongId" integer,
    "AlbumId" integer NOT NULL,
    "ContributorType" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "JellyfinAccessTokens" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "TokenHash" text NOT NULL,
    "TokenPrefixHash" text NOT NULL,
    "TokenSalt" text NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUsedAt" timestamptz,
    "ExpiresAt" timestamptz,
    "RevokedAt" timestamptz,
    "Client" text,
    "Device" text,
    "DeviceId" text,
    "Version" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "JobHistories" (
    "Id" integer NOT NULL,
    "JobName" text NOT NULL,
    "StartedAt" timestamptz NOT NULL,
    "CompletedAt" timestamptz,
    "DurationInMs" double precision,
    "Success" boolean NOT NULL,
    "ErrorMessage" text,
    "WasManualTrigger" boolean NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Libraries" (
    "Id" integer NOT NULL,
    "Name" text NOT NULL,
    "ArtistCount" integer,
    "AlbumCount" integer,
    "SongCount" integer,
    "Path" text NOT NULL,
    "Type" integer NOT NULL,
    "LastScanAt" timestamptz,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "LibraryAccessControls" (
    "Id" integer NOT NULL,
    "LibraryId" integer NOT NULL,
    "UserGroupId" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "LibraryScanHistories" (
    "Id" integer NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LibraryId" integer NOT NULL,
    "ForArtistId" integer,
    "ForAlbumId" integer,
    "FoundArtistsCount" integer NOT NULL,
    "FoundAlbumsCount" integer NOT NULL,
    "FoundSongsCount" integer NOT NULL,
    "DurationInMs" double precision NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PartyAuditEvents" (
    "Id" integer NOT NULL,
    "PartySessionId" integer NOT NULL,
    "UserId" integer NOT NULL,
    "EventType" integer NOT NULL,
    "PayloadJson" text,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PartyPlaybackStates" (
    "Id" integer NOT NULL,
    "PartySessionId" integer NOT NULL,
    "CurrentQueueItemApiKey" uuid,
    "PositionSeconds" double precision NOT NULL,
    "IsPlaying" boolean NOT NULL,
    "Volume" double precision,
    "LastHeartbeatAt" timestamptz,
    "UpdatedByUserId" integer,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PartyQueueItems" (
    "Id" integer NOT NULL,
    "PartySessionId" integer NOT NULL,
    "SongApiKey" uuid NOT NULL,
    "EnqueuedByUserId" integer NOT NULL,
    "EnqueuedAt" timestamptz NOT NULL,
    "SortOrder" integer NOT NULL,
    "Source" text,
    "Note" text,
    "IsLocked" boolean NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PartySessionEndpoints" (
    "Id" integer NOT NULL,
    "OwnerUserId" integer,
    "Name" text NOT NULL,
    "Type" integer NOT NULL,
    "CapabilitiesJson" text,
    "LastSeenAt" timestamptz,
    "IsShared" boolean NOT NULL,
    "Room" text,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PartySessionParticipants" (
    "PartySessionId" integer NOT NULL,
    "UserId" integer NOT NULL,
    "Role" integer NOT NULL,
    "JoinedAt" timestamptz NOT NULL,
    "LastSeenAt" timestamptz,
    "IsBanned" boolean NOT NULL,
    PRIMARY KEY ("PartySessionId", "UserId")
);

CREATE TABLE "PartySessions" (
    "Id" integer NOT NULL,
    "Name" text NOT NULL,
    "OwnerUserId" integer NOT NULL,
    "Status" integer NOT NULL,
    "JoinCodeHash" text,
    "ActiveEndpointId" uuid,
    "QueueRevision" bigint NOT NULL,
    "PlaybackRevision" bigint NOT NULL,
    "IsQueueLocked" boolean NOT NULL,
    "IsEndpointOffline" boolean NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PlayQues" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "SongId" integer NOT NULL,
    "SongApiKey" uuid NOT NULL,
    "IsCurrentSong" boolean NOT NULL,
    "ChangedBy" text NOT NULL,
    "Position" double precision NOT NULL,
    "PlayQueId" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Players" (
    "Id" integer NOT NULL,
    "Name" text NOT NULL,
    "UserAgent" text,
    "UserId" integer NOT NULL,
    "Client" text NOT NULL,
    "IpAddress" text,
    "LastSeenAt" timestamptz NOT NULL,
    "MaxBitRate" integer,
    "ScrobbleEnabled" boolean NOT NULL,
    "TranscodingId" text,
    "Hostname" text,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PlaylistSong" (
    "SongId" integer NOT NULL,
    "PlaylistId" integer NOT NULL,
    "SongApiKey" uuid NOT NULL,
    "PlaylistOrder" integer NOT NULL,
    PRIMARY KEY ("SongId", "PlaylistId")
);

CREATE TABLE "PlaylistUploadedFileItems" (
    "Id" integer NOT NULL,
    "PlaylistUploadedFileId" integer NOT NULL,
    "SongId" integer,
    "Status" integer NOT NULL,
    "RawReference" text NOT NULL,
    "NormalizedReference" text NOT NULL,
    "HintsJson" text,
    "LastAttemptAt" timestamptz,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PlaylistUploadedFiles" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "OriginalFileName" text NOT NULL,
    "ContentType" text,
    "Length" bigint NOT NULL,
    "Content" bytea NOT NULL,
    "PlaylistId" integer,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Playlists" (
    "Id" integer NOT NULL,
    "Name" text NOT NULL,
    "Comment" text,
    "UserId" integer NOT NULL,
    "IsPublic" boolean NOT NULL,
    "SongCount" smallint,
    "Duration" double precision NOT NULL,
    "AllowedUserIds" text,
    "SongId" integer,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PodcastChannels" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "FeedUrl" text NOT NULL,
    "Title" text NOT NULL,
    "TitleNormalized" text,
    "Description" text,
    "SiteUrl" text,
    "ImageUrl" text,
    "CoverArtLocalPath" text,
    "Etag" text,
    "LastModified" timestamptz,
    "LastSyncAt" timestamptz,
    "LastSyncAttemptAt" timestamptz,
    "LastSyncError" text,
    "ConsecutiveFailureCount" integer NOT NULL,
    "NextSyncAt" timestamptz,
    "IsDeleted" boolean NOT NULL,
    "MaxDownloadedEpisodes" integer,
    "MaxStorageBytes" bigint,
    "AutoDownloadEnabled" boolean NOT NULL,
    "RefreshIntervalHours" integer,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PodcastEpisodeBookmarks" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "PodcastEpisodeId" integer NOT NULL,
    "PositionSeconds" integer NOT NULL,
    "Comment" text,
    "CreatedAt" timestamptz NOT NULL,
    "UpdatedAt" timestamptz NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "PodcastEpisodes" (
    "Id" integer NOT NULL,
    "PodcastChannelId" integer NOT NULL,
    "Guid" text,
    "Title" text NOT NULL,
    "TitleNormalized" text,
    "Description" text,
    "PublishDate" timestamptz,
    "EnclosureUrl" text NOT NULL,
    "EnclosureLength" bigint,
    "MimeType" text,
    "EpisodeKey" text NOT NULL,
    "DownloadStatus" integer NOT NULL,
    "DownloadError" text,
    "LocalPath" text,
    "LocalFileSize" bigint,
    "Duration" interval,
    "QueuedAt" timestamptz,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "RadioStations" (
    "Id" integer NOT NULL,
    "Name" text NOT NULL,
    "StreamUrl" text NOT NULL,
    "HomePageUrl" text,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "RefreshTokens" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "HashedToken" text NOT NULL,
    "TokenFamily" text NOT NULL,
    "IssuedAt" timestamptz NOT NULL,
    "ExpiresAt" timestamptz NOT NULL,
    "RevokedAt" timestamptz,
    "RevokedReason" text,
    "ReplacedByToken" text,
    "SessionStartedAt" timestamptz NOT NULL,
    "DeviceId" text,
    "UserAgent" text,
    "IpAddress" text,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "RequestComments" (
    "Id" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "RequestId" integer NOT NULL,
    "ParentCommentId" integer,
    "Body" text NOT NULL,
    "IsSystem" boolean NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "CreatedByUserId" integer,
    PRIMARY KEY ("Id")
);

CREATE TABLE "RequestParticipants" (
    "RequestId" integer NOT NULL,
    "UserId" integer NOT NULL,
    "IsCreator" boolean NOT NULL,
    "IsCommenter" boolean NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    PRIMARY KEY ("RequestId", "UserId")
);

CREATE TABLE "RequestUserStates" (
    "RequestId" integer NOT NULL,
    "UserId" integer NOT NULL,
    "LastSeenAt" timestamptz NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "UpdatedAt" timestamptz NOT NULL,
    PRIMARY KEY ("RequestId", "UserId")
);

CREATE TABLE "Requests" (
    "Id" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "Category" integer NOT NULL,
    "Status" integer NOT NULL,
    "Description" text NOT NULL,
    "ArtistName" text,
    "TargetArtistApiKey" uuid,
    "AlbumTitle" text,
    "TargetAlbumApiKey" uuid,
    "SongTitle" text,
    "TargetSongApiKey" uuid,
    "ReleaseYear" integer,
    "ExternalUrl" text,
    "Notes" text,
    "CreatedAt" timestamptz NOT NULL,
    "CreatedByUserId" integer NOT NULL,
    "UpdatedAt" timestamptz NOT NULL,
    "UpdatedByUserId" integer NOT NULL,
    "LastActivityAt" timestamptz NOT NULL,
    "LastActivityUserId" integer,
    "LastActivityType" integer NOT NULL,
    "ArtistNameNormalized" text,
    "AlbumTitleNormalized" text,
    "SongTitleNormalized" text,
    "DescriptionNormalized" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "SearchHistories" (
    "Id" integer NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "ByUserId" integer NOT NULL,
    "ByUserAgent" text,
    "SearchQuery" text,
    "FoundArtistsCount" integer NOT NULL,
    "FoundAlbumsCount" integer NOT NULL,
    "FoundSongsCount" integer NOT NULL,
    "FoundOtherItems" integer NOT NULL,
    "SearchDurationInMs" double precision NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Settings" (
    "Id" integer NOT NULL,
    "Key" text NOT NULL,
    "Comment" text,
    "Category" integer,
    "Value" text NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "ShareActivities" (
    "Id" integer NOT NULL,
    "ShareId" integer NOT NULL,
    "UserId" integer,
    "CreatedAt" timestamptz NOT NULL,
    "ByUserAgent" text,
    "Client" text NOT NULL,
    "IpAddress" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Shares" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "ShareId" integer NOT NULL,
    "ShareType" integer NOT NULL,
    "ShareUniqueId" text NOT NULL,
    "ExpiresAt" timestamptz,
    "IsDownloadable" boolean NOT NULL,
    "LastVisitedAt" timestamptz,
    "VisitCount" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "SmartPlaylists" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "Name" text NOT NULL,
    "MqlQuery" text NOT NULL,
    "EntityType" text NOT NULL,
    "LastResultCount" integer NOT NULL,
    "LastEvaluatedAt" timestamptz,
    "IsPublic" boolean NOT NULL,
    "NormalizedQuery" text,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Songs" (
    "Id" integer NOT NULL,
    "AlbumId" integer NOT NULL,
    "Title" text NOT NULL,
    "TitleSort" text,
    "TitleNormalized" text NOT NULL,
    "Genres" text[],
    "Moods" text[],
    "Comment" text,
    "ReplayGain" double precision,
    "ReplayPeak" double precision,
    "ImageCount" integer,
    "SongNumber" integer NOT NULL,
    "FileName" text NOT NULL,
    "Lyrics" text,
    "FileSize" bigint NOT NULL,
    "FileHash" text NOT NULL,
    "PartTitles" text,
    "Duration" double precision NOT NULL,
    "SamplingRate" integer NOT NULL,
    "BitRate" integer NOT NULL,
    "BitDepth" integer NOT NULL,
    "BPM" integer NOT NULL,
    "ContentType" text NOT NULL,
    "ChannelCount" integer,
    "IsVbr" boolean NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    "AlternateNames" text,
    "LastPlayedAt" timestamptz,
    "LastMetaDataUpdatedAt" timestamptz,
    "PlayedCount" integer NOT NULL,
    "ItunesId" text,
    "AmgId" text,
    "DeezerId" integer,
    "DiscogsId" text,
    "WikiDataId" text,
    "MusicBrainzId" uuid,
    "LastFmId" text,
    "SpotifyId" text,
    "CalculatedRating" numeric NOT NULL,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserAlbums" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "AlbumId" integer NOT NULL,
    "PlayedCount" integer NOT NULL,
    "LastPlayedAt" timestamptz,
    "IsStarred" boolean NOT NULL,
    "IsHated" boolean NOT NULL,
    "StarredAt" timestamptz,
    "Rating" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserArtists" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "ArtistId" integer NOT NULL,
    "IsStarred" boolean NOT NULL,
    "IsHated" boolean NOT NULL,
    "StarredAt" timestamptz,
    "Rating" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserDeviceProfiles" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "PlayerId" integer,
    "IsDefaultProfile" boolean NOT NULL,
    "Name" text NOT NULL,
    "DirectPlay" boolean NOT NULL,
    "TargetCodec" text,
    "MaxBitrate" integer,
    "ResampleRate" integer,
    "Priority" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserEqualizerPresets" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "Name" text NOT NULL,
    "NameNormalized" text NOT NULL,
    "BandsJson" text NOT NULL,
    "IsDefault" boolean NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserGroupMembers" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "UserGroupId" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserGroups" (
    "Id" integer NOT NULL,
    "Name" text NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserPins" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "PinId" integer NOT NULL,
    "PinType" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserPlaybackSettings" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "CrossfadeDuration" double precision NOT NULL,
    "GaplessPlayback" boolean NOT NULL,
    "VolumeNormalization" boolean NOT NULL,
    "ReplayGain" text NOT NULL,
    "AudioQuality" text NOT NULL,
    "EqualizerPreset" text,
    "LastUsedDevice" text,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserPodcastEpisodePlayHistories" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "PodcastEpisodeId" integer NOT NULL,
    "PlayedAt" timestamptz NOT NULL,
    "Client" text NOT NULL,
    "ByUserAgent" text,
    "IpAddress" text,
    "SecondsPlayed" integer,
    "Source" smallint NOT NULL,
    "IsNowPlaying" boolean NOT NULL,
    "LastHeartbeatAt" timestamptz,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserSocialLogins" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "Provider" text NOT NULL,
    "Subject" text NOT NULL,
    "Email" text,
    "DisplayName" text,
    "HostedDomain" text,
    "LastLoginAt" timestamptz,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserSongPlayHistories" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "SongId" integer NOT NULL,
    "PlayedAt" timestamptz NOT NULL,
    "Client" text NOT NULL,
    "ByUserAgent" text,
    "IpAddress" text,
    "SecondsPlayed" integer,
    "Source" smallint NOT NULL,
    "IsNowPlaying" boolean NOT NULL,
    "LastHeartbeatAt" timestamptz,
    PRIMARY KEY ("Id")
);

CREATE TABLE "UserSongs" (
    "Id" integer NOT NULL,
    "UserId" integer NOT NULL,
    "SongId" integer NOT NULL,
    "PlayedCount" integer NOT NULL,
    "LastPlayedAt" timestamptz,
    "IsStarred" boolean NOT NULL,
    "IsHated" boolean NOT NULL,
    "StarredAt" timestamptz,
    "Rating" integer NOT NULL,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);

CREATE TABLE "Users" (
    "Id" integer NOT NULL,
    "UserName" text NOT NULL,
    "UserNameNormalized" text NOT NULL,
    "Email" text NOT NULL,
    "EmailNormalized" text NOT NULL,
    "EmailConfirmedDate" timestamptz,
    "PublicKey" text NOT NULL,
    "PasswordEncrypted" text NOT NULL,
    "PasswordHash" text,
    "PasswordHashAlgorithm" text,
    "OpenSubsonicSecretProtected" text,
    "LastLoginAt" timestamptz,
    "LastActivityAt" timestamptz,
    "IsAdmin" boolean NOT NULL,
    "IsEditor" boolean NOT NULL,
    "HasSettingsRole" boolean NOT NULL,
    "HasDownloadRole" boolean NOT NULL,
    "HasUploadRole" boolean NOT NULL,
    "HasPlaylistRole" boolean NOT NULL,
    "HasCoverArtRole" boolean NOT NULL,
    "HasCommentRole" boolean NOT NULL,
    "HasPodcastRole" boolean NOT NULL,
    "HasStreamRole" boolean NOT NULL,
    "HasJukeboxRole" boolean NOT NULL,
    "HasShareRole" boolean NOT NULL,
    "IsScrobblingEnabled" boolean NOT NULL,
    "LastFmSessionKey" text,
    "TimeZoneId" text NOT NULL,
    "PreferredLanguage" text,
    "PreferredTheme" text,
    "HatedGenres" text,
    "StarredGenres" text,
    "PasswordResetToken" text,
    "PasswordResetTokenExpiresAt" timestamptz,
    "IsLocked" boolean NOT NULL,
    "SortOrder" integer NOT NULL,
    "ApiKey" uuid NOT NULL,
    "CreatedAt" timestamptz NOT NULL,
    "LastUpdatedAt" timestamptz,
    "Tags" text,
    "Notes" text,
    "Description" text,
    PRIMARY KEY ("Id")
);
