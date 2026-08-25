//! Native-v1 user annotations, playlists, and scrobble persistence.
//!
//! Wire DTOs remain in the native adapter. This module accepts internal User
//! IDs and public catalog IDs so authorization never depends on display text.

use std::collections::{HashMap, HashSet, VecDeque};

use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Maximum playlist items accepted by one mutation.
pub const MAX_NATIVE_PLAYLIST_ITEMS: usize = 10_000;
/// Maximum number of playlist rows returned by one native request.
pub const MAX_NATIVE_PLAYLIST_PAGE_SIZE: u16 = 500;

/// User-state mutations shared by Artist, Release, and Track adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeUserStateMutation {
    /// Set favorite/star state.
    Starred(bool),
    /// Set disliked/hated state.
    Hated(bool),
    /// Set the captured zero-through-five rating; zero clears it.
    Rating(i16),
    /// Set pinned state.
    Pinned(bool),
}

/// Stored per-user interaction state.
#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct NativeUserState {
    /// Whether the entity is starred.
    pub starred: bool,
    /// Whether the entity is hated.
    pub hated: bool,
    /// Zero-through-five rating.
    pub rating: i16,
    /// Whether the entity is pinned.
    pub pinned: bool,
    /// Successful completed-play count.
    pub play_count: i64,
    /// Most recent completed play.
    pub last_played_at: Option<OffsetDateTime>,
}

/// Stored per-user preference for a native-v1 genre compatibility value.
#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct NativeGenrePreference {
    /// Exact genre display value observed in the catalog.
    pub genre_name: String,
    /// Whether the genre is starred.
    pub starred: bool,
    /// Whether the genre is hated.
    pub hated: bool,
}

/// Playlist creation facts.
#[derive(Clone, Debug)]
pub struct NativePlaylistCreate {
    /// Existing public ID during migration, or `None` for a new UUIDv7 ID.
    pub public_api_id: Option<Uuid>,
    /// Nonblank display name.
    pub name: String,
    /// Optional description/comment.
    pub description: Option<String>,
    /// Public visibility.
    pub is_public: bool,
    /// Ordered Track public IDs; duplicates are preserved.
    pub track_public_ids: Vec<Uuid>,
}

/// Stored playlist summary.
#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct NativePlaylist {
    /// Internal immutable UUIDv7 identifier.
    pub id: Uuid,
    /// Wire-compatible immutable public ID.
    pub public_api_id: Uuid,
    /// Owning internal User ID.
    pub owner_user_id: Uuid,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Public visibility.
    pub is_public: bool,
    /// Monotonic mutation revision.
    pub revision: i64,
    /// Creation time.
    pub created_at: OffsetDateTime,
    /// Last mutation time.
    pub updated_at: OffsetDateTime,
}

/// Playlist item resolved to the Track public ID used by wire adapters.
#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct NativePlaylistItem {
    /// Immutable item ID; duplicate Tracks have distinct IDs.
    pub id: Uuid,
    /// Track public ID.
    pub track_public_id: Uuid,
    /// Stable zero-based position.
    pub position: i32,
    /// Item creation time.
    pub created_at: OffsetDateTime,
}

/// Partial mutation of an owned playlist.
#[derive(Clone, Debug, Default)]
pub struct NativePlaylistUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub is_public: Option<bool>,
}

/// Smart-playlist creation facts.
#[derive(Clone, Debug)]
pub struct NativeSmartPlaylistCreate {
    pub public_api_id: Option<Uuid>,
    pub name: String,
    pub mql_query: String,
    pub entity_type: String,
    pub is_public: bool,
}

/// Partial smart-playlist mutation.
#[derive(Clone, Debug, Default)]
pub struct NativeSmartPlaylistUpdate {
    pub name: Option<String>,
    pub mql_query: Option<String>,
    pub entity_type: Option<String>,
    pub is_public: Option<bool>,
}

/// Stored smart-playlist facts.
#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct NativeSmartPlaylist {
    pub id: Uuid,
    pub public_api_id: Uuid,
    pub owner_user_id: Uuid,
    pub name: String,
    pub mql_query: String,
    pub entity_type: String,
    pub last_result_count: i32,
    pub last_evaluated_at: Option<OffsetDateTime>,
    pub is_public: bool,
    pub normalized_query: Option<String>,
    pub revision: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Result of an idempotent scrobble operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeScrobbleResult {
    /// A new play and its derived counters committed.
    Applied,
    /// The same client event was already durably applied.
    Adopted,
}

/// Native user-state persistence errors.
#[derive(Debug, Error)]
pub enum NativeUserStateStoreError {
    /// PostgreSQL rejected or could not execute the operation.
    #[error("native user-state persistence failed")]
    Database(#[source] sqlx::Error),
    /// A referenced User/catalog entity/playlist does not exist or is not owned.
    #[error("native resource was not found")]
    NotFound,
    /// Input violates a bounded native-v1 contract.
    #[error("invalid native user-state request: {0}")]
    InvalidRequest(&'static str),
    /// A replay used the same idempotency key for different facts.
    #[error("native user-state replay conflicts with durable facts")]
    Conflict,
}

impl From<sqlx::Error> for NativeUserStateStoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

/// PostgreSQL-backed user state and playlist store.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeUserStateStore;

impl NativeUserStateStore {
    /// Applies one genre star/hate mutation without treating a display string
    /// as a catalog entity identity.
    pub async fn mutate_genre(
        pool: &PgPool,
        user_id: Uuid,
        genre_name: &str,
        mutation: NativeUserStateMutation,
        now: OffsetDateTime,
    ) -> Result<NativeGenrePreference, NativeUserStateStoreError> {
        let genre_name = genre_name.trim();
        if genre_name.is_empty() || genre_name.chars().count() > 500 {
            return Err(NativeUserStateStoreError::InvalidRequest(
                "genre name is blank or too long",
            ));
        }
        let (action, flag) = match mutation {
            NativeUserStateMutation::Starred(value) => ("starred", value),
            NativeUserStateMutation::Hated(value) => ("hated", value),
            NativeUserStateMutation::Rating(_) | NativeUserStateMutation::Pinned(_) => {
                return Err(NativeUserStateStoreError::InvalidRequest(
                    "genre supports only starred and hated state",
                ));
            }
        };
        ensure_user_pool(pool, user_id).await?;
        sqlx::query_as::<_, NativeGenrePreference>(
            r#"
            INSERT INTO native_genre_user_states (
                user_id, genre_name, normalized_genre_name, starred, hated, created_at, updated_at
            ) VALUES (
                $1, $2, lower(btrim($2)),
                CASE WHEN $3 = 'starred' THEN $4 ELSE false END,
                CASE WHEN $3 = 'hated' THEN $4 ELSE false END,
                $5, $5
            )
            ON CONFLICT (user_id, normalized_genre_name) DO UPDATE SET
                genre_name = excluded.genre_name,
                starred = CASE WHEN $3 = 'starred' THEN $4 ELSE native_genre_user_states.starred END,
                hated = CASE WHEN $3 = 'hated' THEN $4 ELSE native_genre_user_states.hated END,
                updated_at = $5
            RETURNING genre_name, starred, hated
            "#,
        )
        .bind(user_id)
        .bind(genre_name)
        .bind(action)
        .bind(flag)
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }

    /// Lists exact genre display values with the requested preference.
    pub async fn genre_preferences(
        pool: &PgPool,
        user_id: Uuid,
        mutation: NativeUserStateMutation,
    ) -> Result<Vec<String>, NativeUserStateStoreError> {
        let column = match mutation {
            NativeUserStateMutation::Starred(true) => "starred",
            NativeUserStateMutation::Hated(true) => "hated",
            NativeUserStateMutation::Starred(false)
            | NativeUserStateMutation::Hated(false)
            | NativeUserStateMutation::Rating(_)
            | NativeUserStateMutation::Pinned(_) => {
                return Err(NativeUserStateStoreError::InvalidRequest(
                    "genre preference list requires an enabled star or hate state",
                ));
            }
        };
        let query = sqlx::AssertSqlSafe(format!(
            "SELECT genre_name FROM native_genre_user_states \
             WHERE user_id = $1 AND {column} ORDER BY normalized_genre_name"
        ));
        sqlx::query_scalar(query)
            .bind(user_id)
            .fetch_all(pool)
            .await
            .map_err(Into::into)
    }

    /// Applies one Artist interaction mutation, preserving all other fields.
    pub async fn mutate_artist(
        pool: &PgPool,
        user_id: Uuid,
        artist_public_id: Uuid,
        mutation: NativeUserStateMutation,
        now: OffsetDateTime,
    ) -> Result<NativeUserState, NativeUserStateStoreError> {
        validate_mutation(mutation)?;
        let (action, flag, rating) = mutation_parts(mutation);
        sqlx::query_as::<_, NativeUserState>(
            r#"
            INSERT INTO native_artist_user_states (
                user_id, artist_id, starred, hated, rating, pinned,
                play_count, last_played_at, created_at, updated_at
            )
            SELECT $1, resolve_artist_redirect(id),
                   CASE WHEN $3 = 'starred' THEN $4 ELSE false END,
                   CASE WHEN $3 = 'hated' THEN $4 ELSE false END,
                   CASE WHEN $3 = 'rating' THEN $5 ELSE 0 END,
                   CASE WHEN $3 = 'pinned' THEN $4 ELSE false END,
                   0, NULL, $6, $6
            FROM artists WHERE public_api_id = $2
            ON CONFLICT (user_id, artist_id) DO UPDATE SET
                starred = CASE
                    WHEN $3 = 'starred' THEN $4
                    WHEN $3 = 'hated' AND $4 THEN false
                    ELSE native_artist_user_states.starred
                END,
                hated = CASE
                    WHEN $3 = 'hated' THEN $4
                    WHEN $3 = 'starred' AND $4 THEN false
                    ELSE native_artist_user_states.hated
                END,
                rating = CASE WHEN $3 = 'rating' THEN $5 ELSE native_artist_user_states.rating END,
                pinned = CASE WHEN $3 = 'pinned' THEN $4 ELSE native_artist_user_states.pinned END,
                updated_at = $6
            RETURNING starred, hated, rating, pinned, play_count, last_played_at
            "#,
        )
        .bind(user_id)
        .bind(artist_public_id)
        .bind(action)
        .bind(flag)
        .bind(rating)
        .bind(now)
        .fetch_optional(pool)
        .await?
        .ok_or(NativeUserStateStoreError::NotFound)
    }

    /// Applies one Release interaction mutation, preserving all other fields.
    pub async fn mutate_release(
        pool: &PgPool,
        user_id: Uuid,
        release_public_id: Uuid,
        mutation: NativeUserStateMutation,
        now: OffsetDateTime,
    ) -> Result<NativeUserState, NativeUserStateStoreError> {
        validate_mutation(mutation)?;
        let (action, flag, rating) = mutation_parts(mutation);
        sqlx::query_as::<_, NativeUserState>(
            r#"
            INSERT INTO native_release_user_states (
                user_id, release_id, starred, hated, rating, pinned,
                play_count, last_played_at, created_at, updated_at
            )
            SELECT $1, id,
                   CASE WHEN $3 = 'starred' THEN $4 ELSE false END,
                   CASE WHEN $3 = 'hated' THEN $4 ELSE false END,
                   CASE WHEN $3 = 'rating' THEN $5 ELSE 0 END,
                   CASE WHEN $3 = 'pinned' THEN $4 ELSE false END,
                   0, NULL, $6, $6
            FROM releases WHERE public_api_id = $2 AND status = 'active'
            ON CONFLICT (user_id, release_id) DO UPDATE SET
                starred = CASE
                    WHEN $3 = 'starred' THEN $4
                    WHEN $3 = 'hated' AND $4 THEN false
                    ELSE native_release_user_states.starred
                END,
                hated = CASE
                    WHEN $3 = 'hated' THEN $4
                    WHEN $3 = 'starred' AND $4 THEN false
                    ELSE native_release_user_states.hated
                END,
                rating = CASE WHEN $3 = 'rating' THEN $5 ELSE native_release_user_states.rating END,
                pinned = CASE WHEN $3 = 'pinned' THEN $4 ELSE native_release_user_states.pinned END,
                updated_at = $6
            RETURNING starred, hated, rating, pinned, play_count, last_played_at
            "#,
        )
        .bind(user_id)
        .bind(release_public_id)
        .bind(action)
        .bind(flag)
        .bind(rating)
        .bind(now)
        .fetch_optional(pool)
        .await?
        .ok_or(NativeUserStateStoreError::NotFound)
    }

    /// Applies one Track interaction mutation, preserving all other fields.
    pub async fn mutate_track(
        pool: &PgPool,
        user_id: Uuid,
        track_public_id: Uuid,
        mutation: NativeUserStateMutation,
        now: OffsetDateTime,
    ) -> Result<NativeUserState, NativeUserStateStoreError> {
        validate_mutation(mutation)?;
        let (action, flag, rating) = mutation_parts(mutation);
        sqlx::query_as::<_, NativeUserState>(
            r#"
            INSERT INTO native_track_user_states (
                user_id, track_id, starred, hated, rating, pinned,
                play_count, last_played_at, created_at, updated_at
            )
            SELECT $1, id,
                   CASE WHEN $3 = 'starred' THEN $4 ELSE false END,
                   CASE WHEN $3 = 'hated' THEN $4 ELSE false END,
                   CASE WHEN $3 = 'rating' THEN $5 ELSE 0 END,
                   CASE WHEN $3 = 'pinned' THEN $4 ELSE false END,
                   0, NULL, $6, $6
            FROM tracks WHERE public_api_id = $2
            ON CONFLICT (user_id, track_id) DO UPDATE SET
                starred = CASE
                    WHEN $3 = 'starred' THEN $4
                    WHEN $3 = 'hated' AND $4 THEN false
                    ELSE native_track_user_states.starred
                END,
                hated = CASE
                    WHEN $3 = 'hated' THEN $4
                    WHEN $3 = 'starred' AND $4 THEN false
                    ELSE native_track_user_states.hated
                END,
                rating = CASE WHEN $3 = 'rating' THEN $5 ELSE native_track_user_states.rating END,
                pinned = CASE WHEN $3 = 'pinned' THEN $4 ELSE native_track_user_states.pinned END,
                updated_at = $6
            RETURNING starred, hated, rating, pinned, play_count, last_played_at
            "#,
        )
        .bind(user_id)
        .bind(track_public_id)
        .bind(action)
        .bind(flag)
        .bind(rating)
        .bind(now)
        .fetch_optional(pool)
        .await?
        .ok_or(NativeUserStateStoreError::NotFound)
    }

    /// Creates a playlist and its initial ordered items atomically.
    pub async fn create_playlist(
        pool: &PgPool,
        owner_user_id: Uuid,
        request: &NativePlaylistCreate,
        now: OffsetDateTime,
    ) -> Result<NativePlaylist, NativeUserStateStoreError> {
        validate_playlist_request(request)?;
        let mut tx = pool.begin().await?;
        ensure_user(&mut tx, owner_user_id).await?;
        let playlist = sqlx::query_as::<_, NativePlaylist>(
            r#"
            INSERT INTO native_playlists (
                id, public_api_id, owner_user_id, name, description,
                is_public, revision, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $7)
            RETURNING id, public_api_id, owner_user_id, name, description,
                      is_public, revision, created_at, updated_at
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(request.public_api_id.unwrap_or_else(Uuid::now_v7))
        .bind(owner_user_id)
        .bind(request.name.trim())
        .bind(request.description.as_deref())
        .bind(request.is_public)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        insert_playlist_items(
            &mut tx,
            playlist.id,
            owner_user_id,
            &request.track_public_ids,
            0,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(playlist)
    }

    /// Loads an owned or public playlist without exposing a private record.
    pub async fn playlist(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
    ) -> Result<NativePlaylist, NativeUserStateStoreError> {
        sqlx::query_as::<_, NativePlaylist>(
            r#"
            SELECT id, public_api_id, owner_user_id, name, description,
                   is_public, revision, created_at, updated_at
            FROM native_playlists
            WHERE public_api_id = $1 AND (owner_user_id = $2 OR is_public)
            "#,
        )
        .bind(playlist_public_id)
        .bind(owner_user_id)
        .fetch_optional(pool)
        .await?
        .ok_or(NativeUserStateStoreError::NotFound)
    }

    /// Lists owned playlists in the captured deterministic name order.
    pub async fn playlists(
        pool: &PgPool,
        owner_user_id: Uuid,
        page: u32,
        page_size: u16,
    ) -> Result<(i64, Vec<NativePlaylist>), NativeUserStateStoreError> {
        Self::playlists_ordered(pool, owner_user_id, page, page_size, false).await
    }

    /// Lists owned playlists oldest first — the frozen Jellyfin baseline
    /// orders `/Playlists` by ascending creation time
    /// (`jf-playlists/playlists-list`).
    pub async fn playlists_oldest_first(
        pool: &PgPool,
        owner_user_id: Uuid,
        page: u32,
        page_size: u16,
    ) -> Result<(i64, Vec<NativePlaylist>), NativeUserStateStoreError> {
        Self::playlists_ordered(pool, owner_user_id, page, page_size, true).await
    }

    async fn playlists_ordered(
        pool: &PgPool,
        owner_user_id: Uuid,
        page: u32,
        page_size: u16,
        oldest_first: bool,
    ) -> Result<(i64, Vec<NativePlaylist>), NativeUserStateStoreError> {
        let page = page.max(1);
        let page_size = page_size.clamp(1, MAX_NATIVE_PLAYLIST_PAGE_SIZE);
        let offset = i64::from(page - 1)
            .checked_mul(i64::from(page_size))
            .ok_or(NativeUserStateStoreError::InvalidRequest(
                "playlist page overflow",
            ))?;
        let total =
            sqlx::query_scalar("SELECT count(*) FROM native_playlists WHERE owner_user_id = $1")
                .bind(owner_user_id)
                .fetch_one(pool)
                .await?;
        let ordering = if oldest_first {
            "created_at ASC, public_api_id"
        } else {
            "lower(name) ASC, public_api_id"
        };
        let rows = sqlx::query_as::<_, NativePlaylist>(sqlx::AssertSqlSafe(format!(
            "SELECT id, public_api_id, owner_user_id, name, description, is_public,
                    revision, created_at, updated_at
             FROM native_playlists WHERE owner_user_id = $1
             ORDER BY {ordering}
             LIMIT $2 OFFSET $3"
        )))
        .bind(owner_user_id)
        .bind(i64::from(page_size))
        .bind(offset)
        .fetch_all(pool)
        .await?;
        Ok((total, rows))
    }

    /// Mutates only supplied playlist fields under owner authorization.
    pub async fn update_playlist(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
        request: &NativePlaylistUpdate,
        now: OffsetDateTime,
    ) -> Result<NativePlaylist, NativeUserStateStoreError> {
        if request
            .name
            .as_ref()
            .is_some_and(|name| name.trim().is_empty() || name.chars().count() > 500)
            || request
                .description
                .as_ref()
                .is_some_and(|description| description.chars().count() > 10_000)
        {
            return Err(NativeUserStateStoreError::InvalidRequest(
                "invalid playlist update",
            ));
        }
        sqlx::query_as::<_, NativePlaylist>(
            "UPDATE native_playlists SET
                name = COALESCE($3, name), description = COALESCE($4, description),
                is_public = COALESCE($5, is_public), revision = revision + 1,
                updated_at = $6
             WHERE public_api_id = $1 AND owner_user_id = $2
             RETURNING id, public_api_id, owner_user_id, name, description, is_public,
                       revision, created_at, updated_at",
        )
        .bind(playlist_public_id)
        .bind(owner_user_id)
        .bind(request.name.as_ref().map(|name| name.trim()))
        .bind(request.description.as_deref())
        .bind(request.is_public)
        .bind(now)
        .fetch_optional(pool)
        .await?
        .ok_or(NativeUserStateStoreError::NotFound)
    }

    /// Lists playlist items in deterministic order.
    pub async fn playlist_items(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
    ) -> Result<Vec<NativePlaylistItem>, NativeUserStateStoreError> {
        let rows = sqlx::query_as::<_, NativePlaylistItem>(
            r#"
            SELECT i.id, t.public_api_id AS track_public_id, i.position, i.created_at
            FROM native_playlists p
            JOIN native_playlist_items i ON i.playlist_id = p.id
            JOIN tracks t ON t.id = i.track_id
            WHERE p.public_api_id = $1 AND p.owner_user_id = $2
            ORDER BY i.position, i.id
            LIMIT 10001
            "#,
        )
        .bind(playlist_public_id)
        .bind(owner_user_id)
        .fetch_all(pool)
        .await?;
        if rows.len() > MAX_NATIVE_PLAYLIST_ITEMS {
            return Err(NativeUserStateStoreError::InvalidRequest(
                "playlist exceeds item limit",
            ));
        }
        Ok(rows)
    }

    /// Appends Tracks in the supplied order; both accepted ADR-0114 wire shapes use this method.
    pub async fn append_playlist_tracks(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
        track_public_ids: &[Uuid],
        now: OffsetDateTime,
    ) -> Result<(), NativeUserStateStoreError> {
        validate_track_ids(track_public_ids)?;
        let mut tx = pool.begin().await?;
        let playlist_id = lock_owned_playlist(&mut tx, owner_user_id, playlist_public_id).await?;
        let next_position: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position) + 1, 0)::integer FROM native_playlist_items WHERE playlist_id = $1",
        )
        .bind(playlist_id)
        .fetch_one(&mut *tx)
        .await?;
        let existing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM native_playlist_items WHERE playlist_id = $1")
                .bind(playlist_id)
                .fetch_one(&mut *tx)
                .await?;
        let existing_track_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT t.public_api_id
             FROM native_playlist_items item
             JOIN tracks t ON t.id = item.track_id
             WHERE item.playlist_id = $1 AND t.public_api_id = ANY($2)",
        )
        .bind(playlist_id)
        .bind(track_public_ids)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect::<HashSet<_>>();
        let mut seen_track_ids = existing_track_ids;
        let new_track_ids = track_public_ids
            .iter()
            .copied()
            .filter(|track_id| seen_track_ids.insert(*track_id))
            .collect::<Vec<_>>();
        if new_track_ids.is_empty() {
            tx.commit().await?;
            return Ok(());
        }
        let final_count = usize::try_from(existing)
            .ok()
            .and_then(|count| count.checked_add(new_track_ids.len()))
            .ok_or(NativeUserStateStoreError::InvalidRequest(
                "playlist item count overflow",
            ))?;
        if final_count > MAX_NATIVE_PLAYLIST_ITEMS {
            return Err(NativeUserStateStoreError::InvalidRequest(
                "playlist exceeds item limit",
            ));
        }
        insert_playlist_items(
            &mut tx,
            playlist_id,
            owner_user_id,
            &new_track_ids,
            next_position,
            now,
        )
        .await?;
        bump_playlist(&mut tx, playlist_id, now).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Reorders by Track public IDs, preserving duplicate occurrences deterministically.
    pub async fn reorder_playlist_tracks(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
        ordered_track_public_ids: &[Uuid],
        now: OffsetDateTime,
    ) -> Result<(), NativeUserStateStoreError> {
        validate_track_ids(ordered_track_public_ids)?;
        let mut tx = pool.begin().await?;
        let playlist_id = lock_owned_playlist(&mut tx, owner_user_id, playlist_public_id).await?;
        let current = sqlx::query_as::<_, NativePlaylistItem>(
            r#"
            SELECT i.id, t.public_api_id AS track_public_id, i.position, i.created_at
            FROM native_playlist_items i JOIN tracks t ON t.id = i.track_id
            WHERE i.playlist_id = $1 ORDER BY i.position, i.id LIMIT 10001
            "#,
        )
        .bind(playlist_id)
        .fetch_all(&mut *tx)
        .await?;
        if current.len() > MAX_NATIVE_PLAYLIST_ITEMS {
            return Err(NativeUserStateStoreError::Conflict);
        }
        // MeloAmp can resend a drag-reorder list while a prior request is
        // still in flight. Treat repeated Track IDs as one requested item;
        // the playlist itself remains occurrence-aware if it contains
        // duplicates from older data.
        let mut requested_track_ids = Vec::with_capacity(ordered_track_public_ids.len());
        let mut requested = HashSet::with_capacity(ordered_track_public_ids.len());
        for track_id in ordered_track_public_ids {
            if requested.insert(*track_id) {
                requested_track_ids.push(*track_id);
            }
        }
        let mut available: HashMap<Uuid, VecDeque<Uuid>> = HashMap::new();
        for item in &current {
            available
                .entry(item.track_public_id)
                .or_default()
                .push_back(item.id);
        }
        let mut selected = HashSet::with_capacity(requested_track_ids.len());
        let mut item_ids = Vec::with_capacity(current.len());
        for track_id in requested_track_ids {
            let item_id = available
                .get_mut(&track_id)
                .and_then(VecDeque::pop_front)
                .ok_or(NativeUserStateStoreError::Conflict)?;
            selected.insert(item_id);
            item_ids.push(item_id);
        }
        // The legacy endpoint accepts a partial list. Preserve omitted items
        // after the requested prefix in their prior relative order, which
        // also gives the atomic update a complete, unique position set.
        for item in current {
            if selected.insert(item.id) {
                item_ids.push(item.id);
            }
        }
        let positions = (0..item_ids.len())
            .map(i32::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NativeUserStateStoreError::InvalidRequest("playlist position overflow"))?;
        sqlx::query(
            r#"
            UPDATE native_playlist_items i SET position = ordered.position
            FROM unnest($1::uuid[], $2::integer[]) AS ordered(id, position)
            WHERE i.id = ordered.id AND i.playlist_id = $3
            "#,
        )
        .bind(&item_ids)
        .bind(&positions)
        .bind(playlist_id)
        .execute(&mut *tx)
        .await?;
        bump_playlist(&mut tx, playlist_id, now).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Removes every occurrence of the supplied Track IDs and compacts positions atomically.
    pub async fn remove_playlist_tracks(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
        track_public_ids: &[Uuid],
        now: OffsetDateTime,
    ) -> Result<(), NativeUserStateStoreError> {
        validate_track_ids(track_public_ids)?;
        let mut tx = pool.begin().await?;
        let playlist_id = lock_owned_playlist(&mut tx, owner_user_id, playlist_public_id).await?;
        sqlx::query(
            "DELETE FROM native_playlist_items i USING tracks t
             WHERE i.playlist_id = $1 AND i.track_id = t.id AND t.public_api_id = ANY($2)",
        )
        .bind(playlist_id)
        .bind(track_public_ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE native_playlist_items i SET position = ordered.position
             FROM (
               SELECT id, (row_number() OVER (ORDER BY position, id) - 1)::integer AS position
               FROM native_playlist_items WHERE playlist_id = $1
             ) ordered WHERE i.id = ordered.id",
        )
        .bind(playlist_id)
        .execute(&mut *tx)
        .await?;
        bump_playlist(&mut tx, playlist_id, now).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replaces or clears a bounded verified playlist image.
    pub async fn set_playlist_image(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
        image: Option<(&str, &[u8])>,
        now: OffsetDateTime,
    ) -> Result<(), NativeUserStateStoreError> {
        if let Some((mime, bytes)) = image
            && (!matches!(mime, "image/jpeg" | "image/png" | "image/webp")
                || bytes.is_empty()
                || bytes.len() > 10 * 1024 * 1024)
        {
            return Err(NativeUserStateStoreError::InvalidRequest(
                "invalid playlist image",
            ));
        }
        let affected = sqlx::query(
            "UPDATE native_playlists SET image_mime_type = $3, image_bytes = $4,
                    revision = revision + 1, updated_at = $5
             WHERE public_api_id = $1 AND owner_user_id = $2",
        )
        .bind(playlist_public_id)
        .bind(owner_user_id)
        .bind(image.map(|value| value.0))
        .bind(image.map(|value| value.1))
        .bind(now)
        .execute(pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(NativeUserStateStoreError::NotFound);
        }
        Ok(())
    }

    /// Creates an owned smart playlist after bounded MQL validation by the caller.
    pub async fn create_smart_playlist(
        pool: &PgPool,
        owner_user_id: Uuid,
        request: &NativeSmartPlaylistCreate,
        normalized_query: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<NativeSmartPlaylist, NativeUserStateStoreError> {
        validate_smart_playlist(&request.name, &request.mql_query, &request.entity_type)?;
        ensure_user_pool(pool, owner_user_id).await?;
        sqlx::query_as::<_, NativeSmartPlaylist>(
            "INSERT INTO native_smart_playlists (
               id, public_api_id, owner_user_id, name, mql_query, entity_type,
               is_public, normalized_query, created_at, updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)
             RETURNING id, public_api_id, owner_user_id, name, mql_query, entity_type,
               last_result_count, last_evaluated_at, is_public, normalized_query,
               revision, created_at, updated_at",
        )
        .bind(Uuid::now_v7())
        .bind(request.public_api_id.unwrap_or_else(Uuid::now_v7))
        .bind(owner_user_id)
        .bind(request.name.trim())
        .bind(request.mql_query.trim())
        .bind(request.entity_type.to_ascii_lowercase())
        .bind(request.is_public)
        .bind(normalized_query)
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(map_smart_unique)
    }

    /// Resolves an owned or public smart playlist without exposing private records.
    pub async fn smart_playlist(
        pool: &PgPool,
        user_id: Uuid,
        public_api_id: Uuid,
    ) -> Result<NativeSmartPlaylist, NativeUserStateStoreError> {
        sqlx::query_as::<_, NativeSmartPlaylist>(
            "SELECT id, public_api_id, owner_user_id, name, mql_query, entity_type,
               last_result_count, last_evaluated_at, is_public, normalized_query,
               revision, created_at, updated_at
             FROM native_smart_playlists
             WHERE public_api_id = $1 AND (owner_user_id = $2 OR is_public)",
        )
        .bind(public_api_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .ok_or(NativeUserStateStoreError::NotFound)
    }

    pub async fn smart_playlists(
        pool: &PgPool,
        owner_user_id: Uuid,
        page: u32,
        page_size: u16,
        order_by: &str,
        descending: bool,
    ) -> Result<(i64, Vec<NativeSmartPlaylist>), NativeUserStateStoreError> {
        let page_size = page_size.clamp(1, MAX_NATIVE_PLAYLIST_PAGE_SIZE);
        let offset = i64::from(page.max(1) - 1)
            .checked_mul(i64::from(page_size))
            .ok_or(NativeUserStateStoreError::InvalidRequest(
                "playlist page overflow",
            ))?;
        let total = sqlx::query_scalar(
            "SELECT count(*) FROM native_smart_playlists WHERE owner_user_id = $1",
        )
        .bind(owner_user_id)
        .fetch_one(pool)
        .await?;
        let order_column = match order_by {
            "name" => "name",
            "last_result_count" => "last_result_count",
            _ => "created_at",
        };
        let direction = if descending { "DESC" } else { "ASC" };
        let query = sqlx::AssertSqlSafe(format!(
            "SELECT id, public_api_id, owner_user_id, name, mql_query, entity_type,
               last_result_count, last_evaluated_at, is_public, normalized_query,
               revision, created_at, updated_at
             FROM native_smart_playlists WHERE owner_user_id = $1
             ORDER BY {order_column} {direction}, public_api_id LIMIT $2 OFFSET $3"
        ));
        let rows = sqlx::query_as::<_, NativeSmartPlaylist>(query)
            .bind(owner_user_id)
            .bind(i64::from(page_size))
            .bind(offset)
            .fetch_all(pool)
            .await?;
        Ok((total, rows))
    }

    pub async fn update_smart_playlist(
        pool: &PgPool,
        owner_user_id: Uuid,
        public_api_id: Uuid,
        request: &NativeSmartPlaylistUpdate,
        normalized_query: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<NativeSmartPlaylist, NativeUserStateStoreError> {
        if request
            .name
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 255)
            || request
                .mql_query
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 4_000)
            || request.entity_type.as_ref().is_some_and(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "songs" | "albums" | "artists"
                )
            })
        {
            return Err(NativeUserStateStoreError::InvalidRequest(
                "invalid smart playlist",
            ));
        }
        sqlx::query_as::<_, NativeSmartPlaylist>(
            "UPDATE native_smart_playlists SET
               name=COALESCE($3,name), mql_query=COALESCE($4,mql_query),
               entity_type=COALESCE($5,entity_type), is_public=COALESCE($6,is_public),
               normalized_query=CASE WHEN $4 IS NULL THEN normalized_query ELSE $7 END,
               last_result_count=CASE WHEN $4 IS NULL AND $5 IS NULL THEN last_result_count ELSE 0 END,
               last_evaluated_at=CASE WHEN $4 IS NULL AND $5 IS NULL THEN last_evaluated_at ELSE NULL END,
               revision=revision+1, updated_at=$8
             WHERE public_api_id=$1 AND owner_user_id=$2
             RETURNING id, public_api_id, owner_user_id, name, mql_query, entity_type,
               last_result_count, last_evaluated_at, is_public, normalized_query,
               revision, created_at, updated_at",
        )
        .bind(public_api_id)
        .bind(owner_user_id)
        .bind(request.name.as_ref().map(|value| value.trim()))
        .bind(request.mql_query.as_ref().map(|value| value.trim()))
        .bind(
            request
                .entity_type
                .as_ref()
                .map(|value| value.to_ascii_lowercase()),
        )
        .bind(request.is_public)
        .bind(normalized_query)
        .bind(now)
        .fetch_optional(pool)
        .await
        .map_err(map_smart_unique)?
        .ok_or(NativeUserStateStoreError::NotFound)
    }

    pub async fn record_smart_playlist_evaluation(
        pool: &PgPool,
        owner_user_id: Uuid,
        public_api_id: Uuid,
        result_count: i32,
        now: OffsetDateTime,
    ) -> Result<(), NativeUserStateStoreError> {
        let changed = sqlx::query(
            "UPDATE native_smart_playlists SET last_result_count=$2,
               last_evaluated_at=$3, updated_at=$3, revision=revision+1
             WHERE public_api_id=$1 AND owner_user_id=$4",
        )
        .bind(public_api_id)
        .bind(result_count.max(0))
        .bind(now)
        .bind(owner_user_id)
        .execute(pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Err(NativeUserStateStoreError::NotFound);
        }
        Ok(())
    }

    pub async fn delete_smart_playlist(
        pool: &PgPool,
        owner_user_id: Uuid,
        public_api_id: Uuid,
    ) -> Result<(), NativeUserStateStoreError> {
        let affected = sqlx::query(
            "DELETE FROM native_smart_playlists WHERE public_api_id=$1 AND owner_user_id=$2",
        )
        .bind(public_api_id)
        .bind(owner_user_id)
        .execute(pool)
        .await?
        .rows_affected();
        if affected == 0 {
            Err(NativeUserStateStoreError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Deletes an owned playlist and its item rows atomically.
    pub async fn delete_playlist(
        pool: &PgPool,
        owner_user_id: Uuid,
        playlist_public_id: Uuid,
    ) -> Result<(), NativeUserStateStoreError> {
        let affected = sqlx::query(
            "DELETE FROM native_playlists WHERE public_api_id = $1 AND owner_user_id = $2",
        )
        .bind(playlist_public_id)
        .bind(owner_user_id)
        .execute(pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(NativeUserStateStoreError::NotFound);
        }
        Ok(())
    }

    /// Records now-playing state without incrementing completed-play counters.
    pub async fn record_now_playing(
        pool: &PgPool,
        user_id: Uuid,
        track_public_id: Uuid,
        client_event_id: &str,
        started_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Result<(), NativeUserStateStoreError> {
        validate_client_event_id(client_event_id)?;
        let affected = sqlx::query(
            r#"
            INSERT INTO native_now_playing (
                user_id, track_id, client_event_id, started_at, updated_at
            )
            SELECT $1, id, $3, $4, $5 FROM tracks WHERE public_api_id = $2
            ON CONFLICT (user_id) DO UPDATE SET
                track_id = excluded.track_id,
                client_event_id = excluded.client_event_id,
                started_at = excluded.started_at,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(user_id)
        .bind(track_public_id)
        .bind(client_event_id)
        .bind(started_at)
        .bind(now)
        .execute(pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(NativeUserStateStoreError::NotFound);
        }
        Ok(())
    }

    /// Removes listener now-playing rows whose heartbeat is older than the
    /// caller's bounded staleness window. Podcast history uses the same
    /// cleanup pass because its active rows are a separate compatibility
    /// projection and must not remain visible after a client disappears.
    pub async fn cleanup_stale_now_playing(
        pool: &PgPool,
        stale_before: OffsetDateTime,
    ) -> Result<u64, NativeUserStateStoreError> {
        let mut tx = pool.begin().await?;
        let native_removed = sqlx::query("DELETE FROM native_now_playing WHERE updated_at < $1")
            .bind(stale_before)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let podcast_removed = sqlx::query(
            "DELETE FROM native_podcast_play_history
             WHERE is_now_playing
               AND COALESCE(last_heartbeat_at, played_at) < $1",
        )
        .bind(stale_before)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(native_removed.saturating_add(podcast_removed))
    }

    /// Records one completed play and derived Track/Release/primary-Artist counters atomically.
    pub async fn record_scrobble(
        pool: &PgPool,
        user_id: Uuid,
        track_public_id: Uuid,
        client_event_id: &str,
        played_at: OffsetDateTime,
        submitted_at: OffsetDateTime,
        duration_seconds: Option<i32>,
    ) -> Result<NativeScrobbleResult, NativeUserStateStoreError> {
        validate_client_event_id(client_event_id)?;
        if duration_seconds.is_some_and(|duration| duration < 0) || played_at > submitted_at {
            return Err(NativeUserStateStoreError::InvalidRequest(
                "invalid scrobble time or duration",
            ));
        }
        let mut tx = pool.begin().await?;
        let track = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, release_id FROM tracks WHERE public_api_id = $1",
        )
        .bind(track_public_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(NativeUserStateStoreError::NotFound)?;
        let inserted = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO native_track_plays (
                id, user_id, track_id, client_event_id, played_at,
                submitted_at, duration_seconds, completed
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, true)
            ON CONFLICT (user_id, client_event_id) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(track.0)
        .bind(client_event_id)
        .bind(played_at)
        .bind(submitted_at)
        .bind(duration_seconds)
        .fetch_optional(&mut *tx)
        .await?;
        if inserted.is_none() {
            let same: bool = sqlx::query_scalar(
                r#"
                SELECT track_id = $3 AND played_at = $4 AND duration_seconds IS NOT DISTINCT FROM $5
                FROM native_track_plays WHERE user_id = $1 AND client_event_id = $2
                "#,
            )
            .bind(user_id)
            .bind(client_event_id)
            .bind(track.0)
            .bind(played_at)
            .bind(duration_seconds)
            .fetch_one(&mut *tx)
            .await?;
            if !same {
                return Err(NativeUserStateStoreError::Conflict);
            }
            tx.commit().await?;
            return Ok(NativeScrobbleResult::Adopted);
        }
        increment_play_counters(&mut tx, user_id, track.0, track.1, played_at, submitted_at)
            .await?;
        sqlx::query("DELETE FROM native_now_playing WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(NativeScrobbleResult::Applied)
    }
}

fn validate_mutation(mutation: NativeUserStateMutation) -> Result<(), NativeUserStateStoreError> {
    if let NativeUserStateMutation::Rating(rating) = mutation
        && !(0..=5).contains(&rating)
    {
        return Err(NativeUserStateStoreError::InvalidRequest(
            "rating must be between zero and five",
        ));
    }
    Ok(())
}

fn mutation_parts(mutation: NativeUserStateMutation) -> (&'static str, bool, i16) {
    match mutation {
        NativeUserStateMutation::Starred(value) => ("starred", value, 0),
        NativeUserStateMutation::Hated(value) => ("hated", value, 0),
        NativeUserStateMutation::Rating(value) => ("rating", false, value),
        NativeUserStateMutation::Pinned(value) => ("pinned", value, 0),
    }
}

fn validate_playlist_request(
    request: &NativePlaylistCreate,
) -> Result<(), NativeUserStateStoreError> {
    if request.name.trim().is_empty() || request.name.chars().count() > 500 {
        return Err(NativeUserStateStoreError::InvalidRequest(
            "playlist name is blank or too long",
        ));
    }
    if request
        .description
        .as_ref()
        .is_some_and(|value| value.chars().count() > 10_000)
    {
        return Err(NativeUserStateStoreError::InvalidRequest(
            "playlist description is too long",
        ));
    }
    validate_track_ids(&request.track_public_ids)
}

fn validate_track_ids(ids: &[Uuid]) -> Result<(), NativeUserStateStoreError> {
    if ids.len() > MAX_NATIVE_PLAYLIST_ITEMS {
        return Err(NativeUserStateStoreError::InvalidRequest(
            "too many playlist items",
        ));
    }
    Ok(())
}

fn validate_smart_playlist(
    name: &str,
    mql_query: &str,
    entity_type: &str,
) -> Result<(), NativeUserStateStoreError> {
    if name.trim().is_empty()
        || name.chars().count() > 255
        || mql_query.trim().is_empty()
        || mql_query.chars().count() > 4_000
        || !matches!(
            entity_type.to_ascii_lowercase().as_str(),
            "songs" | "albums" | "artists"
        )
    {
        return Err(NativeUserStateStoreError::InvalidRequest(
            "invalid smart playlist",
        ));
    }
    Ok(())
}

fn map_smart_unique(error: sqlx::Error) -> NativeUserStateStoreError {
    if error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
    {
        NativeUserStateStoreError::Conflict
    } else {
        NativeUserStateStoreError::Database(error)
    }
}

fn validate_client_event_id(value: &str) -> Result<(), NativeUserStateStoreError> {
    if value.trim().is_empty() || value.chars().count() > 256 {
        return Err(NativeUserStateStoreError::InvalidRequest(
            "client event id is blank or too long",
        ));
    }
    Ok(())
}

async fn ensure_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), NativeUserStateStoreError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_users WHERE id = $1)")
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?;
    if !exists {
        return Err(NativeUserStateStoreError::NotFound);
    }
    Ok(())
}

async fn ensure_user_pool(pool: &PgPool, user_id: Uuid) -> Result<(), NativeUserStateStoreError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_users WHERE id = $1)")
            .bind(user_id)
            .fetch_one(pool)
            .await?;
    if !exists {
        return Err(NativeUserStateStoreError::NotFound);
    }
    Ok(())
}

async fn lock_owned_playlist(
    tx: &mut Transaction<'_, Postgres>,
    owner_user_id: Uuid,
    playlist_public_id: Uuid,
) -> Result<Uuid, NativeUserStateStoreError> {
    sqlx::query_scalar(
        "SELECT id FROM native_playlists WHERE public_api_id = $1 AND owner_user_id = $2 FOR UPDATE",
    )
    .bind(playlist_public_id)
    .bind(owner_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(NativeUserStateStoreError::NotFound)
}

async fn insert_playlist_items(
    tx: &mut Transaction<'_, Postgres>,
    playlist_id: Uuid,
    owner_user_id: Uuid,
    track_public_ids: &[Uuid],
    first_position: i32,
    now: OffsetDateTime,
) -> Result<(), NativeUserStateStoreError> {
    if track_public_ids.is_empty() {
        return Ok(());
    }
    let requested = i64::try_from(track_public_ids.len())
        .map_err(|_| NativeUserStateStoreError::InvalidRequest("playlist item count overflow"))?;
    let resolved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM unnest($1::uuid[]) requested(id) JOIN tracks t ON t.public_api_id = requested.id",
    )
    .bind(track_public_ids)
    .fetch_one(&mut **tx)
    .await?;
    if resolved != requested {
        return Err(NativeUserStateStoreError::NotFound);
    }
    let positions = (0..track_public_ids.len())
        .map(|offset| {
            i32::try_from(offset)
                .ok()
                .and_then(|value| first_position.checked_add(value))
                .ok_or(NativeUserStateStoreError::InvalidRequest(
                    "playlist position overflow",
                ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let item_ids = (0..track_public_ids.len())
        .map(|_| Uuid::now_v7())
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO native_playlist_items (
            id, playlist_id, track_id, position, added_by_user_id, created_at
        )
        SELECT input.item_id, $1, t.id, input.position, $2, $6
        FROM unnest($3::uuid[], $4::uuid[], $5::integer[])
             WITH ORDINALITY AS input(item_id, public_id, position, ordinal)
        JOIN tracks t ON t.public_api_id = input.public_id
        ORDER BY input.ordinal
        "#,
    )
    .bind(playlist_id)
    .bind(owner_user_id)
    .bind(&item_ids)
    .bind(track_public_ids)
    .bind(&positions)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn bump_playlist(
    tx: &mut Transaction<'_, Postgres>,
    playlist_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), NativeUserStateStoreError> {
    sqlx::query(
        "UPDATE native_playlists SET revision = revision + 1, updated_at = $2 WHERE id = $1",
    )
    .bind(playlist_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn increment_play_counters(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    track_id: Uuid,
    release_id: Uuid,
    played_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<(), NativeUserStateStoreError> {
    sqlx::query(
        r#"
        INSERT INTO native_track_user_states (
            user_id, track_id, play_count, last_played_at, created_at, updated_at
        ) VALUES ($1, $2, 1, $3, $4, $4)
        ON CONFLICT (user_id, track_id) DO UPDATE SET
            play_count = native_track_user_states.play_count + 1,
            last_played_at = GREATEST(native_track_user_states.last_played_at, excluded.last_played_at),
            updated_at = $4
        "#,
    )
    .bind(user_id)
    .bind(track_id)
    .bind(played_at)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO native_release_user_states (
            user_id, release_id, play_count, last_played_at, created_at, updated_at
        ) VALUES ($1, $2, 1, $3, $4, $4)
        ON CONFLICT (user_id, release_id) DO UPDATE SET
            play_count = native_release_user_states.play_count + 1,
            last_played_at = GREATEST(native_release_user_states.last_played_at, excluded.last_played_at),
            updated_at = $4
        "#,
    )
    .bind(user_id)
    .bind(release_id)
    .bind(played_at)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO native_artist_user_states (
            user_id, artist_id, play_count, last_played_at, created_at, updated_at
        )
        SELECT $1, artist_id, 1, $3, $4, $4
        FROM track_artists WHERE track_id = $2 AND role = 'primary'
        ON CONFLICT (user_id, artist_id) DO UPDATE SET
            play_count = native_artist_user_states.play_count + 1,
            last_played_at = GREATEST(native_artist_user_states.last_played_at, excluded.last_played_at),
            updated_at = $4
        "#,
    )
    .bind(user_id)
    .bind(track_id)
    .bind(played_at)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
