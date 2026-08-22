//! Shared Jellyfin wire-model helpers (PRD-API-JF-001..005).
//!
//! Protocol-owned PascalCase DTOs are built here and stay inside the Jellyfin
//! adapter (ADR-0102); nothing is shared with native-v1 or OpenSubsonic.
//!
//! Frozen conventions ( capture, `design/evidence/phase-0/jellyfin-capture.md`): //!
//! - IDs are ApiKey GUIDs in N-format (32 lowercase hex, no dashes).
//! - Dates are `yyyy-MM-ddTHH:mm:ss.fffffffZ` (always 7 fractional digits).
//! - Null fields serialize as `null` (no null-stripping).
//! - List envelope: `{Items, TotalRecordCount, StartIndex}`.
//! - Genre ids are `md5("genre:" + name.ToUpperInvariant)` (verified against
//! goldens, including the Unicode `環境音楽` case).
//! - Item ETags use the exact legacy preimage `md5("{apiKey:N}-{updatedAt
//! UnixTicks}")`; collection ETags use the per-handler legacy preimages
//! (`collection-…`, `genres-…`, `musicgenres-…`).

use md5::{Digest, Md5};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

/// N-format (32 lowercase hex, no dashes) — the captured Jellyfin id form.
pub fn nf(id: Uuid) -> String {
    id.simple().to_string()
}

/// Parses an N-format or dashed GUID path/query id; anything else is unknown.
pub fn parse_id(raw: &str) -> Option<Uuid> {
    Uuid::parse_str(raw).ok()
}

/// Formats the captured `yyyy-MM-ddTHH:mm:ss.fffffffZ` date shape.
pub fn format_date(value: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:07}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.nanosecond() / 100
    )
}

/// Lowercase md5 hex (genre ids, ETags) — a frozen wire contract, not a
/// security primitive (mirrors the OpenSubsonic token note in Cargo.toml).
pub fn md5_hex(input: impl AsRef<[u8]>) -> String {
    let digest = Md5::digest(input.as_ref());
    let mut out = String::with_capacity(32);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Deterministic genre id: `md5hex("genre:" + name.ToUpperInvariant)`.
pub fn genre_public_id(name: &str) -> String {
    md5_hex(format!("genre:{}", name.to_uppercase()))
}

/// Item ETag — the exact legacy preimage: `md5("{apiKey:N}-{lastUpdated
/// in Unix ticks (100ns)}")` (`JellyfinControllerBase`-family `ComputeEtag`;
/// MD5 is a cache identifier here, not a security primitive). Matching the
/// preimage keeps ETags byte-identical to the legacy server at fixed data.
pub fn item_etag(public_id: Uuid, updated: OffsetDateTime) -> String {
    md5_hex(format!(
        "{}-{}",
        nf(public_id),
        updated.unix_timestamp_nanos() / 100
    ))
}

/// Milliseconds to Jellyfin `RunTimeTicks`. The frozen legacy baseline
/// multiplies the stored duration-ms value by 10^7 (treating it as seconds: /// 3000 ms → 30,000,000,000 ticks), not by the correct 10^4. The quirk is
/// captured contract (`compatibility/jellyfin/golden` RunTimeTicks values)
/// and is preserved until a product decision and ADR change it.
pub fn ticks_from_ms(ms: i64) -> i64 {
    ms.saturating_mul(10_000_000)
}

/// The captured UserData object (`Key` is the item N-format id).
pub fn user_data(
    play_count: i64,
    is_favorite: bool,
    key: &str,
    last_played: Option<OffsetDateTime>,
) -> Value {
    json!({
        "PlaybackPositionTicks": 0,
        "PlayCount": play_count,
        "IsFavorite": is_favorite,
        "Played": play_count > 0,
        "Key": key,
        "LastPlayedDate": last_played.map(format_date),
    })
}

/// The captured list envelope, in captured key order.
pub fn items_result(items: Vec<Value>, total: i64, start_index: i64) -> Value {
    json!({
        "Items": items,
        "TotalRecordCount": total,
        "StartIndex": start_index,
    })
}

/// Facts needed to render a MusicArtist BaseItem.
pub struct ArtistFacts {
    pub public_id: Uuid,
    pub name: String,
    pub sort_name: String,
    pub album_count: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Facts needed to render a MusicAlbum BaseItem.
pub struct AlbumFacts {
    pub public_id: Uuid,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    pub track_count: i64,
    pub duration_ms: i64,
    pub genres: Vec<String>,
    pub album_artists: Vec<(String, Uuid)>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    /// Populated on album detail; null on album lists (captured difference).
    pub user_data: Option<Value>,
}

/// Facts needed to render an Audio BaseItem.
pub struct SongFacts {
    pub public_id: Uuid,
    pub title: String,
    pub sort_title: String,
    /// Captured SongNumber (disc * 100 + track).
    pub song_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub container: Option<String>,
    pub byte_size: Option<i64>,
    pub bitrate_bps: Option<i64>,
    pub sample_rate_hz: Option<i32>,
    pub release_title: String,
    pub release_public_id: Uuid,
    pub track_artists: Vec<(String, Uuid)>,
    pub album_artists: Vec<(String, Uuid)>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub user_data: Option<Value>,
}

/// Which captured Audio serialization to emit. Album listings and song detail
/// embed full MediaSources (`SupportsTranscoding: true`, `Codec` = container);
/// playlist entries use the reduced shape (no UserData/MediaSources/Container,
/// `ParentIndexNumber` = 1-based playlist position).
pub enum SongShape {
    Full,
    PlaylistEntry { position: i32 },
}

fn name_id_json(entries: &[(String, Uuid)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(name, id)| json!({"Name": name, "Id": nf(*id)}))
            .collect(),
    )
}

fn names_json(entries: &[(String, Uuid)]) -> Value {
    Value::Array(entries.iter().map(|(name, _)| json!(name)).collect())
}

/// The list/detail-variant MediaSource (`SupportsTranscoding: true`).
fn media_source_list(song: &SongFacts) -> Value {
    let container = song.container.clone().unwrap_or_default();
    json!([{
        "Protocol": "File",
        "Id": nf(song.public_id),
        "Path": null,
        "Type": "Default",
        "Container": container,
        "Size": song.byte_size,
        "Name": song.title,
        "IsRemote": false,
        "ETag": null,
        "RunTimeTicks": song.duration_ms.map(ticks_from_ms),
        "ReadAtNativeFramerate": false,
        "IgnoreDts": false,
        "IgnoreIndex": false,
        "GenPtsInput": false,
        "SupportsTranscoding": true,
        "SupportsDirectStream": true,
        "SupportsDirectPlay": true,
        "IsInfiniteStream": false,
        "RequiresOpening": false,
        "RequiresClosing": false,
        "RequiresLooping": false,
        "SupportsProbing": true,
        "MediaStreams": [{
            "Codec": container,
            "TimeBase": null,
            "CodecTimeBase": null,
            "Title": null,
            "DisplayTitle": null,
            "DisplayLanguage": null,
            "IsInterlaced": false,
            "IsAVC": null,
            "ChannelLayout": null,
            "BitRate": song.bitrate_bps,
            "BitDepth": null,
            "RefFrames": null,
            "IsDefault": true,
            "IsForced": false,
            "Type": "Audio",
            "Index": 0,
            "IsExternal": false,
            "IsTextSubtitleStream": false,
            "SupportsExternalStream": false,
            "Level": 0,
            "Channels": null,
            "SampleRate": song.sample_rate_hz,
            "IsAnamorphic": null
        }],
        "Bitrate": song.bitrate_bps,
        "DefaultAudioStreamIndex": null
    }])
}

/// MusicArtist BaseItem (53 keys, captured order).
pub fn artist_item(server_id: &str, facts: &ArtistFacts) -> Value {
    json!({
        "Name": facts.name,
        "ServerId": server_id,
        "Id": nf(facts.public_id),
        "Etag": item_etag(facts.public_id, facts.updated_at),
        "DateCreated": format_date(facts.created_at),
        "CanDelete": false,
        "CanDownload": true,
        "SortName": facts.sort_name,
        "ExternalUrls": null,
        "Path": null,
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Overview": null,
        "Taglines": null,
        "Genres": null,
        "CommunityRating": null,
        "RunTimeTicks": null,
        "PlayAccess": "Full",
        "ProductionYear": null,
        "IndexNumber": null,
        "ParentIndexNumber": null,
        "RemoteTrailers": null,
        "ProviderIds": null,
        "IsFolder": true,
        "ParentId": null,
        "Type": "MusicArtist",
        "Studios": null,
        "GenreItems": null,
        "LocalTrailerCount": 0,
        "UserData": null,
        "ChildCount": facts.album_count,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": null,
        "Tags": null,
        "PrimaryImageAspectRatio": null,
        "Artists": null,
        "ArtistItems": null,
        "Album": null,
        "AlbumId": null,
        "AlbumPrimaryImageTag": null,
        "AlbumArtist": null,
        "AlbumArtists": null,
        "ImageTags": {},
        "BackdropImageTags": [],
        "ScreenshotImageTags": null,
        "ImageBlurHashes": null,
        "LocationType": "FileSystem",
        "LockedFields": null,
        "LockData": false,
        "MediaSources": null,
        "Container": null,
        "MediaType": "Audio",
        "HasLyrics": false
    })
}

/// MusicAlbum BaseItem (53 keys, captured order). `UserData` is null on album
/// lists and populated on album detail (captured difference).
pub fn album_item(server_id: &str, facts: &AlbumFacts) -> Value {
    let album_artist_name = facts.album_artists.first().map(|(name, _)| name.clone());
    json!({
        "Name": facts.title,
        "ServerId": server_id,
        "Id": nf(facts.public_id),
        "Etag": item_etag(facts.public_id, facts.updated_at),
        "DateCreated": format_date(facts.created_at),
        "CanDelete": false,
        "CanDownload": true,
        "SortName": facts.sort_title,
        "ExternalUrls": null,
        "Path": null,
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Overview": null,
        "Taglines": null,
        "Genres": facts.genres,
        "CommunityRating": null,
        "RunTimeTicks": ticks_from_ms(facts.duration_ms),
        "PlayAccess": "Full",
        "ProductionYear": facts.year,
        "IndexNumber": null,
        "ParentIndexNumber": null,
        "RemoteTrailers": null,
        "ProviderIds": null,
        "IsFolder": true,
        "ParentId": null,
        "Type": "MusicAlbum",
        "Studios": null,
        "GenreItems": null,
        "LocalTrailerCount": 0,
        "UserData": facts.user_data,
        "ChildCount": facts.track_count,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": null,
        "Tags": null,
        "PrimaryImageAspectRatio": null,
        "Artists": null,
        "ArtistItems": null,
        "Album": null,
        "AlbumId": null,
        "AlbumPrimaryImageTag": null,
        "AlbumArtist": album_artist_name,
        "AlbumArtists": name_id_json(&facts.album_artists),
        "ImageTags": {},
        "BackdropImageTags": [],
        "ScreenshotImageTags": null,
        "ImageBlurHashes": null,
        "LocationType": "FileSystem",
        "LockedFields": null,
        "LockData": false,
        "MediaSources": null,
        "Container": null,
        "MediaType": "Audio",
        "HasLyrics": false
    })
}

/// Audio BaseItem (53 keys, captured order) in the full or playlist-entry
/// shape (see [`SongShape`]).
pub fn song_item(server_id: &str, facts: &SongFacts, shape: SongShape) -> Value {
    let (user_data, media_sources, container, parent_index, album_artist, album_artists) =
        match shape {
            SongShape::Full => (
                facts.user_data.clone(),
                media_source_list(facts),
                facts.container.clone().map_or(Value::Null, Value::from),
                Value::Null,
                facts
                    .album_artists
                    .first()
                    .map_or(Value::Null, |(name, _)| json!(name)),
                name_id_json(&facts.album_artists),
            ),
            // PlaylistEntry passes facts.user_data through: it is None unless
            // the caller hydrated it (captured: enableUserData=true only).
            SongShape::PlaylistEntry { position } => (
                facts.user_data.clone(),
                Value::Null,
                Value::Null,
                json!(position),
                Value::Null,
                Value::Null,
            ),
        };
    json!({
        "Name": facts.title,
        "ServerId": server_id,
        "Id": nf(facts.public_id),
        "Etag": item_etag(facts.public_id, facts.updated_at),
        "DateCreated": format_date(facts.created_at),
        "CanDelete": false,
        "CanDownload": true,
        "SortName": facts.sort_title,
        "ExternalUrls": null,
        "Path": null,
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Overview": null,
        "Taglines": null,
        "Genres": null,
        "CommunityRating": null,
        "RunTimeTicks": facts.duration_ms.map(ticks_from_ms),
        "PlayAccess": "Full",
        "ProductionYear": null,
        "IndexNumber": facts.song_number,
        "ParentIndexNumber": parent_index,
        "RemoteTrailers": null,
        "ProviderIds": null,
        "IsFolder": false,
        "ParentId": null,
        "Type": "Audio",
        "Studios": null,
        "GenreItems": null,
        "LocalTrailerCount": 0,
        "UserData": user_data,
        "ChildCount": null,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": null,
        "Tags": null,
        "PrimaryImageAspectRatio": null,
        "Artists": names_json(&facts.track_artists),
        "ArtistItems": name_id_json(&facts.track_artists),
        "Album": facts.release_title,
        "AlbumId": nf(facts.release_public_id),
        "AlbumPrimaryImageTag": null,
        "AlbumArtist": album_artist,
        "AlbumArtists": album_artists,
        "ImageTags": {},
        "BackdropImageTags": [],
        "ScreenshotImageTags": null,
        "ImageBlurHashes": null,
        "LocationType": "FileSystem",
        "LockedFields": null,
        "LockData": false,
        "MediaSources": media_sources,
        "Container": container,
        "MediaType": "Audio",
        "HasLyrics": false
    })
}

/// MusicGenre BaseItem. `Etag` is null and `DateCreated` is synthesized per
/// request (captured, masked as volatile).
pub fn genre_item(server_id: &str, name: &str, now: OffsetDateTime) -> Value {
    json!({
        "Name": name,
        "ServerId": server_id,
        "Id": genre_public_id(name),
        "Etag": null,
        "DateCreated": format_date(now),
        "CanDelete": false,
        "CanDownload": false,
        "SortName": name,
        "ExternalUrls": null,
        "Path": null,
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Overview": null,
        "Taglines": null,
        "Genres": null,
        "CommunityRating": null,
        "RunTimeTicks": null,
        "PlayAccess": "Full",
        "ProductionYear": null,
        "IndexNumber": null,
        "ParentIndexNumber": null,
        "RemoteTrailers": null,
        "ProviderIds": null,
        "IsFolder": true,
        "ParentId": null,
        "Type": "MusicGenre",
        "Studios": null,
        "GenreItems": null,
        "LocalTrailerCount": 0,
        "UserData": null,
        "ChildCount": null,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": null,
        "Tags": null,
        "PrimaryImageAspectRatio": null,
        "Artists": null,
        "ArtistItems": null,
        "Album": null,
        "AlbumId": null,
        "AlbumPrimaryImageTag": null,
        "AlbumArtist": null,
        "AlbumArtists": null,
        "ImageTags": {},
        "BackdropImageTags": [],
        "ScreenshotImageTags": null,
        "ImageBlurHashes": null,
        "LocationType": "FileSystem",
        "LockedFields": null,
        "LockData": false,
        "MediaSources": null,
        "Container": null,
        "MediaType": "Audio",
        "HasLyrics": false
    })
}

/// Facts needed to render a Playlist BaseItem.
pub struct PlaylistFacts {
    pub public_id: Uuid,
    pub name: String,
    pub comment: Option<String>,
    pub song_count: i64,
    pub duration_ms: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Playlist BaseItem (captured shape: `Overview` is the playlist comment).
pub fn playlist_item(server_id: &str, facts: &PlaylistFacts) -> Value {
    json!({
        "Name": facts.name,
        "ServerId": server_id,
        "Id": nf(facts.public_id),
        "Etag": item_etag(facts.public_id, facts.updated_at),
        "DateCreated": format_date(facts.created_at),
        "CanDelete": false,
        "CanDownload": true,
        "SortName": facts.name,
        "ExternalUrls": null,
        "Path": null,
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Overview": facts.comment,
        "Taglines": null,
        "Genres": null,
        "CommunityRating": null,
        "RunTimeTicks": ticks_from_ms(facts.duration_ms),
        "PlayAccess": "Full",
        "ProductionYear": null,
        "IndexNumber": null,
        "ParentIndexNumber": null,
        "RemoteTrailers": null,
        "ProviderIds": null,
        "IsFolder": true,
        "ParentId": null,
        "Type": "Playlist",
        "Studios": null,
        "GenreItems": null,
        "LocalTrailerCount": 0,
        "UserData": null,
        "ChildCount": facts.song_count,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": null,
        "Tags": null,
        "PrimaryImageAspectRatio": null,
        "Artists": null,
        "ArtistItems": null,
        "Album": null,
        "AlbumId": null,
        "AlbumPrimaryImageTag": null,
        "AlbumArtist": null,
        "AlbumArtists": null,
        "ImageTags": {},
        "BackdropImageTags": [],
        "ScreenshotImageTags": null,
        "ImageBlurHashes": null,
        "LocationType": "FileSystem",
        "LockedFields": null,
        "LockData": false,
        "MediaSources": null,
        "Container": null,
        "MediaType": "Audio",
        "HasLyrics": false
    })
}

/// UserView BaseItem (reduced key set, `CollectionType: "music"`,
/// `LocationType: "Virtual"`, populated UserData).
pub fn user_view_item(server_id: &str, id: Uuid, name: &str, created_at: OffsetDateTime) -> Value {
    json!({
        "Name": name,
        "ServerId": server_id,
        "Id": nf(id),
        "Etag": item_etag(id, created_at),
        "DateCreated": format_date(created_at),
        "CanDelete": false,
        "CanDownload": true,
        "SortName": name,
        "ExternalUrls": null,
        "Path": null,
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Taglines": null,
        "Genres": null,
        "PlayAccess": "Full",
        "RemoteTrailers": null,
        "ProviderIds": null,
        "IsFolder": true,
        "ParentId": null,
        "Type": "UserView",
        "Studios": null,
        "GenreItems": null,
        "LocalTrailerCount": 0,
        "UserData": user_data(0, false, &nf(id), None),
        "ChildCount": 0,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": null,
        "Tags": null,
        "PrimaryImageAspectRatio": null,
        "CollectionType": "music",
        "ImageTags": {},
        "BackdropImageTags": [],
        "ScreenshotImageTags": null,
        "ImageBlurHashes": null,
        "LocationType": "Virtual",
        "LockedFields": null,
        "LockData": false
    })
}

/// Artists/AlbumArtists collection ETag — legacy preimage
/// `collection-{total}-{skip}-{take}-{latestUpdateTicks}` (single shared
/// handler, so both routes produce identical etags).
pub fn artists_collection_etag(total: i64, skip: i64, take: i64, latest: OffsetDateTime) -> String {
    md5_hex(format!(
        "collection-{total}-{skip}-{take}-{}",
        latest.unix_timestamp_nanos() / 100
    ))
}

/// Genres/MusicGenres collection ETag — legacy preimage
/// `{handler}-{total}-{skip}-{take}-{nowTicks}`. Time-based (volatile) exactly
/// as captured; the handler prefix differs (`genres` vs `musicgenres`).
pub fn genres_collection_etag(
    handler: &str,
    total: i64,
    skip: i64,
    take: i64,
    now: OffsetDateTime,
) -> String {
    md5_hex(format!(
        "{handler}-{total}-{skip}-{take}-{}",
        now.unix_timestamp_nanos() / 100
    ))
}

/// Reduced InstantMix Audio item (Items/InstantMix and Songs/InstantMix
/// build items inline: no Etag/UserData/MediaSources/Container, `ImageTags`
/// carries `{"Primary": albumId}`; all other record keys serialize as null).
#[allow(clippy::too_many_arguments)]
pub fn mix_item(
    server_id: &str,
    public_id: Uuid,
    title: &str,
    can_download: bool,
    song_number: Option<i32>,
    duration_ms: Option<i64>,
    album_title: &str,
    album_public_id: Uuid,
    album_year: Option<i32>,
    artist: Option<(String, Uuid)>,
    genres: Option<Vec<String>>,
    created_at: OffsetDateTime,
) -> Value {
    let (artist_name, artist_id) = match artist {
        Some((name, id)) => (Some(name), Some(id)),
        None => (None, None),
    };
    let artist_items = match (&artist_name, artist_id) {
        (Some(name), Some(id)) => json!([{"Name": name, "Id": nf(id)}]),
        _ => json!([]),
    };
    let artist_names = match &artist_name {
        Some(name) => json!([name]),
        None => json!([]),
    };
    let album_artists = artist_items.clone();
    json!({
        "Name": title,
        "ServerId": server_id,
        "Id": nf(public_id),
        "Etag": null,
        "DateCreated": format_date(created_at),
        "CanDelete": false,
        "CanDownload": can_download,
        "SortName": title,
        "ExternalUrls": null,
        "Path": null,
        "EnableMediaSourceDisplay": true,
        "ChannelId": null,
        "Overview": null,
        "Taglines": null,
        "Genres": genres,
        "CommunityRating": null,
        "RunTimeTicks": duration_ms.map_or(0, ticks_from_ms),
        "PlayAccess": "Full",
        "ProductionYear": album_year,
        "IndexNumber": song_number,
        "ParentIndexNumber": null,
        "RemoteTrailers": null,
        "ProviderIds": null,
        "IsFolder": false,
        "ParentId": null,
        "Type": "Audio",
        "Studios": null,
        "GenreItems": null,
        "LocalTrailerCount": 0,
        "UserData": null,
        "ChildCount": null,
        "SpecialFeatureCount": 0,
        "DisplayPreferencesId": null,
        "Tags": null,
        "PrimaryImageAspectRatio": null,
        "Artists": artist_names,
        "ArtistItems": artist_items,
        "Album": album_title,
        "AlbumId": nf(album_public_id),
        "AlbumPrimaryImageTag": null,
        "AlbumArtist": artist_name,
        "AlbumArtists": album_artists,
        "ImageTags": {"Primary": nf(album_public_id)},
        "BackdropImageTags": [],
        "ScreenshotImageTags": null,
        "ImageBlurHashes": null,
        "LocationType": "FileSystem",
        "LockedFields": null,
        "LockData": false,
        "MediaSources": null,
        "Container": null,
        "MediaType": "Audio",
        "HasLyrics": false
    })
}

/// The PlaybackInfo-variant MediaSource (`SupportsTranscoding: false`, codec
/// mapped from the container, computed DisplayTitle).
pub fn playback_info_media_source(song: &SongFacts) -> Value {
    let container = song.container.clone().unwrap_or_else(|| "mp3".to_string());
    let container_lower = container.to_ascii_lowercase();
    let codec = match container_lower.as_str() {
        "mp3" => "mp3",
        "flac" => "flac",
        "m4a" | "aac" => "aac",
        "ogg" => "vorbis",
        "opus" => "opus",
        "wav" => "pcm",
        "wma" => "wma",
        other => other,
    };
    let channels = song_media_channels(song);
    let display_title = match channels {
        Some(2) => format!("{} - Stereo", codec.to_uppercase()),
        Some(1) => format!("{} - Mono", codec.to_uppercase()),
        Some(count) => format!("{} - {count} channels", codec.to_uppercase()),
        // Legacy interpolation of a null ChannelCount renders empty.
        None => format!("{} -  channels", codec.to_uppercase()),
    };
    json!({
        "Protocol": "File",
        "Id": nf(song.public_id),
        "Path": null,
        "Type": "Default",
        "Container": container,
        "Size": song.byte_size,
        "Name": song.title,
        "IsRemote": false,
        "ETag": null,
        "RunTimeTicks": song.duration_ms.filter(|d| *d > 0).map(ticks_from_ms),
        "ReadAtNativeFramerate": false,
        "IgnoreDts": false,
        "IgnoreIndex": false,
        "GenPtsInput": false,
        "SupportsTranscoding": false,
        "SupportsDirectStream": true,
        "SupportsDirectPlay": true,
        "IsInfiniteStream": false,
        "RequiresOpening": false,
        "RequiresClosing": false,
        "RequiresLooping": false,
        "SupportsProbing": true,
        "MediaStreams": [{
            "Codec": codec,
            "TimeBase": null,
            "CodecTimeBase": null,
            "Title": null,
            "DisplayTitle": display_title,
            "DisplayLanguage": null,
            "IsInterlaced": false,
            "IsAVC": null,
            "ChannelLayout": null,
            "BitRate": song.bitrate_bps,
            "BitDepth": null,
            "RefFrames": null,
            "IsDefault": true,
            "IsForced": false,
            "Type": "Audio",
            "Index": 0,
            "IsExternal": false,
            "IsTextSubtitleStream": false,
            "SupportsExternalStream": false,
            "Level": 0,
            "Channels": channels,
            "SampleRate": song.sample_rate_hz,
            "IsAnamorphic": null
        }],
        "Bitrate": song.bitrate_bps,
        "DefaultAudioStreamIndex": null
    })
}

fn song_media_channels(song: &SongFacts) -> Option<i32> {
    // SongFacts carries no channels today; the PlaybackInfo builder leaves it
    // None so the captured empty-channel DisplayTitle form is reproduced.
    let _ = song;
    None
}

/// The full JellyfinUser object (record order; `Configuration` and `Policy`
/// carry the captured legacy defaults with role-derived policy values).
#[allow(clippy::too_many_arguments)]
pub fn user_json(
    server_id: &str,
    public_id: Uuid,
    username: &str,
    is_admin: bool,
    is_disabled: bool,
    has_stream_role: bool,
    has_download_role: bool,
    last_activity: OffsetDateTime,
) -> Value {
    json!({
        "Name": username,
        "ServerId": server_id,
        "Id": nf(public_id),
        "HasPassword": true,
        "HasConfiguredPassword": true,
        "HasConfiguredEasyPassword": false,
        "EnableAutoLogin": false,
        "LastLoginDate": format_date(last_activity),
        "LastActivityDate": format_date(last_activity),
        "Configuration": {
            "PlayDefaultAudioTrack": true,
            "SubtitleLanguagePreference": null,
            "DisplayMissingEpisodes": false,
            "GroupedFolders": null,
            "SubtitleMode": "Default",
            "DisplayCollectionsView": false,
            "EnableLocalPassword": false,
            "OrderedViews": null,
            "LatestItemsExcludes": null,
            "MyMediaExcludes": null,
            "HidePlayedInLatest": true,
            "RememberAudioSelections": true,
            "RememberSubtitleSelections": true,
            "EnableNextEpisodeAutoPlay": true
        },
        "Policy": {
            "IsAdministrator": is_admin,
            "IsHidden": false,
            "IsDisabled": is_disabled,
            "EnableUserPreferenceAccess": true,
            "EnableRemoteControlOfOtherUsers": false,
            "EnableSharedDeviceControl": true,
            "EnableRemoteAccess": true,
            "EnableLiveTvManagement": false,
            "EnableLiveTvAccess": false,
            "EnableMediaPlayback": has_stream_role,
            "EnableAudioPlaybackTranscoding": true,
            "EnableVideoPlaybackTranscoding": false,
            "EnablePlaybackRemuxing": true,
            "EnableContentDeletion": false,
            "EnableContentDownloading": has_download_role,
            "EnableSyncTranscoding": true,
            "EnableMediaConversion": false,
            "EnableAllChannels": true,
            "EnableAllFolders": true,
            "InvalidLoginAttemptCount": 0,
            "EnablePublicSharing": true,
            "RemoteClientBitrateLimit": 0,
            "AuthenticationProviderId": "Melodee.Blazor.Auth.DefaultAuthenticationProvider",
            "PasswordResetProviderId": "Melodee.Blazor.Auth.DefaultPasswordResetProvider"
        }
    })
}

/// SessionInfo in the captured legacy record order (masked in goldens; the
/// field set matches `JellyfinSessionInfo` declaration order).
#[allow(clippy::too_many_arguments)]
pub fn session_info_json(
    server_id: &str,
    session_id: &str,
    user_public_id: Uuid,
    username: &str,
    client: Option<&str>,
    device_name: Option<&str>,
    device_id: Option<&str>,
    app_version: Option<&str>,
    now: OffsetDateTime,
) -> Value {
    json!({
        "PlayState": null,
        "Capabilities": null,
        "RemoteEndPoint": null,
        "PlayableMediaTypes": ["Audio"],
        "Id": session_id,
        "UserId": nf(user_public_id),
        "UserName": username,
        "Client": client,
        "LastActivityDate": format_date(now),
        "LastPlaybackCheckIn": null,
        "DeviceName": device_name,
        "DeviceId": device_id,
        "ApplicationVersion": app_version,
        "IsActive": true,
        "SupportsMediaControl": false,
        "SupportsRemoteControl": false,
        "NowPlayingQueue": null,
        "NowPlayingQueueFullItems": null,
        "NowPlayingItem": null,
        "HasCustomDeviceName": false,
        "ServerId": server_id,
        "SupportedCommands": []
    })
}

/// One GET /Sessions entry synthesized from now-playing state (captured
/// handler shape; the frozen fixture golden is `[]` because no now-playing
/// entries existed at capture time).
#[allow(clippy::too_many_arguments)]
pub fn session_entry_json(
    session_id: &str,
    user_wire_id: &str,
    username: &str,
    client: &str,
    device_id: Option<&str>,
    device_name: Option<&str>,
    track_public_id: Uuid,
    track_title: &str,
    duration_ms: Option<i64>,
    last_activity: OffsetDateTime,
) -> Value {
    json!({
        "PlayState": {
            "PositionTicks": 0,
            "IsPaused": false,
            "IsMuted": false,
            "CanSeek": true,
            "PlayMethod": "DirectPlay",
            "RepeatMode": "RepeatNone"
        },
        "Capabilities": null,
        "RemoteEndPoint": null,
        "PlayableMediaTypes": null,
        "Id": session_id,
        "UserId": user_wire_id,
        "UserName": username,
        "Client": client,
        "LastActivityDate": format_date(last_activity),
        "LastPlaybackCheckIn": null,
        "DeviceName": device_name.unwrap_or("Unknown Device"),
        "DeviceId": device_id.unwrap_or(""),
        "ApplicationVersion": "1.0.0",
        "IsActive": true,
        "SupportsMediaControl": false,
        "SupportsRemoteControl": false,
        "NowPlayingQueue": null,
        "NowPlayingQueueFullItems": null,
        "NowPlayingItem": {
            "Id": nf(track_public_id),
            "Name": track_title,
            "Type": "Audio",
            "MediaType": "Audio",
            "RunTimeTicks": duration_ms.map(ticks_from_ms)
        },
        "HasCustomDeviceName": false,
        "ServerId": null,
        "SupportedCommands": null
    })
}
