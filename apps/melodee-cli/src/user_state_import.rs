//! Portable user-state import (ADR-0200, ADR-0201, ADR-0203 through ADR-0205).
//!
//! Reads an export directory produced by
//! `scripts/export_legacy_user_state.py` and applies it to this server's native
//! user-state tables. The export is a versioned manifest plus NDJSON streams,
//! so a C# Melodee snapshot can be exported once and applied to any Rust
//! deployment — repeatedly — as the catalog grows.
//!
//! Non-destructive re-run contract:
//!
//! * **Nothing is ever deleted.** Rows missing from a later export are kept.
//! * **Metadata and flags are last-writer-wins by `updatedAt`.** An older
//!   export cannot downgrade a newer local change.
//! * **Play counts and last-played timestamps are monotonic maxima.**
//! * **Play history is additive** and idempotent by a derived event id, so a
//!   re-run with newly found plays adds only what is new.
//! * **Playlist items are additive.** When a playlist already has items, only
//!   tracks it does not yet contain are appended; existing order and local
//!   edits are preserved.
//! * **Memberships and bindings are additive.**
//!
//! Accounts resolve by public ID, saved links, or a unique normalized email from the trusted
//! export. The first applied email match binds the two public IDs for subsequent runs.
//! Live catalog items resolve by preserved or linked public IDs. Rebuilt tracks can also
//! resolve by a unique artist, release, and track title. Apply saves that association. Playlists
//! and smart playlists resolve by their public IDs. Missing catalog rows are skipped and reported
//! so the same export can be retried after more music arrives. Operators can
//! require a complete catalog with `--require-all-matches`.
//!
//! Domains that carry secrets or feed-dependent state (shares, Last.fm links,
//! social logins, OpenSubsonic app passwords, device profiles, podcast
//! subscriptions/episodes, user-authored radio, requests/comments) are listed
//! in the manifest's `referenceOnly` section with their counts and reasons so
//! nothing is silently dropped; use the `melodee-cli legacy import` path for
//! those.

mod identity_map;
use identity_map::IdentityMap;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

/// Format version this binary understands.
pub const SCHEMA_VERSION: u32 = 1;

const MANIFEST_NAME: &str = "manifest.json";
const USERS_NAME: &str = "users.ndjson";
const ENTITY_STATES_NAME: &str = "entity-states.ndjson";
const TRACK_PLAYS_NAME: &str = "track-plays.ndjson";
const PLAYLISTS_NAME: &str = "playlists.ndjson";
const PLAYLIST_ITEMS_NAME: &str = "playlist-items.ndjson";
const SMART_PLAYLISTS_NAME: &str = "smart-playlists.ndjson";
const BOOKMARKS_NAME: &str = "bookmarks.ndjson";
const GROUPS_NAME: &str = "groups.ndjson";
const GROUP_MEMBERS_NAME: &str = "group-members.ndjson";
const PLAYBACK_SETTINGS_NAME: &str = "playback-settings.ndjson";
const EQUALIZER_PRESETS_NAME: &str = "equalizer-presets.ndjson";

/// Every stream a schema-v1 export must contain.
const STREAMS: [&str; 11] = [
    USERS_NAME,
    ENTITY_STATES_NAME,
    TRACK_PLAYS_NAME,
    PLAYLISTS_NAME,
    PLAYLIST_ITEMS_NAME,
    SMART_PLAYLISTS_NAME,
    BOOKMARKS_NAME,
    GROUPS_NAME,
    GROUP_MEMBERS_NAME,
    PLAYBACK_SETTINGS_NAME,
    EQUALIZER_PRESETS_NAME,
];

const MAX_STREAM_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_STREAM_ROWS: u64 = 20_000_000;
const MAX_LINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_BATCH_SIZE: i64 = 10_000;
const MAX_REPORTED: usize = 50;
const MAX_AVATAR_BYTES: usize = 1024 * 1024;

/// Options controlling one import.
#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    pub dry_run: bool,
    pub allow_unmatched: bool,
    pub require_all_matches: bool,
    /// Create exported accounts that do not exist yet. This is the default:
    /// importing user data is meaningless if the accounts are rejected.
    pub create_users: bool,
    /// Keep the exported administrator/editor flags on **newly created**
    /// accounts. Off by default so an export cannot silently grant authority;
    /// existing accounts never change privilege.
    pub grant_privileged_roles: bool,
    pub batch_size: i64,
}

/// Machine-readable import report.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub schema_version: u32,
    pub dry_run: bool,
    pub generator: String,
    pub generated_at: String,
    pub users_in_export: usize,
    pub users_already_on_server: usize,
    pub users_linked: usize,
    pub users_matched_by_email: usize,
    pub users_requiring_review: usize,
    pub users_matched: usize,
    pub users_created: usize,
    /// Accounts a dry run would create.
    pub users_planned_created: usize,
    pub users_updated: usize,
    pub users_unchanged: usize,
    pub users_unmatched: Vec<String>,
    pub avatars_written: usize,
    pub entity_states_in_export: usize,
    pub exported_preferences: PreferenceCounts,
    pub matched_preferences: PreferenceCounts,
    pub tracks_matched_by_metadata: usize,
    pub tracks_with_ambiguous_matches: usize,
    pub superseded_preferences: PreferenceCounts,
    pub preferences_by_user: BTreeMap<Uuid, ListenerPreferences>,
    pub states_created: usize,
    pub states_updated: usize,
    pub states_unchanged: usize,
    pub states_unmatched: usize,
    pub plays_in_export: usize,
    pub plays_inserted: usize,
    pub plays_duplicate: usize,
    pub plays_unmatched: usize,
    pub playlists_in_export: usize,
    pub playlists_created: usize,
    pub playlists_updated: usize,
    pub playlist_items_in_export: usize,
    pub playlist_items_added: usize,
    pub playlist_items_duplicate: usize,
    pub playlist_items_unmatched: usize,
    pub smart_playlists_created: usize,
    pub smart_playlists_updated: usize,
    pub bookmarks_written: usize,
    pub bookmarks_unmatched: usize,
    pub groups_created: usize,
    pub group_members_added: usize,
    pub playback_settings_written: usize,
    pub equalizer_presets_written: usize,
    pub reference_only: BTreeMap<String, u64>,
    /// SHA-256 of the non-sensitive source facts, when the exporter recorded it.
    pub source_fingerprint: Option<String>,
    pub warnings: Vec<String>,
}

impl ImportSummary {
    fn warn(&mut self, message: String) {
        if self.warnings.len() < MAX_REPORTED {
            self.warnings.push(message);
        }
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerPreferences {
    username: String,
    exported: PreferenceCounts,
    matched: PreferenceCounts,
}

/// Each count represents one listener's preference for one catalog item.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceCounts {
    favorite_artists: usize,
    favorite_releases: usize,
    favorite_tracks: usize,
    dislikes: usize,
    ratings: usize,
    pins: usize,
}

impl PreferenceCounts {
    fn count(&mut self, row: &EntityStateRow) {
        self.add(
            row.entity,
            row.starred,
            row.hated,
            row.rating > 0,
            row.pinned,
        );
    }

    fn add(&mut self, kind: EntityKind, favorite: bool, dislike: bool, rated: bool, pinned: bool) {
        if favorite {
            match kind {
                EntityKind::Artist => self.favorite_artists += 1,
                EntityKind::Release => self.favorite_releases += 1,
                EntityKind::Track => self.favorite_tracks += 1,
            }
        }
        self.dislikes += usize::from(dislike);
        self.ratings += usize::from(rated);
        self.pins += usize::from(pinned);
    }

    fn favorites(&self) -> usize {
        self.favorite_artists + self.favorite_releases + self.favorite_tracks
    }
}

// ---------------------------------------------------------------------------
// Manifest and streams
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    generator: Generator,
    generated_at: String,
    /// Rows per stream file, keyed by file name.
    counts: BTreeMap<String, u64>,
    /// SHA-256 per stream file, keyed by file name.
    checksums: BTreeMap<String, String>,
    /// Domains intentionally not exported, with counts and reasons.
    #[serde(default)]
    reference_only: BTreeMap<String, ReferenceOnly>,
    /// Non-sensitive source provenance written by the exporter.
    #[serde(default)]
    source: Option<SourceInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceInfo {
    fingerprint: String,
    #[serde(default)]
    #[allow(dead_code)]
    facts: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Generator {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReferenceOnly {
    count: u64,
    reason: String,
}

fn parse_timestamp(value: &str, field: &str) -> Result<OffsetDateTime, String> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| format!("{field} {value:?} is not RFC 3339: {error}"))
}

fn parse_optional_timestamp(
    value: Option<&str>,
    field: &str,
) -> Result<Option<OffsetDateTime>, String> {
    value.map(|value| parse_timestamp(value, field)).transpose()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn read_stream(input: &Path, name: &str, expected_sha256: &str) -> Result<Vec<u8>, String> {
    let path = input.join(name);
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if metadata.len() > MAX_STREAM_BYTES {
        return Err(format!(
            "{} is {} bytes, above the {MAX_STREAM_BYTES} byte cap",
            path.display(),
            metadata.len()
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    std::fs::File::open(&path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(format!(
            "{name} checksum mismatch: manifest {expected_sha256}, file {actual}"
        ));
    }
    Ok(bytes)
}

fn parse_ndjson<T: for<'de> Deserialize<'de>>(name: &str, bytes: &[u8]) -> Result<Vec<T>, String> {
    let mut rows = Vec::new();
    for (index, line) in BufReader::new(bytes).lines().enumerate() {
        let line = line.map_err(|error| format!("{name} line {}: {error}", index + 1))?;
        if line.len() > MAX_LINE_BYTES {
            return Err(format!(
                "{name} line {} is above the {MAX_LINE_BYTES} byte cap",
                index + 1
            ));
        }
        if line.trim().is_empty() {
            continue;
        }
        let row = serde_json::from_str::<T>(&line)
            .map_err(|error| format!("{name} line {}: {error}", index + 1))?;
        rows.push(row);
        if u64::try_from(rows.len()).unwrap_or(u64::MAX) > MAX_STREAM_ROWS {
            return Err(format!(
                "{name} has more than {MAX_STREAM_ROWS} rows; split the export"
            ));
        }
    }
    Ok(rows)
}

fn expect_stream<T: for<'de> Deserialize<'de>>(
    input: &Path,
    manifest: &Manifest,
    name: &str,
) -> Result<Vec<T>, String> {
    let checksum = manifest
        .checksums
        .get(name)
        .ok_or_else(|| format!("manifest is missing the {name} checksum"))?;
    let bytes = read_stream(input, name, checksum)?;
    let rows = parse_ndjson::<T>(name, &bytes)?;
    let expected = manifest
        .counts
        .get(name)
        .ok_or_else(|| format!("manifest is missing the {name} count"))?;
    if u64::try_from(rows.len()).unwrap_or(u64::MAX) != *expected {
        return Err(format!(
            "{name} has {} rows but the manifest declares {expected}",
            rows.len()
        ));
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UserRow {
    public_id: Uuid,
    username: String,
    email: String,
    #[serde(default)]
    is_admin: bool,
    #[serde(default)]
    is_editor: bool,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    blacklisted: bool,
    #[serde(default)]
    preferred_language: Option<String>,
    #[serde(default)]
    preferred_theme: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    time_zone_id: Option<String>,
    #[serde(default)]
    credential: Option<CredentialRow>,
    #[serde(default)]
    avatar: Option<AvatarRow>,
    created_at: String,
    updated_at: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialRow {
    algorithm: String,
    hash: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AvatarRow {
    mime_type: String,
    base64: String,
    updated_at: String,
}

#[derive(Debug)]
struct UserRecord {
    row: UserRow,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    avatar: Option<(String, Vec<u8>, OffsetDateTime)>,
}

impl UserRecord {
    fn validated(row: UserRow) -> Result<Self, String> {
        if row.username.trim().is_empty() || row.username.chars().count() > 500 {
            return Err(format!("user {} has an invalid username", row.public_id));
        }
        if row.email.trim().is_empty() || row.email.chars().count() > 500 {
            return Err(format!("user {} has an invalid email", row.public_id));
        }
        if row
            .description
            .as_deref()
            .is_some_and(|value| value.chars().count() > 62_000)
        {
            return Err(format!(
                "user {} has an over-long description",
                row.public_id
            ));
        }
        if row
            .time_zone_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 64)
        {
            return Err(format!("user {} has an invalid timeZoneId", row.public_id));
        }
        if let Some(credential) = &row.credential
            && (credential.algorithm != "bcrypt" || credential.hash.trim().is_empty())
        {
            return Err(format!(
                "user {} has an unsupported credential; only portable bcrypt is accepted",
                row.public_id
            ));
        }
        let created_at = parse_timestamp(&row.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&row.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "user {} has createdAt after updatedAt",
                row.public_id
            ));
        }
        let avatar = match &row.avatar {
            Some(avatar) => {
                if avatar.mime_type != "image/png" {
                    return Err(format!(
                        "user {} avatar must be normalized image/png",
                        row.public_id
                    ));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(avatar.base64.as_bytes())
                    .map_err(|error| {
                        format!("user {} avatar is not base64: {error}", row.public_id)
                    })?;
                if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
                    return Err(format!(
                        "user {} avatar is outside 1..={MAX_AVATAR_BYTES} bytes",
                        row.public_id
                    ));
                }
                Some((
                    avatar.mime_type.clone(),
                    bytes,
                    parse_timestamp(&avatar.updated_at, "avatar.updatedAt")?,
                ))
            }
            None => None,
        };
        Ok(Self {
            row,
            created_at,
            updated_at,
            avatar,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum EntityKind {
    Artist,
    Release,
    Track,
}

impl EntityKind {
    const fn table(self) -> &'static str {
        match self {
            Self::Artist => "native_artist_user_states",
            Self::Release => "native_release_user_states",
            Self::Track => "native_track_user_states",
        }
    }

    const fn entity_column(self) -> &'static str {
        match self {
            Self::Artist => "artist_id",
            Self::Release => "release_id",
            Self::Track => "track_id",
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Release => "release",
            Self::Track => "track",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntityStateRow {
    user_public_id: Uuid,
    entity: EntityKind,
    entity_public_id: Uuid,
    #[serde(default)]
    starred: bool,
    #[serde(default)]
    hated: bool,
    #[serde(default)]
    rating: i16,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    play_count: i64,
    #[serde(default)]
    last_played_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct StateRecord {
    row: EntityStateRow,
    last_played_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl StateRecord {
    fn validated(row: EntityStateRow) -> Result<Self, String> {
        if !(0..=5).contains(&row.rating) {
            return Err(format!(
                "rating {} is outside 0..=5 for user {} {} {}",
                row.rating,
                row.user_public_id,
                row.entity.as_str(),
                row.entity_public_id
            ));
        }
        if row.play_count < 0 {
            return Err(format!(
                "playCount {} is negative for user {} {} {}",
                row.play_count,
                row.user_public_id,
                row.entity.as_str(),
                row.entity_public_id
            ));
        }
        let created_at = parse_timestamp(&row.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&row.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "createdAt is after updatedAt for user {} {} {}",
                row.user_public_id,
                row.entity.as_str(),
                row.entity_public_id
            ));
        }
        Ok(Self {
            last_played_at: parse_optional_timestamp(
                row.last_played_at.as_deref(),
                "lastPlayedAt",
            )?,
            created_at,
            updated_at,
            row,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrackPlayRow {
    user_public_id: Uuid,
    track_public_id: Uuid,
    played_at: String,
    #[serde(default)]
    duration_seconds: Option<i32>,
    #[serde(default = "default_true")]
    completed: bool,
}

#[derive(Debug)]
struct PlayRecord {
    row: TrackPlayRow,
    played_at: OffsetDateTime,
}

impl PlayRecord {
    fn validated(row: TrackPlayRow) -> Result<Self, String> {
        if row.duration_seconds.is_some_and(|seconds| seconds < 0) {
            return Err(format!(
                "durationSeconds is negative for user {} track {}",
                row.user_public_id, row.track_public_id
            ));
        }
        Ok(Self {
            played_at: parse_timestamp(&row.played_at, "playedAt")?,
            row,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlaylistRow {
    public_id: Uuid,
    owner_user_public_id: Uuid,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    is_public: bool,
    created_at: String,
    updated_at: String,
}

impl PlaylistRow {
    fn validated(&self) -> Result<(OffsetDateTime, OffsetDateTime), String> {
        if self.name.trim().is_empty() || self.name.chars().count() > 500 {
            return Err(format!(
                "playlist {} has an empty or over-long name",
                self.public_id
            ));
        }
        if self
            .description
            .as_deref()
            .is_some_and(|value| value.chars().count() > 10_000)
        {
            return Err(format!(
                "playlist {} has an over-long description",
                self.public_id
            ));
        }
        let created_at = parse_timestamp(&self.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&self.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "playlist {} has createdAt after updatedAt",
                self.public_id
            ));
        }
        Ok((created_at, updated_at))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlaylistItemRow {
    playlist_public_id: Uuid,
    position: i32,
    track_public_id: Uuid,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SmartPlaylistRow {
    public_id: Uuid,
    owner_user_public_id: Uuid,
    name: String,
    mql_query: String,
    entity_type: String,
    #[serde(default)]
    is_public: bool,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct SmartPlaylistRecord {
    row: SmartPlaylistRow,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl SmartPlaylistRecord {
    fn validated(row: SmartPlaylistRow) -> Result<Self, String> {
        if row.name.trim().is_empty() || row.name.chars().count() > 255 {
            return Err(format!(
                "smart playlist {} has an empty or over-long name",
                row.public_id
            ));
        }
        if row.mql_query.trim().is_empty() || row.mql_query.chars().count() > 4000 {
            return Err(format!(
                "smart playlist {} has an empty or over-long query",
                row.public_id
            ));
        }
        if !matches!(row.entity_type.as_str(), "songs" | "albums" | "artists") {
            return Err(format!(
                "smart playlist {} has unsupported entityType {:?}",
                row.public_id, row.entity_type
            ));
        }
        let created_at = parse_timestamp(&row.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&row.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "smart playlist {} has createdAt after updatedAt",
                row.public_id
            ));
        }
        Ok(Self {
            row,
            created_at,
            updated_at,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BookmarkRow {
    user_public_id: Uuid,
    track_public_id: Uuid,
    position: i64,
    #[serde(default)]
    comment: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct BookmarkRecord {
    row: BookmarkRow,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl BookmarkRecord {
    fn validated(row: BookmarkRow) -> Result<Self, String> {
        if row.position < 0 {
            return Err(format!(
                "bookmark for user {} track {} has a negative position",
                row.user_public_id, row.track_public_id
            ));
        }
        if row
            .comment
            .as_deref()
            .is_some_and(|comment| comment.chars().count() > 4000)
        {
            return Err(format!(
                "bookmark for user {} track {} has an over-long comment",
                row.user_public_id, row.track_public_id
            ));
        }
        let created_at = parse_timestamp(&row.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&row.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "bookmark for user {} track {} has createdAt after updatedAt",
                row.user_public_id, row.track_public_id
            ));
        }
        Ok(Self {
            row,
            created_at,
            updated_at,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GroupRow {
    public_id: Uuid,
    name: String,
    created_at: String,
    updated_at: String,
}

impl GroupRow {
    fn validated(&self) -> Result<(OffsetDateTime, OffsetDateTime), String> {
        if self.name.trim().is_empty() || self.name.chars().count() > 500 {
            return Err(format!(
                "group {} has an empty or over-long name",
                self.public_id
            ));
        }
        let created_at = parse_timestamp(&self.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&self.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "group {} has createdAt after updatedAt",
                self.public_id
            ));
        }
        Ok((created_at, updated_at))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GroupMemberRow {
    group_public_id: Uuid,
    user_public_id: Uuid,
    created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlaybackSettingsRow {
    user_public_id: Uuid,
    crossfade_duration: f64,
    gapless_playback: bool,
    volume_normalization: bool,
    replay_gain: String,
    audio_quality: String,
    #[serde(default)]
    equalizer_preset: Option<String>,
    #[serde(default)]
    last_used_device: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct PlaybackRecord {
    row: PlaybackSettingsRow,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl PlaybackRecord {
    fn validated(row: PlaybackSettingsRow) -> Result<Self, String> {
        if !(0.0..=30.0).contains(&row.crossfade_duration) {
            return Err(format!(
                "playback settings for user {} have a crossfade outside 0..=30",
                row.user_public_id
            ));
        }
        if !matches!(row.replay_gain.as_str(), "none" | "track" | "album") {
            return Err(format!(
                "playback settings for user {} have an unsupported replayGain {:?}",
                row.user_public_id, row.replay_gain
            ));
        }
        if !matches!(
            row.audio_quality.as_str(),
            "low" | "medium" | "high" | "lossless"
        ) {
            return Err(format!(
                "playback settings for user {} have an unsupported audioQuality {:?}",
                row.user_public_id, row.audio_quality
            ));
        }
        if row
            .equalizer_preset
            .as_deref()
            .is_some_and(|value| value.chars().count() > 500)
            || row
                .last_used_device
                .as_deref()
                .is_some_and(|value| value.chars().count() > 500)
        {
            return Err(format!(
                "playback settings for user {} have an over-long text field",
                row.user_public_id
            ));
        }
        let created_at = parse_timestamp(&row.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&row.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "playback settings for user {} have createdAt after updatedAt",
                row.user_public_id
            ));
        }
        Ok(Self {
            row,
            created_at,
            updated_at,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EqualizerPresetRow {
    public_id: Uuid,
    user_public_id: Uuid,
    name: String,
    bands: serde_json::Value,
    #[serde(default)]
    is_default: bool,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct EqualizerRecord {
    row: EqualizerPresetRow,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl EqualizerRecord {
    fn validated(row: EqualizerPresetRow) -> Result<Self, String> {
        if row.name.trim().is_empty() || row.name.chars().count() > 500 {
            return Err(format!(
                "equalizer preset {} has an empty or over-long name",
                row.public_id
            ));
        }
        let Some(bands) = row.bands.as_array() else {
            return Err(format!(
                "equalizer preset {} bands must be an array",
                row.public_id
            ));
        };
        if bands.is_empty() || bands.len() > 64 {
            return Err(format!(
                "equalizer preset {} has {} bands (1..=64 required)",
                row.public_id,
                bands.len()
            ));
        }
        let created_at = parse_timestamp(&row.created_at, "createdAt")?;
        let updated_at = parse_timestamp(&row.updated_at, "updatedAt")?;
        if created_at > updated_at {
            return Err(format!(
                "equalizer preset {} has createdAt after updatedAt",
                row.public_id
            ));
        }
        Ok(Self {
            row,
            created_at,
            updated_at,
        })
    }
}

// ---------------------------------------------------------------------------
// Export loading
// ---------------------------------------------------------------------------

/// Names as observed by the legacy exporter. Extra provider and edition evidence
/// stays in the portable stream for operator review.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogEvidence {
    #[serde(skip_serializing)]
    kind: EntityKind,
    public_id: Uuid,
    artist: Option<String>,
    release: Option<String>,
    title: Option<String>,
}

/// Fully validated export contents.
#[derive(Debug)]
pub struct Export {
    schema_version: u32,
    generator: String,
    generated_at: String,
    source_fingerprint: Option<String>,
    users: Vec<UserRecord>,
    track_evidence: Option<Vec<CatalogEvidence>>,
    states: Vec<StateRecord>,
    plays: Vec<PlayRecord>,
    playlists: Vec<(PlaylistRow, OffsetDateTime, OffsetDateTime)>,
    playlist_items: Vec<PlaylistItemRow>,
    smart_playlists: Vec<SmartPlaylistRecord>,
    bookmarks: Vec<BookmarkRecord>,
    groups: Vec<(GroupRow, OffsetDateTime, OffsetDateTime)>,
    group_members: Vec<(GroupMemberRow, OffsetDateTime)>,
    playback: Vec<PlaybackRecord>,
    equalizer: Vec<EqualizerRecord>,
    reference_only: BTreeMap<String, ReferenceOnly>,
}

fn reject_duplicates<T: Ord + std::fmt::Display>(
    label: &str,
    keys: impl Iterator<Item = T>,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for key in keys {
        let rendered = key.to_string();
        if !seen.insert(key) {
            return Err(format!("{label} repeats {rendered}"));
        }
    }
    Ok(())
}

/// Loads and validates an export directory without touching the database.
///
/// # Errors
/// Returns a message for a missing file, checksum/count mismatch, unsupported
/// schema version, or any invalid row.
pub fn load(input: &Path) -> Result<Export, String> {
    let manifest_path = input.join(MANIFEST_NAME);
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid {MANIFEST_NAME}: {error}"))?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "export schema version {} is not supported (this binary reads {SCHEMA_VERSION})",
            manifest.schema_version
        ));
    }
    if manifest.generator.name.trim().is_empty() || manifest.generator.version.trim().is_empty() {
        return Err("manifest generator name/version must be non-blank".to_owned());
    }
    parse_timestamp(&manifest.generated_at, "generatedAt")?;
    for stream in STREAMS {
        if !manifest.checksums.contains_key(stream) || !manifest.counts.contains_key(stream) {
            return Err(format!(
                "manifest is missing the {stream} checksum or count"
            ));
        }
    }

    let evidence = "catalog-evidence.ndjson";
    let track_evidence =
        if manifest.checksums.contains_key(evidence) || manifest.counts.contains_key(evidence) {
            let rows = expect_stream::<CatalogEvidence>(input, &manifest, evidence)?;
            let tracks: Vec<_> = rows
                .into_iter()
                .filter(|row| row.kind == EntityKind::Track)
                .collect();
            reject_duplicates(evidence, tracks.iter().map(|row| row.public_id))?;
            Some(tracks)
        } else {
            None
        };

    let users = expect_stream::<UserRow>(input, &manifest, USERS_NAME)?
        .into_iter()
        .map(UserRecord::validated)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicates(USERS_NAME, users.iter().map(|user| user.row.public_id))?;

    let states = expect_stream::<EntityStateRow>(input, &manifest, ENTITY_STATES_NAME)?
        .into_iter()
        .map(StateRecord::validated)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicates(
        ENTITY_STATES_NAME,
        states.iter().map(|state| {
            format!(
                "{}:{}:{}",
                state.row.user_public_id,
                state.row.entity.as_str(),
                state.row.entity_public_id
            )
        }),
    )?;

    let plays = expect_stream::<TrackPlayRow>(input, &manifest, TRACK_PLAYS_NAME)?
        .into_iter()
        .map(PlayRecord::validated)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicates(
        TRACK_PLAYS_NAME,
        plays.iter().map(|play| {
            format!(
                "{}:{}:{}",
                play.row.user_public_id, play.row.track_public_id, play.played_at
            )
        }),
    )?;

    let playlists = expect_stream::<PlaylistRow>(input, &manifest, PLAYLISTS_NAME)?
        .into_iter()
        .map(|row| {
            let (created, updated) = row.validated()?;
            Ok((row, created, updated))
        })
        .collect::<Result<Vec<_>, String>>()?;
    reject_duplicates(
        PLAYLISTS_NAME,
        playlists.iter().map(|(row, _, _)| row.public_id),
    )?;

    let playlist_items = expect_stream::<PlaylistItemRow>(input, &manifest, PLAYLIST_ITEMS_NAME)?;
    let playlist_ids = playlists
        .iter()
        .map(|(row, _, _)| row.public_id)
        .collect::<BTreeSet<_>>();
    reject_duplicates(
        PLAYLIST_ITEMS_NAME,
        playlist_items
            .iter()
            .map(|item| format!("{}:{}", item.playlist_public_id, item.position)),
    )?;
    for item in &playlist_items {
        if item.position < 0 {
            return Err(format!(
                "{PLAYLIST_ITEMS_NAME} has a negative position in playlist {}",
                item.playlist_public_id
            ));
        }
        if !playlist_ids.contains(&item.playlist_public_id) {
            return Err(format!(
                "{PLAYLIST_ITEMS_NAME} references playlist {} that is not in {PLAYLISTS_NAME}",
                item.playlist_public_id
            ));
        }
    }

    let smart_playlists =
        expect_stream::<SmartPlaylistRow>(input, &manifest, SMART_PLAYLISTS_NAME)?
            .into_iter()
            .map(SmartPlaylistRecord::validated)
            .collect::<Result<Vec<_>, _>>()?;
    reject_duplicates(
        SMART_PLAYLISTS_NAME,
        smart_playlists.iter().map(|row| row.row.public_id),
    )?;

    let bookmarks = expect_stream::<BookmarkRow>(input, &manifest, BOOKMARKS_NAME)?
        .into_iter()
        .map(BookmarkRecord::validated)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicates(
        BOOKMARKS_NAME,
        bookmarks
            .iter()
            .map(|row| format!("{}:{}", row.row.user_public_id, row.row.track_public_id)),
    )?;

    let groups = expect_stream::<GroupRow>(input, &manifest, GROUPS_NAME)?
        .into_iter()
        .map(|row| {
            let (created, updated) = row.validated()?;
            Ok((row, created, updated))
        })
        .collect::<Result<Vec<_>, String>>()?;
    reject_duplicates(GROUPS_NAME, groups.iter().map(|(row, _, _)| row.public_id))?;

    let group_members = expect_stream::<GroupMemberRow>(input, &manifest, GROUP_MEMBERS_NAME)?
        .into_iter()
        .map(|row| {
            let created = parse_timestamp(&row.created_at, "createdAt")?;
            Ok((row, created))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let group_ids = groups
        .iter()
        .map(|(row, _, _)| row.public_id)
        .collect::<BTreeSet<_>>();
    reject_duplicates(
        GROUP_MEMBERS_NAME,
        group_members
            .iter()
            .map(|(row, _)| format!("{}:{}", row.group_public_id, row.user_public_id)),
    )?;
    for (member, _) in &group_members {
        if !group_ids.contains(&member.group_public_id) {
            return Err(format!(
                "{GROUP_MEMBERS_NAME} references group {} that is not in {GROUPS_NAME}",
                member.group_public_id
            ));
        }
    }

    let playback = expect_stream::<PlaybackSettingsRow>(input, &manifest, PLAYBACK_SETTINGS_NAME)?
        .into_iter()
        .map(PlaybackRecord::validated)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicates(
        PLAYBACK_SETTINGS_NAME,
        playback.iter().map(|row| row.row.user_public_id),
    )?;

    let equalizer = expect_stream::<EqualizerPresetRow>(input, &manifest, EQUALIZER_PRESETS_NAME)?
        .into_iter()
        .map(EqualizerRecord::validated)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicates(
        EQUALIZER_PRESETS_NAME,
        equalizer.iter().map(|row| row.row.public_id),
    )?;

    Ok(Export {
        schema_version: manifest.schema_version,
        generator: format!("{} {}", manifest.generator.name, manifest.generator.version),
        generated_at: manifest.generated_at,
        source_fingerprint: manifest
            .source
            .as_ref()
            .map(|source| source.fingerprint.clone()),
        users,
        track_evidence,
        states,
        plays,
        playlists,
        playlist_items,
        smart_playlists,
        bookmarks,
        groups,
        group_members,
        playback,
        equalizer,
        reference_only: manifest.reference_only,
    })
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// Applies a validated export to the target database.
///
/// # Errors
/// Returns a message for unresolved users without `--allow-unmatched`, catalog
/// gaps with `--require-all-matches`, ownership conflicts, or database failures.
async fn apply(
    pool: &PgPool,
    export: &Export,
    options: ImportOptions,
    links: &mut IdentityMap,
) -> Result<ImportSummary, String> {
    if options.batch_size < 1 || options.batch_size > MAX_BATCH_SIZE {
        return Err(format!(
            "batch size {} is outside 1..={MAX_BATCH_SIZE}",
            options.batch_size
        ));
    }
    let batch = usize::try_from(options.batch_size).unwrap_or(1);
    let reference_notes = export
        .reference_only
        .iter()
        .map(|(domain, entry)| {
            format!(
                "reference-only {domain} ({} rows): {}",
                entry.count, entry.reason
            )
        })
        .collect::<Vec<_>>();
    let mut summary = ImportSummary {
        schema_version: export.schema_version,
        dry_run: options.dry_run,
        generator: export.generator.clone(),
        generated_at: export.generated_at.clone(),
        source_fingerprint: export.source_fingerprint.clone(),
        users_in_export: export.users.len(),
        entity_states_in_export: export.states.len(),
        plays_in_export: export.plays.len(),
        playlists_in_export: export.playlists.len(),
        playlist_items_in_export: export.playlist_items.len(),
        reference_only: export
            .reference_only
            .iter()
            .map(|(domain, entry)| (domain.clone(), entry.count))
            .collect(),
        ..ImportSummary::default()
    };
    for note in reference_notes {
        summary.warn(note);
    }

    let UserResolution {
        ids: mut users,
        missing: missing_users,
    } = resolve_users(pool, export, options, links, &mut summary).await?;
    if !summary.users_unmatched.is_empty() && !options.allow_unmatched {
        return Err(format!(
            "{} user(s) in the export do not exist on this server ({}); remove --no-create-users to create them, or pass --allow-unmatched to skip their data",
            summary.users_unmatched.len(),
            summary.users_unmatched.join(", ")
        ));
    }

    let mut entities = resolve_entities(pool, export, links).await?;
    match_track_metadata(pool, export, links, &mut entities, &mut summary).await?;
    if export.track_evidence.is_none()
        && export.states.iter().any(|state| {
            state.row.entity == EntityKind::Track
                && !entities.contains_key(&("track", state.row.entity_public_id))
        })
    {
        summary.warn("This export has no artist, release, or track titles. Use the export directory containing catalog-evidence.ndjson to match tracks by name.".into());
    }
    summary.preferences_by_user = export
        .users
        .iter()
        .map(|user| {
            (
                user.row.public_id,
                ListenerPreferences {
                    username: user.row.username.clone(),
                    ..Default::default()
                },
            )
        })
        .collect();
    for state in &export.states {
        let row = &state.row;
        let listener = summary
            .preferences_by_user
            .entry(row.user_public_id)
            .or_default();
        listener.exported.count(row);
        summary.exported_preferences.count(row);
        if users.contains_key(&row.user_public_id)
            && entities.contains_key(&(row.entity.as_str(), row.entity_public_id))
        {
            listener.matched.count(row);
            summary.matched_preferences.count(row);
        } else {
            summary.states_unmatched += 1;
        }
    }
    summary.superseded_preferences =
        superseded_preferences(pool, &export.states, &users, &entities, batch).await?;
    let track_ids = entities
        .iter()
        .filter_map(|(&(kind, source), &id)| (kind == "track").then_some((source, id)))
        .collect::<BTreeMap<_, _>>();
    summary.plays_unmatched = export
        .plays
        .iter()
        .filter(|play| {
            !users.contains_key(&play.row.user_public_id)
                || !track_ids.contains_key(&play.row.track_public_id)
        })
        .count();
    let matched_playlists = export
        .playlists
        .iter()
        .filter(|(row, _, _)| users.contains_key(&row.owner_user_public_id))
        .map(|(row, _, _)| row.public_id)
        .collect::<BTreeSet<_>>();
    summary.playlist_items_unmatched = export
        .playlist_items
        .iter()
        .filter(|item| {
            !matched_playlists.contains(&item.playlist_public_id)
                || !track_ids.contains_key(&item.track_public_id)
        })
        .count();
    summary.bookmarks_unmatched = export
        .bookmarks
        .iter()
        .filter(|bookmark| {
            !users.contains_key(&bookmark.row.user_public_id)
                || !track_ids.contains_key(&bookmark.row.track_public_id)
        })
        .count();
    let unresolved_rows = summary.states_unmatched
        + summary.plays_unmatched
        + summary.playlist_items_unmatched
        + summary.bookmarks_unmatched;
    if unresolved_rows > 0 && options.require_all_matches {
        return Err(format!(
            "{unresolved_rows} row(s) depend on users or music that are missing or not live, or on conflicting matches that need review. Review the suggested pairs or remove --require-all-matches to import ready rows"
        ));
    }
    if unresolved_rows > 0 {
        summary.warn(format!(
            "{unresolved_rows} row(s) skipped because their user or music is missing, inactive, or needs identity review. Re-run the same export after adding music to import newly matched rows."
        ));
    }

    if options.dry_run {
        summary.users_matched = users.len();
        summary.states_created = summary.entity_states_in_export - summary.states_unmatched;
        summary.plays_inserted = export
            .plays
            .iter()
            .filter(|play| {
                users.contains_key(&play.row.user_public_id)
                    && track_ids.contains_key(&play.row.track_public_id)
            })
            .count();
        summary.playlists_created = export
            .playlists
            .iter()
            .filter(|(row, _, _)| users.contains_key(&row.owner_user_public_id))
            .count();
        summary.playlist_items_added = export
            .playlist_items
            .iter()
            .filter(|item| {
                matched_playlists.contains(&item.playlist_public_id)
                    && track_ids.contains_key(&item.track_public_id)
            })
            .count();
        summary.bookmarks_written = export.bookmarks.len() - summary.bookmarks_unmatched;
        return Ok(summary);
    }

    // Complete identity preflight before any account or user-state writes, so
    // strict catalog validation cannot leave newly created accounts behind.
    links.save()?;
    for user in missing_users {
        let id = create_user(pool, user, options.grant_privileged_roles).await?;
        users.insert(user.row.public_id, id);
        summary.users_created += 1;
        if (user.row.is_admin || user.row.is_editor) && !options.grant_privileged_roles {
            summary.warn(format!(
                "user {} is an administrator/editor in the export; created unprivileged — pass --grant-privileged-roles or grant it in the Admin UI",
                user.row.username
            ));
        }
    }
    apply_users(pool, export, &users, links, &mut summary).await?;
    apply_groups(pool, export, &users, &mut summary).await?;
    apply_states(pool, export, &users, &entities, batch, &mut summary).await?;
    apply_plays(pool, export, &users, &track_ids, batch, &mut summary).await?;
    apply_playlists(pool, export, &users, &track_ids, batch, &mut summary).await?;
    apply_smart_playlists(pool, export, &users, batch, &mut summary).await?;
    apply_bookmarks(pool, export, &users, &track_ids, batch, &mut summary).await?;
    apply_preferences(pool, export, &users, batch, &mut summary).await?;
    Ok(summary)
}

struct UserResolution<'a> {
    ids: BTreeMap<Uuid, Uuid>,
    missing: Vec<&'a UserRecord>,
}

/// The C# exporter synthesizes an @import.local address for missing email.
fn usable_account_email(normalized: &str) -> bool {
    normalized.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && !(domain == "IMPORT.LOCAL"
                && local
                    .strip_prefix("MIGRATED-USER-")
                    .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())))
            && !normalized.chars().any(char::is_whitespace)
    })
}

async fn resolve_users<'a>(
    pool: &PgPool,
    export: &'a Export,
    options: ImportOptions,
    links: &mut IdentityMap,
    summary: &mut ImportSummary,
) -> Result<UserResolution<'a>, String> {
    let mut missing = Vec::new();
    let mut resolved = links
        .resolve(
            pool,
            "user",
            export.users.iter().map(|user| user.row.public_id).collect(),
            "SELECT public_api_id, id FROM native_users WHERE public_api_id = ANY($1)",
        )
        .await?;
    summary.users_linked = resolved
        .keys()
        .filter(|id| links.target("user", id).is_some())
        .count();
    summary.users_already_on_server = resolved.len() - summary.users_linked;
    let mut emails = BTreeMap::new();
    let mut names = BTreeMap::new();
    for user in &export.users {
        *emails
            .entry(user.row.email.trim().to_uppercase())
            .or_insert(0usize) += 1;
        *names
            .entry(user.row.username.trim().to_uppercase())
            .or_insert(0usize) += 1;
    }
    let targets: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT public_api_id, id, normalized_email, normalized_username FROM native_users
         WHERE normalized_email=ANY($1) OR normalized_username=ANY($2)",
    )
    .bind(emails.keys().collect::<Vec<_>>())
    .bind(names.keys().collect::<Vec<_>>())
    .fetch_all(pool)
    .await
    .map_err(|e| format!("account email lookup failed: {e}"))?;
    let by_email: BTreeMap<_, _> = targets
        .iter()
        .map(|(public, id, email, _)| (email, (*public, *id)))
        .collect();
    let by_name: BTreeMap<_, _> = targets
        .iter()
        .map(|(public, _, _, name)| (name, *public))
        .collect();
    let mut reserved = links.targets("user");
    reserved.extend(export.users.iter().map(|u| u.row.public_id));
    for user in &export.users {
        let source = user.row.public_id;
        if resolved.contains_key(&source) {
            continue;
        }
        let email = user.row.email.trim().to_uppercase();
        let name = user.row.username.trim().to_uppercase();
        let same_email = by_email.get(&email);
        let same_name = by_name.get(&name);
        if let Some(&(public, internal)) = same_email
            && emails[&email] == 1
            && usable_account_email(&email)
            && same_name.is_none_or(|other| *other == public)
            && reserved.insert(public)
        {
            links.link_email_match(source, public);
            resolved.insert(source, internal);
            summary.users_matched_by_email += 1;
            continue;
        }
        if same_email.is_some() || same_name.is_some() || emails[&email] > 1 || names[&name] > 1 {
            summary.users_requiring_review += 1;
            if !options.create_users && options.allow_unmatched {
                summary
                    .users_unmatched
                    .push(format!("{} ({source})", user.row.username));
            }
        } else if options.create_users {
            missing.push(user);
            resolved.insert(source, Uuid::now_v7());
        } else {
            summary
                .users_unmatched
                .push(format!("{} ({source})", user.row.username));
        }
    }
    if summary.users_requiring_review > 0 {
        let message = format!(
            "{} exported account(s) have a conflicting username, email, or saved identity. Review these accounts with scripts/reconcile_legacy_user_state.py --review and use the saved --identity-map.",
            summary.users_requiring_review
        );
        if options.require_all_matches
            || (!options.dry_run && (options.create_users || !options.allow_unmatched))
        {
            return Err(message);
        }
        summary.warn(message);
    }
    if options.dry_run {
        summary.users_planned_created = missing.len();
    }
    summary.users_matched = resolved.len();
    Ok(UserResolution {
        ids: resolved,
        missing,
    })
}

async fn create_user(
    pool: &PgPool,
    user: &UserRecord,
    grant_privileged_roles: bool,
) -> Result<Uuid, String> {
    let id = Uuid::now_v7();
    let username = user.row.username.trim().to_owned();
    let email = user.row.email.trim().to_owned();
    let password_hash = match &user.row.credential {
        Some(credential) => credential.hash.clone(),
        None => {
            // Unguessable placeholder credential: the user must reset their
            // password before signing in.
            let secret = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
            melodee_auth::hash_password(secret.as_bytes())
                .map_err(|error| format!("cannot hash placeholder credential: {error}"))?
        }
    };
    let stream_signing_key = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
    sqlx::query(
        "INSERT INTO native_users (
            id, public_api_id, username, normalized_username, email, normalized_email,
            password_hash, password_hash_algorithm, stream_signing_key,
            is_admin, is_editor, enabled, locked, blacklisted,
            preferred_language, preferred_theme, description, time_zone_id,
            auth_revision, created_at, updated_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,'bcrypt',$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,1,$18,$19)",
    )
    .bind(id)
    .bind(user.row.public_id)
    .bind(&username)
    .bind(username.to_uppercase())
    .bind(&email)
    .bind(email.to_uppercase())
    .bind(&password_hash)
    .bind(&stream_signing_key)
    .bind(grant_privileged_roles && user.row.is_admin)
    .bind(grant_privileged_roles && user.row.is_editor)
    .bind(user.row.enabled)
    .bind(user.row.locked)
    .bind(user.row.blacklisted)
    .bind(user.row.preferred_language.as_deref())
    .bind(user.row.preferred_theme.as_deref())
    .bind(user.row.description.as_deref())
    .bind(user.row.time_zone_id.as_deref().unwrap_or("UTC"))
    .bind(user.created_at)
    .bind(user.updated_at)
    .execute(pool)
    .await
    .map_err(|error| format!("cannot create user {}: {error}", user.row.username))?;
    Ok(id)
}

async fn apply_users(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    links: &IdentityMap,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    for user in &export.users {
        let Some(user_id) = users.get(&user.row.public_id) else {
            continue;
        };
        // A link authorizes attaching listener data, not replacing an existing login or avatar.
        if links.target("user", &user.row.public_id).is_some() {
            continue;
        }
        if user.row.credential.is_some() {
            summary.warn(format!(
                "user {} already exists; its credentials and privileges are kept (export credential is reference-only)",
                user.row.username
            ));
        }
        let applied = sqlx::query_scalar::<_, i64>(
            "UPDATE native_users SET
                preferred_language = CASE WHEN $2 >= updated_at THEN $3 ELSE preferred_language END,
                preferred_theme = CASE WHEN $2 >= updated_at THEN $4 ELSE preferred_theme END,
                enabled = CASE WHEN $2 >= updated_at THEN $5 ELSE enabled END,
                locked = CASE WHEN $2 >= updated_at THEN $6 ELSE locked END,
                blacklisted = CASE WHEN $2 >= updated_at THEN $7 ELSE blacklisted END,
                email = CASE WHEN $2 >= updated_at THEN $8 ELSE email END,
                normalized_email = CASE WHEN $2 >= updated_at THEN $9 ELSE normalized_email END,
                description = CASE WHEN $2 >= updated_at THEN $10 ELSE description END,
                time_zone_id = CASE WHEN $2 >= updated_at THEN COALESCE($11, time_zone_id) ELSE time_zone_id END,
                updated_at = GREATEST(updated_at, $2)
              WHERE id = $1 AND $2 > updated_at
              RETURNING auth_revision",
        )
        .bind(user_id)
        .bind(user.updated_at)
        .bind(user.row.preferred_language.as_deref())
        .bind(user.row.preferred_theme.as_deref())
        .bind(user.row.enabled)
        .bind(user.row.locked)
        .bind(user.row.blacklisted)
        .bind(user.row.email.trim())
        .bind(user.row.email.trim().to_uppercase())
        .bind(user.row.description.as_deref())
        .bind(user.row.time_zone_id.as_deref())
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("cannot update user {}: {error}", user.row.username))?;
        if applied.is_some() {
            summary.users_updated += 1;
        } else {
            summary.users_unchanged += 1;
        }
        if let Some((mime, bytes, updated_at)) = &user.avatar {
            let written = sqlx::query(
                "INSERT INTO native_user_avatars (user_id, content_type, asset_key, updated_at)
                 VALUES ($1,$2,$3,$4)
                 ON CONFLICT (user_id) DO UPDATE SET
                    content_type = EXCLUDED.content_type,
                    asset_key = EXCLUDED.asset_key,
                    updated_at = EXCLUDED.updated_at
                 WHERE native_user_avatars.updated_at <= EXCLUDED.updated_at",
            )
            .bind(user_id)
            .bind(mime)
            .bind(
                melodee_persistence::asset_files::put(pool, bytes)
                    .await
                    .map_err(|e| format!("avatar file write failed: {e}"))?,
            )
            .bind(updated_at)
            .execute(pool)
            .await
            .map_err(|error| format!("cannot write avatar for {}: {error}", user.row.username))?
            .rows_affected();
            summary.avatars_written += usize::try_from(written).unwrap_or(0);
        }
    }
    Ok(())
}

async fn apply_groups(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    let mut group_ids = BTreeMap::new();
    for (group, created_at, updated_at) in &export.groups {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM native_library_access_groups WHERE public_api_id = $1",
        )
        .bind(group.public_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("group lookup failed: {error}"))?;
        let id = match existing {
            Some(id) => {
                sqlx::query(
                    "UPDATE native_library_access_groups SET name=$2, updated_at=GREATEST(updated_at,$3)
                      WHERE id=$1",
                )
                .bind(id)
                .bind(group.name.trim())
                .bind(updated_at)
                .execute(pool)
                .await
                .map_err(|error| format!("group update failed: {error}"))?;
                id
            }
            None => {
                let id = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO native_library_access_groups (id, public_api_id, name, created_at, updated_at)
                     VALUES ($1,$2,$3,$4,$5)",
                )
                .bind(id)
                .bind(group.public_id)
                .bind(group.name.trim())
                .bind(created_at)
                .bind(updated_at)
                .execute(pool)
                .await
                .map_err(|error| format!("group insert failed: {error}"))?;
                summary.groups_created += 1;
                id
            }
        };
        group_ids.insert(group.public_id, id);
    }
    for (member, created_at) in &export.group_members {
        let (Some(group_id), Some(user_id)) = (
            group_ids.get(&member.group_public_id),
            users.get(&member.user_public_id),
        ) else {
            continue;
        };
        let added = sqlx::query(
            "INSERT INTO native_user_library_groups (user_id, group_id, created_at)
             VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(group_id)
        .bind(created_at)
        .execute(pool)
        .await
        .map_err(|error| format!("group membership insert failed: {error}"))?
        .rows_affected();
        summary.group_members_added += usize::try_from(added).unwrap_or(0);
    }
    Ok(())
}

/// Distinguish a resolved legacy preference from one that a listener has since
/// changed. Use the same timestamp precedence as the upsert, including ties.
async fn superseded_preferences(
    pool: &PgPool,
    states: &[StateRecord],
    users: &BTreeMap<Uuid, Uuid>,
    entities: &BTreeMap<(&'static str, Uuid), Uuid>,
    batch: usize,
) -> Result<PreferenceCounts, String> {
    let mut counts = PreferenceCounts::default();
    for kind in [EntityKind::Artist, EntityKind::Release, EntityKind::Track] {
        let matched = states
            .iter()
            .filter(|s| s.row.entity == kind)
            .filter_map(|s| {
                Some((
                    users.get(&s.row.user_public_id)?,
                    entities.get(&(kind.as_str(), s.row.entity_public_id))?,
                    s,
                ))
            })
            .collect::<Vec<_>>();
        let sql = format!(
            "SELECT keys.ord::bigint, s.starred, s.hated, s.rating, s.pinned, s.updated_at
            FROM unnest($1::uuid[], $2::uuid[]) WITH ORDINALITY AS keys(uid, entity, ord)
            JOIN {} s ON s.user_id=keys.uid AND s.{}=keys.entity",
            kind.table(),
            kind.entity_column()
        );
        for chunk in matched.chunks(batch) {
            let (uids, ids): (Vec<_>, Vec<_>) = chunk
                .iter()
                .map(|(user, entity, _)| (**user, **entity))
                .unzip();
            let local: Vec<(i64, bool, bool, i16, bool, OffsetDateTime)> =
                sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
                    .bind(uids)
                    .bind(ids)
                    .fetch_all(pool)
                    .await
                    .map_err(|e| format!("cannot compare existing preferences: {e}"))?;
            for (ord, starred, hated, rating, pinned, modified) in local {
                let index = usize::try_from(ord - 1).map_err(|e| e.to_string())?;
                let (_, _, state) = chunk
                    .get(index)
                    .ok_or("invalid catalog preference position")?;
                if modified >= state.updated_at {
                    let r = &state.row;
                    counts.add(
                        kind,
                        r.starred && !starred,
                        r.hated && !hated,
                        r.rating > 0 && r.rating != rating,
                        r.pinned && !pinned,
                    );
                }
            }
        }
    }
    Ok(counts)
}

async fn apply_states(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    entities: &BTreeMap<(&'static str, Uuid), Uuid>,
    batch: usize,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    let resolved = export
        .states
        .iter()
        .filter_map(|state| {
            let user_id = *users.get(&state.row.user_public_id)?;
            let entity_id =
                *entities.get(&(state.row.entity.as_str(), state.row.entity_public_id))?;
            Some((user_id, entity_id, state))
        })
        .collect::<Vec<_>>();
    for chunk in resolved.chunks(batch) {
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        for (user_id, entity_id, state) in chunk {
            // Flags require a strictly newer export; play maxima merge even
            // from an older export. The guard keeps identical re-runs a no-op.
            let sql = format!(
                "INSERT INTO {table} (user_id, {column}, starred, hated, rating, pinned,
                                      play_count, last_played_at, created_at, updated_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                 ON CONFLICT (user_id, {column}) DO UPDATE SET
                    starred = CASE WHEN EXCLUDED.updated_at > {table}.updated_at
                                   THEN EXCLUDED.starred ELSE {table}.starred END,
                    hated = CASE WHEN EXCLUDED.updated_at > {table}.updated_at
                                 THEN EXCLUDED.hated ELSE {table}.hated END,
                    rating = CASE WHEN EXCLUDED.updated_at > {table}.updated_at
                                  THEN EXCLUDED.rating ELSE {table}.rating END,
                    pinned = CASE WHEN EXCLUDED.updated_at > {table}.updated_at
                                  THEN EXCLUDED.pinned ELSE {table}.pinned END,
                    play_count = GREATEST({table}.play_count, EXCLUDED.play_count),
                    last_played_at = GREATEST({table}.last_played_at, EXCLUDED.last_played_at),
                    updated_at = GREATEST({table}.updated_at, EXCLUDED.updated_at)
                 WHERE EXCLUDED.updated_at > {table}.updated_at
                     OR EXCLUDED.play_count > {table}.play_count
                     OR (EXCLUDED.last_played_at IS NOT NULL
                         AND ({table}.last_played_at IS NULL
                              OR EXCLUDED.last_played_at > {table}.last_played_at))
                 RETURNING (xmax = 0) AS inserted",
                table = state.row.entity.table(),
                column = state.row.entity.entity_column(),
            );
            let outcome = sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(sql))
                .bind(user_id)
                .bind(entity_id)
                .bind(state.row.starred)
                .bind(state.row.hated)
                .bind(state.row.rating)
                .bind(state.row.pinned)
                .bind(state.row.play_count)
                .bind(state.last_played_at)
                .bind(state.created_at)
                .bind(state.updated_at)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| format!("state write failed: {error}"))?;
            match outcome {
                Some(true) => summary.states_created += 1,
                Some(false) => summary.states_updated += 1,
                None => summary.states_unchanged += 1,
            }
        }
        tx.commit().await.map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn play_event_id(play: &PlayRecord) -> String {
    let digest = Sha256::digest(
        format!(
            "{}|{}|{}",
            play.row.user_public_id, play.row.track_public_id, play.played_at
        )
        .as_bytes(),
    );
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("portable-play-{}", &hex[..32])
}

/// Prelocks the complete bounded play batch before inserting any event. The
/// recommendation statement trigger locks candidate rows only after the whole
/// batch is inserted, keeping Track, event-key and candidate lock order aligned
/// with a concurrent scrobble.
pub(crate) async fn lock_play_tracks(
    connection: &mut sqlx::PgConnection,
    track_ids: &[Uuid],
) -> Result<(), String> {
    sqlx::query("SELECT id FROM tracks WHERE id = ANY($1) ORDER BY id FOR NO KEY UPDATE")
        .bind(track_ids)
        .execute(connection)
        .await
        .map_err(|error| format!("play history track lock failed: {error}"))?;
    Ok(())
}

async fn apply_plays(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    track_ids: &BTreeMap<Uuid, Uuid>,
    batch: usize,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    let resolved = export
        .plays
        .iter()
        .filter_map(|play| {
            let user_id = *users.get(&play.row.user_public_id)?;
            let track_id = *track_ids.get(&play.row.track_public_id)?;
            Some((user_id, track_id, play))
        })
        .collect::<Vec<_>>();
    for chunk in resolved.chunks(batch) {
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        let user_ids: Vec<_> = chunk.iter().map(|(user_id, _, _)| *user_id).collect();
        let track_ids: Vec<_> = chunk.iter().map(|(_, track_id, _)| *track_id).collect();
        let event_ids: Vec<_> = chunk
            .iter()
            .map(|(_, _, play)| play_event_id(play))
            .collect();
        let played_at: Vec<_> = chunk.iter().map(|(_, _, play)| play.played_at).collect();
        let durations: Vec<_> = chunk
            .iter()
            .map(|(_, _, play)| play.row.duration_seconds)
            .collect();
        let completed: Vec<_> = chunk
            .iter()
            .map(|(_, _, play)| play.row.completed)
            .collect();
        lock_play_tracks(&mut tx, &track_ids).await?;
        // Stable input ordinality retains the former first-input-wins behavior
        // for duplicates while ordering unique-key acquisition across imports.
        let inserted = sqlx::query(
            "INSERT INTO native_track_plays (
                 id, user_id, track_id, client_event_id, played_at, submitted_at,
                 duration_seconds, completed
             )
             SELECT uuidv7(), user_id, track_id, event_id, played_at, played_at,
                    duration, completed
             FROM unnest($1::uuid[], $2::uuid[], $3::text[], $4::timestamptz[],
                         $5::integer[], $6::boolean[]) WITH ORDINALITY
                  AS input(user_id, track_id, event_id, played_at, duration, completed, ordinal)
             ORDER BY user_id, event_id, ordinal
             ON CONFLICT (user_id, client_event_id) DO NOTHING",
        )
        .bind(&user_ids)
        .bind(&track_ids)
        .bind(&event_ids)
        .bind(&played_at)
        .bind(&durations)
        .bind(&completed)
        .execute(&mut *tx)
        .await
        .map_err(|error| format!("play history write failed: {error}"))?
        .rows_affected();
        let inserted = usize::try_from(inserted)
            .map_err(|_| "play history write count overflow".to_owned())?;
        summary.plays_inserted += inserted;
        summary.plays_duplicate += chunk.len() - inserted;
        tx.commit().await.map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn apply_playlists(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    track_ids: &BTreeMap<Uuid, Uuid>,
    batch: usize,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    let mut playlist_ids = BTreeMap::new();
    for chunk in export.playlists.chunks(batch) {
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        for (playlist, created_at, updated_at) in chunk {
            let Some(owner_id) = users.get(&playlist.owner_user_public_id) else {
                summary.warn(format!(
                    "playlist {} skipped: owner {} is not on this server",
                    playlist.public_id, playlist.owner_user_public_id
                ));
                continue;
            };
            let existing: Option<(Uuid, Uuid)> = sqlx::query_as(
                "SELECT id, owner_user_id FROM native_playlists WHERE public_api_id=$1 FOR UPDATE",
            )
            .bind(playlist.public_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| format!("playlist lookup failed: {error}"))?;
            let id = match existing {
                Some((id, existing_owner)) => {
                    if existing_owner != *owner_id {
                        return Err(format!(
                            "playlist {} already exists with a different owner; refusing to reassign it",
                            playlist.public_id
                        ));
                    }
                    sqlx::query(
                        "UPDATE native_playlists SET
                            name = CASE WHEN $2 >= updated_at THEN $3 ELSE name END,
                            description = CASE WHEN $2 >= updated_at THEN $4 ELSE description END,
                            is_public = CASE WHEN $2 >= updated_at THEN $5 ELSE is_public END,
                            revision = CASE WHEN $2 >= updated_at THEN revision + 1 ELSE revision END,
                            updated_at = GREATEST(updated_at, $2)
                          WHERE id = $1 AND $2 > updated_at",
                    )
                    .bind(id)
                    .bind(updated_at)
                    .bind(playlist.name.trim())
                    .bind(playlist.description.as_deref())
                    .bind(playlist.is_public)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| format!("playlist update failed: {error}"))?;
                    summary.playlists_updated += 1;
                    id
                }
                None => {
                    let id = Uuid::now_v7();
                    sqlx::query(
                        "INSERT INTO native_playlists (
                            id, public_api_id, owner_user_id, name, description,
                            is_public, revision, created_at, updated_at
                         ) VALUES ($1,$2,$3,$4,$5,$6,1,$7,$8)",
                    )
                    .bind(id)
                    .bind(playlist.public_id)
                    .bind(owner_id)
                    .bind(playlist.name.trim())
                    .bind(playlist.description.as_deref())
                    .bind(playlist.is_public)
                    .bind(created_at)
                    .bind(updated_at)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| format!("playlist insert failed: {error}"))?;
                    summary.playlists_created += 1;
                    id
                }
            };
            playlist_ids.insert(playlist.public_id, (id, *owner_id));
        }
        tx.commit().await.map_err(|error| error.to_string())?;
    }

    let mut items_by_playlist = BTreeMap::<Uuid, Vec<&PlaylistItemRow>>::new();
    for item in &export.playlist_items {
        items_by_playlist
            .entry(item.playlist_public_id)
            .or_default()
            .push(item);
    }
    for (playlist_public_id, items) in &items_by_playlist {
        let Some((playlist_id, owner_id)) = playlist_ids.get(playlist_public_id) else {
            continue;
        };
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        let existing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM native_playlist_items WHERE playlist_id=$1")
                .bind(playlist_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|error| format!("playlist item count failed: {error}"))?;
        // Non-destructive merge: a playlist that already has local items only
        // receives the tracks it does not contain, appended in export order.
        let mut next_position = if existing == 0 {
            0
        } else {
            sqlx::query_scalar::<_, Option<i32>>(
                "SELECT max(position) FROM native_playlist_items WHERE playlist_id=$1",
            )
            .bind(playlist_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| format!("playlist position read failed: {error}"))?
            .unwrap_or(-1)
                + 1
        };
        for item in items {
            let Some(track_id) = track_ids.get(&item.track_public_id) else {
                continue;
            };
            let already: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM native_playlist_items
                                 WHERE playlist_id=$1 AND track_id=$2)",
            )
            .bind(playlist_id)
            .bind(track_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| format!("playlist item lookup failed: {error}"))?;
            if already {
                summary.playlist_items_duplicate += 1;
                continue;
            }
            let position = if existing == 0 {
                item.position
            } else {
                next_position
            };
            sqlx::query(
                "INSERT INTO native_playlist_items (
                    id, playlist_id, track_id, position, added_by_user_id, created_at
                 ) VALUES ($1,$2,$3,$4,$5,$6)",
            )
            .bind(Uuid::now_v7())
            .bind(playlist_id)
            .bind(track_id)
            .bind(position)
            .bind(owner_id)
            .bind(OffsetDateTime::now_utc())
            .execute(&mut *tx)
            .await
            .map_err(|error| format!("playlist item insert failed: {error}"))?;
            next_position = position + 1;
            summary.playlist_items_added += 1;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn referenced_entities(export: &Export, entity: EntityKind) -> BTreeSet<Uuid> {
    let mut ids: BTreeSet<Uuid> = export
        .states
        .iter()
        .filter(|state| state.row.entity == entity)
        .map(|state| state.row.entity_public_id)
        .collect();
    if entity == EntityKind::Track {
        ids.extend(export.plays.iter().map(|play| play.row.track_public_id));
        ids.extend(export.playlist_items.iter().map(|row| row.track_public_id));
        ids.extend(
            export
                .bookmarks
                .iter()
                .map(|bookmark| bookmark.row.track_public_id),
        );
    }
    ids
}

#[derive(sqlx::FromRow)]
struct TrackMatch {
    source: Uuid,
    target: Option<Uuid>,
    internal_id: Option<Uuid>,
    source_count: i64,
    target_count: i64,
}

async fn match_track_metadata(
    pool: &PgPool,
    export: &Export,
    links: &mut IdentityMap,
    entities: &mut BTreeMap<(&'static str, Uuid), Uuid>,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    let Some(evidence) = &export.track_evidence else {
        return Ok(());
    };
    let originals = referenced_entities(export, EntityKind::Track);
    let evidence: Vec<_> = evidence
        .iter()
        .filter(|row| originals.contains(&row.public_id))
        .collect();
    if evidence.is_empty() {
        return Ok(());
    }
    let reserved = links.targets("track");
    let rows: Vec<TrackMatch> =
        sqlx::query_as(include_str!("user_state_import/track_metadata.sql"))
            .bind(sqlx::types::Json(evidence))
            .fetch_all(pool)
            .await
            .map_err(|error| {
                format!("cannot compare exported track titles with the live catalog: {error}")
            })?;
    for row in rows {
        let source = row.source;
        if entities.contains_key(&("track", source)) || links.target("track", &source).is_some() {
            continue;
        }
        match (
            row.source_count,
            row.target_count,
            row.target,
            row.internal_id,
        ) {
            (1, 1, Some(public), Some(id))
                if !reserved.contains(&public) && !originals.contains(&public) =>
            {
                links.link_track_match(source, public);
                entities.insert(("track", source), id);
                summary.tracks_matched_by_metadata += 1;
            }
            _ => summary.tracks_with_ambiguous_matches += 1,
        }
    }
    Ok(())
}

async fn resolve_entities(
    pool: &PgPool,
    export: &Export,
    links: &IdentityMap,
) -> Result<BTreeMap<(&'static str, Uuid), Uuid>, String> {
    let mut resolved = BTreeMap::new();
    for entity in [EntityKind::Artist, EntityKind::Release, EntityKind::Track] {
        let ids = referenced_entities(export, entity);
        let sql = match entity {
            EntityKind::Artist => {
                "SELECT public_api_id, id FROM artists
                  WHERE public_api_id = ANY($1) AND status IN ('active','verified')"
            }
            EntityKind::Release => {
                "SELECT public_api_id, id FROM releases
                  WHERE public_api_id = ANY($1) AND status = 'active'"
            }
            EntityKind::Track => {
                "SELECT track.public_api_id, track.id FROM tracks track
                   JOIN releases release ON release.id = track.release_id
                  WHERE track.public_api_id = ANY($1) AND release.status = 'active'"
            }
        };
        for (public_id, id) in links.resolve(pool, entity.as_str(), ids, sql).await? {
            resolved.insert((entity.as_str(), public_id), id);
        }
    }
    Ok(resolved)
}

async fn apply_smart_playlists(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    batch: usize,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    for chunk in export.smart_playlists.chunks(batch) {
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        for playlist in chunk {
            let Some(owner_id) = users.get(&playlist.row.owner_user_public_id) else {
                summary.warn(format!(
                    "smart playlist {} skipped: owner {} is not on this server",
                    playlist.row.public_id, playlist.row.owner_user_public_id
                ));
                continue;
            };
            let existing: Option<(Uuid, Uuid)> = sqlx::query_as(
                "SELECT id, owner_user_id FROM native_smart_playlists
                  WHERE public_api_id=$1 FOR UPDATE",
            )
            .bind(playlist.row.public_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| format!("smart playlist lookup failed: {error}"))?;
            match existing {
                Some((id, existing_owner)) => {
                    if existing_owner != *owner_id {
                        return Err(format!(
                            "smart playlist {} already exists with a different owner; refusing to reassign it",
                            playlist.row.public_id
                        ));
                    }
                    sqlx::query(
                        "UPDATE native_smart_playlists SET
                            name = CASE WHEN $2 >= updated_at THEN $3 ELSE name END,
                            mql_query = CASE WHEN $2 >= updated_at THEN $4 ELSE mql_query END,
                            normalized_query = CASE WHEN $2 >= updated_at THEN NULL ELSE normalized_query END,
                            entity_type = CASE WHEN $2 >= updated_at THEN $5 ELSE entity_type END,
                            is_public = CASE WHEN $2 >= updated_at THEN $6 ELSE is_public END,
                            revision = CASE WHEN $2 >= updated_at THEN revision + 1 ELSE revision END,
                            updated_at = GREATEST(updated_at, $2)
                          WHERE id = $1 AND $2 > updated_at",
                    )
                    .bind(id)
                    .bind(playlist.updated_at)
                    .bind(playlist.row.name.trim())
                    .bind(playlist.row.mql_query.trim())
                    .bind(&playlist.row.entity_type)
                    .bind(playlist.row.is_public)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| format!("smart playlist update failed: {error}"))?;
                    summary.smart_playlists_updated += 1;
                }
                None => {
                    sqlx::query(
                        "INSERT INTO native_smart_playlists (
                            id, public_api_id, owner_user_id, name, mql_query, entity_type,
                            last_result_count, is_public, revision, created_at, updated_at
                         ) VALUES ($1,$2,$3,$4,$5,$6,0,$7,1,$8,$9)",
                    )
                    .bind(Uuid::now_v7())
                    .bind(playlist.row.public_id)
                    .bind(owner_id)
                    .bind(playlist.row.name.trim())
                    .bind(playlist.row.mql_query.trim())
                    .bind(&playlist.row.entity_type)
                    .bind(playlist.row.is_public)
                    .bind(playlist.created_at)
                    .bind(playlist.updated_at)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| format!("smart playlist insert failed: {error}"))?;
                    summary.smart_playlists_created += 1;
                }
            }
        }
        tx.commit().await.map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn apply_bookmarks(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    track_ids: &BTreeMap<Uuid, Uuid>,
    batch: usize,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    let resolved = export
        .bookmarks
        .iter()
        .filter_map(|bookmark| {
            let user_id = *users.get(&bookmark.row.user_public_id)?;
            let track_id = *track_ids.get(&bookmark.row.track_public_id)?;
            Some((user_id, track_id, bookmark))
        })
        .collect::<Vec<_>>();
    for chunk in resolved.chunks(batch) {
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        for (user_id, track_id, bookmark) in chunk {
            sqlx::query(
                "INSERT INTO opensubsonic_bookmarks (
                    user_id, track_id, position, comment, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6)
                 ON CONFLICT (user_id, track_id) DO UPDATE SET
                    position = CASE WHEN EXCLUDED.updated_at >= opensubsonic_bookmarks.updated_at
                                    THEN EXCLUDED.position ELSE opensubsonic_bookmarks.position END,
                    comment = CASE WHEN EXCLUDED.updated_at >= opensubsonic_bookmarks.updated_at
                                   THEN EXCLUDED.comment ELSE opensubsonic_bookmarks.comment END,
                    updated_at = GREATEST(opensubsonic_bookmarks.updated_at, EXCLUDED.updated_at)",
            )
            .bind(user_id)
            .bind(track_id)
            .bind(bookmark.row.position)
            .bind(bookmark.row.comment.as_deref())
            .bind(bookmark.created_at)
            .bind(bookmark.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(|error| format!("bookmark write failed: {error}"))?;
            summary.bookmarks_written += 1;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn apply_preferences(
    pool: &PgPool,
    export: &Export,
    users: &BTreeMap<Uuid, Uuid>,
    batch: usize,
    summary: &mut ImportSummary,
) -> Result<(), String> {
    for chunk in export.playback.chunks(batch) {
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        for settings in chunk {
            let Some(user_id) = users.get(&settings.row.user_public_id) else {
                continue;
            };
            sqlx::query(
                "INSERT INTO native_playback_settings (
                    user_id, crossfade_duration, gapless_playback, volume_normalization,
                    replay_gain, audio_quality, equalizer_preset, last_used_device,
                    created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                 ON CONFLICT (user_id) DO UPDATE SET
                    crossfade_duration = CASE WHEN EXCLUDED.updated_at >= native_playback_settings.updated_at
                                              THEN EXCLUDED.crossfade_duration ELSE native_playback_settings.crossfade_duration END,
                    gapless_playback = CASE WHEN EXCLUDED.updated_at >= native_playback_settings.updated_at
                                            THEN EXCLUDED.gapless_playback ELSE native_playback_settings.gapless_playback END,
                    volume_normalization = CASE WHEN EXCLUDED.updated_at >= native_playback_settings.updated_at
                                                THEN EXCLUDED.volume_normalization ELSE native_playback_settings.volume_normalization END,
                    replay_gain = CASE WHEN EXCLUDED.updated_at >= native_playback_settings.updated_at
                                       THEN EXCLUDED.replay_gain ELSE native_playback_settings.replay_gain END,
                    audio_quality = CASE WHEN EXCLUDED.updated_at >= native_playback_settings.updated_at
                                         THEN EXCLUDED.audio_quality ELSE native_playback_settings.audio_quality END,
                    equalizer_preset = CASE WHEN EXCLUDED.updated_at >= native_playback_settings.updated_at
                                            THEN EXCLUDED.equalizer_preset ELSE native_playback_settings.equalizer_preset END,
                    last_used_device = CASE WHEN EXCLUDED.updated_at >= native_playback_settings.updated_at
                                            THEN EXCLUDED.last_used_device ELSE native_playback_settings.last_used_device END,
                    updated_at = GREATEST(native_playback_settings.updated_at, EXCLUDED.updated_at)",
            )
            .bind(user_id)
            .bind(settings.row.crossfade_duration)
            .bind(settings.row.gapless_playback)
            .bind(settings.row.volume_normalization)
            .bind(&settings.row.replay_gain)
            .bind(&settings.row.audio_quality)
            .bind(settings.row.equalizer_preset.as_deref())
            .bind(settings.row.last_used_device.as_deref())
            .bind(settings.created_at)
            .bind(settings.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(|error| format!("playback settings write failed: {error}"))?;
            summary.playback_settings_written += 1;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
    }

    for chunk in export.equalizer.chunks(batch) {
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        for preset in chunk {
            let Some(user_id) = users.get(&preset.row.user_public_id) else {
                continue;
            };
            let existing: Option<(Uuid, Uuid)> = sqlx::query_as(
                "SELECT id, user_id FROM native_equalizer_presets WHERE public_api_id=$1 FOR UPDATE",
            )
            .bind(preset.row.public_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| format!("equalizer preset lookup failed: {error}"))?;
            match existing {
                Some((id, existing_owner)) => {
                    if existing_owner != *user_id {
                        return Err(format!(
                            "equalizer preset {} already exists for another user; refusing to move it",
                            preset.row.public_id
                        ));
                    }
                    sqlx::query(
                        "UPDATE native_equalizer_presets SET
                            name = CASE WHEN $2 >= updated_at THEN $3 ELSE name END,
                            bands = CASE WHEN $2 >= updated_at THEN $4 ELSE bands END,
                            is_default = CASE WHEN $2 >= updated_at THEN $5 ELSE is_default END,
                            updated_at = GREATEST(updated_at, $2)
                          WHERE id = $1 AND $2 > updated_at",
                    )
                    .bind(id)
                    .bind(preset.updated_at)
                    .bind(preset.row.name.trim())
                    .bind(&preset.row.bands)
                    .bind(preset.row.is_default)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| format!("equalizer preset update failed: {error}"))?;
                }
                None => {
                    sqlx::query(
                        "INSERT INTO native_equalizer_presets (
                            id, public_api_id, user_id, name, bands, is_default, created_at, updated_at
                         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                    )
                    .bind(Uuid::now_v7())
                    .bind(preset.row.public_id)
                    .bind(user_id)
                    .bind(preset.row.name.trim())
                    .bind(&preset.row.bands)
                    .bind(preset.row.is_default)
                    .bind(preset.created_at)
                    .bind(preset.updated_at)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| format!("equalizer preset insert failed: {error}"))?;
                }
            }
            summary.equalizer_presets_written += 1;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Human-readable report used by the CLI.
#[must_use]
pub fn render(summary: &ImportSummary, input: &Path, identity_map: Option<&Path>) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "User data import {} (schema v{}, {})",
        if summary.dry_run {
            "preview — dry run, no changes written"
        } else {
            "applied"
        },
        summary.schema_version,
        summary.generator
    ));
    let (new_accounts, action) = if summary.dry_run {
        (summary.users_planned_created, "planned for creation")
    } else {
        (summary.users_created, "created")
    };
    lines.push(format!(
        "  User accounts — {} in the export, {} matched to existing accounts, {new_accounts} {action}, {} skipped.",
        summary.users_in_export, summary.users_already_on_server + summary.users_linked + summary.users_matched_by_email, summary.users_unmatched.len()
    ));
    if summary.users_matched_by_email > 0 {
        lines.push(format!(
            "  Existing accounts matched automatically by email — {}.",
            summary.users_matched_by_email
        ));
    }
    if summary.users_requiring_review > 0 {
        lines.push(format!(
            "  Accounts to review — {}. Confirm which old and current accounts belong to the same person.",
            summary.users_requiring_review
        ));
    }
    if summary.tracks_matched_by_metadata > 0 {
        lines.push(format!(
            "  Tracks matched automatically by artist, release, and track title — {}.",
            summary.tracks_matched_by_metadata
        ));
    }
    if summary.tracks_with_ambiguous_matches > 0 {
        lines.push(format!(
            "  Tracks needing review because the match is ambiguous — {}.",
            summary.tracks_with_ambiguous_matches
        ));
    }
    if summary.users_requiring_review + summary.tracks_with_ambiguous_matches > 0 {
        lines.push("  Open the interactive review in your terminal from the melodee-rs checkout. Use the same database connection as this preview.".to_owned());
        let map = identity_map.unwrap_or(Path::new("./approved-links.json"));
        let quote_path =
            |path: &Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
        lines.push(format!(
            "    python3 scripts/reconcile_legacy_user_state.py --input {} --identity-map {} --review",
            quote_path(input), quote_path(map)
        ));
        if !input.join("catalog-evidence.ndjson").is_file() {
            lines.push("  This export supports account review only. To review music too, use the export directory containing catalog-evidence.ndjson.".to_owned());
        }
        lines.push("  Enter a candidate number to save a link, press Enter to skip, or enter q to finish. Then repeat this preview with the same --identity-map file.".to_owned());
    }
    if summary.users_linked > 0 {
        lines.push(format!(
            "  Existing accounts linked by saved public IDs — {}.",
            summary.users_linked
        ));
    }
    if !summary.dry_run {
        lines.push(format!(
            "  User profiles — {} updated, {} unchanged. Avatars written — {}.",
            summary.users_updated, summary.users_unchanged, summary.avatars_written
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "  {:<32} {:>10} {:>10} {:>10}",
        "Preferences across all users", "In export", "Matched", "Unmatched"
    ));
    let exported = &summary.exported_preferences;
    let matched = &summary.matched_preferences;
    for (label, total, matches) in [
        (
            "Favorites (likes)",
            exported.favorites(),
            matched.favorites(),
        ),
        (
            "  Favorite artists",
            exported.favorite_artists,
            matched.favorite_artists,
        ),
        (
            "  Favorite releases",
            exported.favorite_releases,
            matched.favorite_releases,
        ),
        (
            "  Favorite tracks",
            exported.favorite_tracks,
            matched.favorite_tracks,
        ),
        ("Dislikes", exported.dislikes, matched.dislikes),
        ("Ratings (1 to 5)", exported.ratings, matched.ratings),
        ("Pinned items", exported.pins, matched.pins),
    ] {
        lines.push(format!(
            "  {label:<32} {total:>10} {matches:>10} {:>10}",
            total - matches
        ));
    }
    let kept = &summary.superseded_preferences;
    if kept.favorites() + kept.dislikes + kept.ratings + kept.pins > 0 {
        lines.push(format!("  Current listener choices take precedence over {} matched favorites, {} dislikes, {} ratings and {} pins from the export.",
            kept.favorites(), kept.dislikes, kept.ratings, kept.pins));
    }
    lines.push(String::new());
    if summary.dry_run {
        lines.push(format!(
            "  Music preferences and play counts — {} records ready to import, {} skipped.",
            summary.states_created, summary.states_unmatched
        ));
        lines.push(format!(
            "  Listening history — {} plays ready to import, {} skipped.",
            summary.plays_inserted, summary.plays_unmatched
        ));
    } else {
        lines.push(format!(
            "  Music preferences and play counts — {} records created, {} updated, {} unchanged, {} unmatched.",
            summary.states_created, summary.states_updated, summary.states_unchanged, summary.states_unmatched
        ));
        lines.push(format!(
            "  Listening history — {} plays added, {} already present, {} unmatched.",
            summary.plays_inserted, summary.plays_duplicate, summary.plays_unmatched
        ));
    }
    if summary.dry_run {
        lines.push(format!(
            "  Playlists — {} with a matched owner. Playlist entries — {} matched, {} unmatched.",
            summary.playlists_created,
            summary.playlist_items_added,
            summary.playlist_items_unmatched
        ));
        lines.push(format!(
            "  Bookmarks — {} matched, {} unmatched.",
            summary.bookmarks_written, summary.bookmarks_unmatched
        ));
    } else {
        lines.push(format!("  Playlists — {} created, {} updated. Playlist entries — {} added, {} already present, {} unmatched.",
            summary.playlists_created, summary.playlists_updated, summary.playlist_items_added,
            summary.playlist_items_duplicate, summary.playlist_items_unmatched));
        lines.push(format!(
            "  Smart playlists — {} created, {} updated.",
            summary.smart_playlists_created, summary.smart_playlists_updated
        ));
        lines.push(format!("  Bookmarks — {} written, {} unmatched. Access groups — {} created, {} memberships added.",
            summary.bookmarks_written, summary.bookmarks_unmatched, summary.groups_created, summary.group_members_added));
        lines.push(format!(
            "  Playback settings written — {}. Equalizer presets written — {}.",
            summary.playback_settings_written, summary.equalizer_presets_written
        ));
    }
    // The detailed notes are capped. Still account for every reference-only stream.
    for (domain, count) in summary.reference_only.iter().skip(MAX_REPORTED) {
        lines.push(format!("  For reference only — {domain} ({count} rows)."));
    }
    for warning in &summary.warnings {
        lines.push(format!("  {warning}"));
    }
    for unmatched in &summary.users_unmatched {
        lines.push(format!("  Missing user — {unmatched}"));
    }
    lines.join("\n")
}

/// Convenience wrapper used by the CLI: load a directory, then apply it.
///
/// # Errors
/// Propagates loader and writer failures.
pub async fn import(
    pool: &PgPool,
    input: &Path,
    options: ImportOptions,
    identity_map: Option<&Path>,
) -> Result<ImportSummary, String> {
    let export = load(input)?;
    let mut links = IdentityMap::load(pool, input, identity_map).await?;
    apply(pool, &export, options, &mut links).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;

    fn write_export(dir: &Path, overrides: &[(&str, &[u8])]) -> BTreeMap<String, u64> {
        let mut counts = BTreeMap::new();
        let mut checksums = BTreeMap::new();
        for stream in STREAMS {
            let bytes = overrides
                .iter()
                .find(|(name, _)| *name == stream)
                .map(|(_, bytes)| *bytes)
                .unwrap_or(b"");
            std::fs::write(dir.join(stream), bytes).expect("stream write");
            checksums.insert(stream.to_owned(), sha256_hex(bytes));
            counts.insert(
                stream.to_owned(),
                u64::try_from(
                    BufReader::new(bytes)
                        .lines()
                        .map_while(Result::ok)
                        .filter(|line| !line.trim().is_empty())
                        .count(),
                )
                .expect("count"),
            );
        }
        counts
    }

    fn write_manifest(dir: &Path, counts: &BTreeMap<String, u64>, schema_version: u32) {
        let checksums = STREAMS
            .iter()
            .map(|stream| {
                let bytes = std::fs::read(dir.join(stream)).expect("stream read");
                (stream.to_string(), sha256_hex(&bytes))
            })
            .collect::<BTreeMap<_, _>>();
        let manifest = serde_json::json!({
            "schemaVersion": schema_version,
            "generator": { "name": "test-export", "version": "1.0.0" },
            "generatedAt": "2024-01-01T00:00:00Z",
            "counts": counts,
            "checksums": checksums,
            "referenceOnly": {
                "shares": { "count": 3, "reason": "bearer secrets are not portable" }
            }
        });
        std::fs::write(
            dir.join(MANIFEST_NAME),
            serde_json::to_vec(&manifest).expect("manifest"),
        )
        .expect("manifest write");
    }

    const USER: &[u8] = b"{\"publicId\":\"018f0000-0000-7000-8000-000000000001\",\"username\":\"listener\",\"email\":\"listener@example.test\",\"createdAt\":\"2024-01-01T00:00:00Z\",\"updatedAt\":\"2024-01-02T00:00:00Z\"}\n";

    #[test]
    fn timestamps_must_be_rfc3339() {
        assert!(parse_timestamp("2024-01-01T00:00:00Z", "createdAt").is_ok());
        assert!(parse_timestamp("2024-01-01 00:00:00+00", "createdAt").is_err());
        assert!(parse_timestamp("yesterday", "createdAt").is_err());
    }

    #[test]
    fn ratings_and_counts_are_bounded() {
        let base = EntityStateRow {
            user_public_id: Uuid::now_v7(),
            entity: EntityKind::Track,
            entity_public_id: Uuid::now_v7(),
            starred: true,
            hated: false,
            rating: 6,
            pinned: false,
            play_count: 0,
            last_played_at: None,
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            updated_at: "2024-01-02T00:00:00Z".to_owned(),
        };
        assert!(StateRecord::validated(base.clone()).is_err());
        assert!(
            StateRecord::validated(EntityStateRow {
                rating: 5,
                play_count: -1,
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            StateRecord::validated(EntityStateRow {
                rating: 5,
                play_count: 1,
                created_at: "2024-02-01T00:00:00Z".to_owned(),
                updated_at: "2024-01-01T00:00:00Z".to_owned(),
                ..base
            })
            .is_err()
        );
    }

    #[test]
    fn play_event_ids_are_stable_and_deduped() {
        let row = TrackPlayRow {
            user_public_id: Uuid::now_v7(),
            track_public_id: Uuid::now_v7(),
            played_at: "2024-05-01T12:00:00Z".to_owned(),
            duration_seconds: Some(180),
            completed: true,
        };
        let first = play_event_id(&PlayRecord::validated(row.clone()).expect("valid"));
        let second = play_event_id(&PlayRecord::validated(row.clone()).expect("valid"));
        assert_eq!(first, second);
        let other = play_event_id(
            &PlayRecord::validated(TrackPlayRow {
                played_at: "2024-05-01T12:00:01Z".to_owned(),
                ..row
            })
            .expect("valid"),
        );
        assert_ne!(first, other);
        assert!(first.starts_with("portable-play-"));
    }

    #[test]
    fn valid_export_loads_with_reference_only_reporting() {
        let dir = tempfile::tempdir().expect("temp dir");
        let counts = write_export(dir.path(), &[(USERS_NAME, USER)]);
        write_manifest(dir.path(), &counts, SCHEMA_VERSION);
        let export = load(dir.path()).expect("loads");
        assert_eq!(export.users.len(), 1);
        assert_eq!(
            export.reference_only.get("shares").map(|entry| entry.count),
            Some(3)
        );
    }

    #[test]
    fn unsupported_schema_version_fails_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let counts = write_export(dir.path(), &[]);
        write_manifest(dir.path(), &counts, SCHEMA_VERSION + 1);
        let error = load(dir.path()).expect_err("version rejected");
        assert!(error.contains("schema version"), "{error}");
    }

    #[test]
    fn checksum_mismatch_fails_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let counts = write_export(dir.path(), &[]);
        write_manifest(dir.path(), &counts, SCHEMA_VERSION);
        std::fs::write(dir.path().join(USERS_NAME), USER).expect("tamper");
        let error = load(dir.path()).expect_err("checksum rejected");
        assert!(error.contains("checksum mismatch"), "{error}");
    }

    #[test]
    fn manifest_count_mismatch_fails_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut counts = write_export(dir.path(), &[(USERS_NAME, USER)]);
        counts.insert(USERS_NAME.to_owned(), 2);
        write_manifest(dir.path(), &counts, SCHEMA_VERSION);
        let error = load(dir.path()).expect_err("count rejected");
        assert!(error.contains("declares 2"), "{error}");
    }

    #[test]
    fn playlist_items_require_a_declared_playlist() {
        let dir = tempfile::tempdir().expect("temp dir");
        let items = b"{\"playlistPublicId\":\"018f0000-0000-7000-8000-0000000000aa\",\"position\":0,\"trackPublicId\":\"018f0000-0000-7000-8000-0000000000bb\"}\n";
        let counts = write_export(dir.path(), &[(PLAYLIST_ITEMS_NAME, items)]);
        write_manifest(dir.path(), &counts, SCHEMA_VERSION);
        let error = load(dir.path()).expect_err("orphan item rejected");
        assert!(error.contains("not in playlists.ndjson"), "{error}");
    }

    #[test]
    fn duplicate_user_rows_fail_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut users = USER.to_vec();
        users.extend_from_slice(USER);
        let counts = write_export(dir.path(), &[(USERS_NAME, &users)]);
        write_manifest(dir.path(), &counts, SCHEMA_VERSION);
        let error = load(dir.path()).expect_err("duplicate user rejected");
        assert!(error.contains("repeats"), "{error}");
    }
}
