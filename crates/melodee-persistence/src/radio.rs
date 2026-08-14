//! Internet radio station persistence (PRD-RADIO-001/002, ADR-0134 §1).
//!
//! `opensubsonic_radio_stations` (migration 0020) is the single authoritative
//! radio store. The OpenSubsonic adapter owns its captured wire behavior; this
//! store is the shared, validated access layer the Admin API surface uses, so
//! both surfaces read and write the same rows and station ids are never
//! regenerated (the migration's immutability trigger rejects `id` /
//! `public_api_id` rewrites as the fail-closed backstop).
//!
//! Validation mirrors the table's CHECK constraints exactly — no stricter
//! rules — so a station accepted through one surface is always representable
//! on the other.

use sqlx::{FromRow, PgPool};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Bound matching the `opensubsonic_radio_stations_name_check` constraint.
pub const MAX_RADIO_NAME_CHARS: usize = 500;
/// Bound matching the `opensubsonic_radio_stations_stream_url_check` /
/// `opensubsonic_radio_stations_home_page_check` constraints.
pub const MAX_RADIO_URL_CHARS: usize = 2000;

/// One stored internet radio station. `public_api_id` is the immutable
/// cross-surface identifier (the OpenSubsonic adapter renders it as
/// `radio_{uuid}`); `id` is the internal primary key.
#[derive(Clone, Debug, Eq, FromRow, PartialEq)]
pub struct RadioStation {
    pub id: Uuid,
    pub public_api_id: Uuid,
    pub name: String,
    pub stream_url: String,
    pub home_page_url: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Station creation input. Both ids are issued here as fresh v7 UUIDs; the
/// caller never supplies them (PRD-RADIO-002 id stability).
pub struct NewRadioStation<'a> {
    pub name: &'a str,
    pub stream_url: &'a str,
    pub home_page_url: Option<&'a str>,
}

/// Field-optional station update. Omitted fields keep their stored value;
/// `Some(None)` on `home_page_url` clears the homepage link.
#[derive(Clone, Debug, Default)]
pub struct RadioStationUpdate {
    pub name: Option<String>,
    pub stream_url: Option<String>,
    pub home_page_url: Option<Option<String>>,
}

#[derive(Debug, Error)]
pub enum RadioStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("radio station not found")]
    NotFound,
    #[error("radio station name must be non-blank and at most 500 characters")]
    InvalidName,
    #[error("radio station stream URL must be non-blank and at most 2000 characters")]
    InvalidStreamUrl,
    #[error("radio station homepage URL must be at most 2000 characters")]
    InvalidHomePageUrl,
}

fn is_valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= MAX_RADIO_NAME_CHARS
}

fn is_valid_stream_url(url: &str) -> bool {
    !url.trim().is_empty() && url.chars().count() <= MAX_RADIO_URL_CHARS
}

fn is_valid_home_page_url(url: &str) -> bool {
    url.chars().count() <= MAX_RADIO_URL_CHARS
}

/// Stateless entry point for radio station administration.
#[derive(Clone, Copy, Debug, Default)]
pub struct RadioStationStore;

impl RadioStationStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Bounded, paged listing ordered by `(name, public_api_id)` — the same
    /// deterministic ordering the OpenSubsonic list endpoint uses, so both
    /// surfaces present stations identically.
    pub async fn list(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RadioStation>, RadioStoreError> {
        Ok(sqlx::query_as::<_, RadioStation>(
            "SELECT id, public_api_id, name, stream_url, home_page_url, created_at, updated_at
             FROM opensubsonic_radio_stations
             ORDER BY name, public_api_id
             LIMIT $1 OFFSET $2",
        )
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?)
    }

    /// Loads one station by its immutable public id.
    pub async fn get_by_public_id(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<RadioStation>, RadioStoreError> {
        Ok(sqlx::query_as::<_, RadioStation>(
            "SELECT id, public_api_id, name, stream_url, home_page_url, created_at, updated_at
             FROM opensubsonic_radio_stations
             WHERE public_api_id = $1",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?)
    }

    /// Inserts a station after validating against the table's constraint
    /// bounds. Ids are fresh v7 UUIDs issued once, never regenerated.
    pub async fn create(
        &self,
        pool: &PgPool,
        station: &NewRadioStation<'_>,
        now: OffsetDateTime,
    ) -> Result<RadioStation, RadioStoreError> {
        if !is_valid_name(station.name) {
            return Err(RadioStoreError::InvalidName);
        }
        if !is_valid_stream_url(station.stream_url) {
            return Err(RadioStoreError::InvalidStreamUrl);
        }
        if let Some(home) = station.home_page_url
            && !is_valid_home_page_url(home)
        {
            return Err(RadioStoreError::InvalidHomePageUrl);
        }
        Ok(sqlx::query_as::<_, RadioStation>(
            "INSERT INTO opensubsonic_radio_stations
                 (id, public_api_id, name, stream_url, home_page_url, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $6)
             RETURNING id, public_api_id, name, stream_url, home_page_url, created_at, updated_at",
        )
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(station.name)
        .bind(station.stream_url)
        .bind(station.home_page_url)
        .bind(now)
        .fetch_one(pool)
        .await?)
    }

    /// Applies a field-optional update. Fetch-merge-write keeps the SQL static
    /// and lets the immutable-identity trigger stand as the backstop; unknown
    /// ids fail with [`RadioStoreError::NotFound`].
    pub async fn update(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
        update: &RadioStationUpdate,
        now: OffsetDateTime,
    ) -> Result<RadioStation, RadioStoreError> {
        let Some(existing) = self.get_by_public_id(pool, public_api_id).await? else {
            return Err(RadioStoreError::NotFound);
        };
        let name = update.name.clone().unwrap_or(existing.name);
        let stream_url = update.stream_url.clone().unwrap_or(existing.stream_url);
        let home_page_url = update
            .home_page_url
            .clone()
            .unwrap_or(existing.home_page_url);
        if !is_valid_name(&name) {
            return Err(RadioStoreError::InvalidName);
        }
        if !is_valid_stream_url(&stream_url) {
            return Err(RadioStoreError::InvalidStreamUrl);
        }
        if let Some(home) = &home_page_url
            && !is_valid_home_page_url(home)
        {
            return Err(RadioStoreError::InvalidHomePageUrl);
        }
        Ok(sqlx::query_as::<_, RadioStation>(
            "UPDATE opensubsonic_radio_stations
             SET name = $2, stream_url = $3, home_page_url = $4, updated_at = $5
             WHERE public_api_id = $1
             RETURNING id, public_api_id, name, stream_url, home_page_url, created_at, updated_at",
        )
        .bind(public_api_id)
        .bind(name)
        .bind(stream_url)
        .bind(home_page_url)
        .bind(now)
        .fetch_one(pool)
        .await?)
    }

    /// Deletes a station by public id. Returns whether a row was removed;
    /// stations are referenced by nothing else, so deletion is unconditional.
    pub async fn delete(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<bool, RadioStoreError> {
        let result =
            sqlx::query("DELETE FROM opensubsonic_radio_stations WHERE public_api_id = $1")
                .bind(public_api_id)
                .execute(pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }
}
