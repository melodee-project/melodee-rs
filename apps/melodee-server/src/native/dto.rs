//! Wire-only DTOs for the frozen Melodee API v1 contract.
//!
//! `Album` and `Song` are deliberate compatibility vocabulary at this
//! adapter boundary (ADR-0101). Core domain and persistence code continue to
//! use `Release` and `Track`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Frozen native-v1 paging metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginationMetadata {
    pub total_count: i32,
    pub page_size: i16,
    pub current_page: i32,
    pub total_pages: i32,
    pub has_previous: bool,
    pub has_next: bool,
}

impl PaginationMetadata {
    pub fn new(total_count: i64, page: u32, page_size: u16) -> Result<Self, &'static str> {
        let page = page.max(1);
        let page_size = page_size.max(1);
        let total_pages = if total_count == 0 {
            0
        } else {
            (total_count + i64::from(page_size) - 1) / i64::from(page_size)
        };
        Ok(Self {
            total_count: i32::try_from(total_count).map_err(|_| "total count exceeds v1")?,
            page_size: i16::try_from(page_size).map_err(|_| "page size exceeds v1")?,
            current_page: i32::try_from(page).map_err(|_| "page exceeds v1")?,
            total_pages: i32::try_from(total_pages).map_err(|_| "page count exceeds v1")?,
            has_previous: page > 1,
            has_next: i64::from(page) < total_pages,
        })
    }
}

/// Frozen native-v1 `{ meta, data }` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PagedResponse<T> {
    pub meta: PaginationMetadata,
    pub data: Vec<T>,
}

/// Native-v1 Artist DTO. Optional fields serialize as `null`, matching the
/// frozen contract rather than being omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDto {
    pub id: Uuid,
    pub thumbnail_url: String,
    pub image_url: String,
    pub name: String,
    pub user_starred: bool,
    pub user_rating: i32,
    pub album_count: i32,
    pub song_count: i32,
    pub created_at: String,
    pub updated_at: String,
    pub biography: Option<String>,
    pub genres: Option<Vec<String>>,
}

/// Native-v1 Album DTO backed internally by a Release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDto {
    pub id: Uuid,
    pub artist: ArtistDto,
    pub thumbnail_url: String,
    pub image_url: String,
    pub name: String,
    pub release_year: i32,
    pub user_starred: bool,
    pub user_rating: i32,
    pub song_count: i32,
    pub duration_ms: i64,
    pub duration_formatted: String,
    pub created_at: String,
    pub updated_at: String,
    pub description: Option<String>,
    pub genre: Option<String>,
}

/// Native-v1 Song DTO backed internally by a Track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongDto {
    pub id: Uuid,
    pub artist: ArtistDto,
    pub album: AlbumDto,
    pub stream_url: String,
    pub thumbnail_url: String,
    pub image_url: String,
    pub title: String,
    pub duration_ms: i64,
    pub duration_formatted: String,
    pub user_starred: bool,
    pub user_rating: i32,
    pub song_number: i32,
    pub bitrate: i32,
    pub play_count: i32,
    pub created_at: String,
    pub updated_at: String,
    pub genre: Option<String>,
}

/// Captured combined-search request. Legacy clients omit nullable/defaulted
/// members despite the generated OpenAPI marking every property required.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    pub r#type: Option<String>,
    pub album_page: Option<i16>,
    pub artist_page: Option<i16>,
    pub song_page: Option<i16>,
    pub page_size: Option<i16>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub filter_by_artist_id: Option<Uuid>,
}

/// Native-v1 combined search payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultDto {
    pub total_count: i32,
    pub artists: Vec<ArtistDto>,
    pub total_artists: i32,
    pub albums: Vec<AlbumDto>,
    pub total_albums: i32,
    pub songs: Vec<SongDto>,
    pub total_songs: i32,
    pub playlists: Vec<PlaylistDto>,
    pub total_playlists: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResultResponse {
    pub meta: PaginationMetadata,
    pub data: SearchResultDto,
}

/// Native-v1 User DTO embedded by playlists and `/user/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: Uuid,
    pub thumbnail_url: String,
    pub image_url: String,
    pub username: String,
    pub email: String,
    pub is_admin: bool,
    pub is_editor: bool,
    pub roles: Vec<String>,
    pub songs_played: i32,
    pub artists_liked: i32,
    pub artists_disliked: i32,
    pub albums_liked: i32,
    pub albums_disliked: i32,
    pub songs_liked: i32,
    pub songs_disliked: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// Native-v1 non-secret administrator view of one User.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserInfoDto {
    pub id: Uuid,
    pub username: String,
    pub email: Option<String>,
    pub is_admin: bool,
    pub is_enabled: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

/// Native-v1 Playlist DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDto {
    pub id: Uuid,
    pub thumbnail_url: String,
    pub image_url: String,
    pub name: String,
    pub description: String,
    pub duration_ms: i64,
    pub duration_formatted: String,
    pub song_count: i16,
    pub is_public: bool,
    pub owner: UserDto,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreDto {
    pub id: String,
    pub name: String,
    pub song_count: i32,
    pub album_count: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreListResponse {
    pub genres: Vec<GenreDto>,
    pub total_count: i32,
    pub page: i32,
    pub limit: i32,
}

/// Native-v1 response for one genre's bounded songs page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreSongsResponse {
    pub songs: Vec<SongDto>,
    pub total_count: i32,
    pub page: i32,
    pub limit: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLineDto {
    pub text: String,
    pub start_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsDto {
    pub song_id: Uuid,
    pub language: String,
    pub is_synced: bool,
    pub plain_text: Option<String>,
    pub lines: Option<Vec<LyricsLineDto>>,
    pub display_artist: Option<String>,
    pub display_title: Option<String>,
    pub offset: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSuggestionDto {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub thumbnail_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchSuggestResponse {
    pub artists: Vec<SearchSuggestionDto>,
    pub albums: Vec<SearchSuggestionDto>,
    pub songs: Vec<SearchSuggestionDto>,
    pub playlists: Vec<SearchSuggestionDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NumericRange<T> {
    pub min: Option<T>,
    pub max: Option<T>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdvancedSearchFilters {
    pub year: Option<NumericRange<i32>>,
    pub bpm: Option<NumericRange<f64>>,
    pub duration: Option<NumericRange<f64>>,
    pub genre: Option<Vec<String>>,
    pub mood: Option<Vec<String>>,
    pub key: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSearchRequest {
    pub query: Option<String>,
    pub filters: Option<AdvancedSearchFilters>,
    pub types: Option<Vec<String>>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub page: Option<i32>,
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvancedSearchResults {
    pub songs: Vec<SongDto>,
    pub albums: Vec<AlbumDto>,
    pub artists: Vec<ArtistDto>,
    pub playlists: Vec<PlaylistDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvancedSearchResponse {
    pub results: AdvancedSearchResults,
    pub meta: PaginationMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarItemDto {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub similarity_score: f64,
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimilarResponse {
    pub similar: Vec<SimilarItemDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserStatsResponse {
    pub period_days: i32,
    pub total_plays: i32,
    pub favorite_songs: i32,
    pub favorite_albums: i32,
    pub favorite_artists: i32,
    pub rated_songs: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopItemResponse {
    pub name: String,
    pub play_count: i32,
    pub song_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopItemsResponse {
    pub period_days: i32,
    pub data: Vec<TopItemResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSeriesDataPointDto {
    pub date: String,
    pub plays: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSeriesResponse {
    pub period_days: i32,
    pub data: Vec<TimeSeriesDataPointDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItemDto {
    pub song_id: Option<Uuid>,
    pub name: String,
    pub played_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryResponse {
    pub data: Vec<HistoryItemDto>,
}

/// The native API compatibility version is independent of the Rust package
/// version. Existing clients gate login on this captured value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfoDto {
    pub name: &'static str,
    pub description: &'static str,
    pub major_version: i32,
    pub minor_version: i32,
    pub patch_version: i32,
    pub version: &'static str,
}

impl Default for ServerInfoDto {
    fn default() -> Self {
        Self {
            name: "Melodee",
            description: "Melodee API",
            major_version: 2,
            minor_version: 2,
            patch_version: 0,
            version: "2.2.0",
        }
    }
}

#[must_use]
pub fn duration_formatted(duration_ms: i64) -> String {
    let seconds = duration_ms.max(0) / 1_000;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    format!("{hours}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn paging_is_one_based_bounded_and_matches_frozen_flags() {
        let meta = PaginationMetadata::new(10, 2, 5).expect("bounded paging");
        assert_eq!(meta.current_page, 2);
        assert_eq!(meta.total_pages, 2);
        assert!(meta.has_previous);
        assert!(!meta.has_next);

        let empty = PaginationMetadata::new(0, 1, 50).expect("empty paging");
        assert_eq!(empty.total_pages, 0);
        assert!(!empty.has_next);
    }

    #[test]
    fn native_system_info_remains_at_the_captured_client_gate_version() {
        assert_eq!(
            serde_json::to_value(ServerInfoDto::default()).expect("serialize"),
            serde_json::json!({
                "name": "Melodee",
                "description": "Melodee API",
                "majorVersion": 2,
                "minorVersion": 2,
                "patchVersion": 0,
                "version": "2.2.0"
            })
        );
    }

    #[test]
    fn duration_format_matches_the_captured_whole_second_behavior() {
        assert_eq!(duration_formatted(2_900), "0:00:02");
        assert_eq!(duration_formatted(3_661_999), "1:01:01");
    }
}
