//! Operator-selected metadata for unpublished review items.
use super::*;
use crate::providers::{ProviderRuntime, review_release_candidates};
use melodee_persistence::{
    ProviderCacheStore,
    review_correction::{ReviewIdentification, ReviewedManifest, save_review_correction},
};

fn supports_release(provider: &str) -> bool {
    matches!(
        provider,
        "melodee"
            | "musicbrainz"
            | "discogs"
            | "spotify"
            | "deezer"
            | "itunes"
            | "lastfm"
            | "metalapi"
    )
}

const SEARCH_LIMIT: usize = 25;

#[derive(Deserialize)]
pub(super) struct Search {
    provider: String,
    artist: String,
    title: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Match {
    external_id: String,
    title: String,
    artists: Vec<String>,
    date: Option<String>,
}

fn conflict() -> Response {
    review_error(
        StatusCode::CONFLICT,
        "review_changed",
        "This Release or selection has changed. Reload the review and search again.",
    )
}

fn invalid(message: &'static str) -> Response {
    review_error(StatusCode::BAD_REQUEST, "invalid_metadata", message)
}

async fn runtime(state: &AdminReviewState) -> Result<&ProviderRuntime, Box<Response>> {
    state
        .providers
        .get_or_try_init(|| async { ProviderRuntime::new(state.admin.pool.clone()) })
        .await
        .map_err(|e| Box::new(store_failure("provider setup failed", &e)))
}

async fn eligible(
    state: &AdminReviewState,
    id: Uuid,
) -> Result<melodee_domain::ReleaseWorkItem, Box<Response>> {
    let item = WorkItemStore
        .get(&state.admin.pool, id)
        .await
        .map_err(|e| work_transition_error(id, e))?;
    if item.state != IngestionState::NeedsReview {
        return Err(Box::new(conflict()));
    }
    Ok(item)
}

pub(super) async fn sources(
    State(state): State<AdminReviewState>,
    _: Reviewer,
    AxumPath(id): AxumPath<Uuid>,
) -> Response {
    if let Err(r) = eligible(&state, id).await {
        return *r;
    }
    let rt = match runtime(&state).await {
        Ok(rt) => rt,
        Err(r) => return *r,
    };
    match rt.capabilities().await {
        Ok(rows) => Json(
            rows.into_iter()
                .filter(|p| p.enabled && p.configured && supports_release(&p.provider))
                .map(|p| serde_json::json!({"id":p.provider,"name":p.display_name}))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => store_failure("provider settings unavailable", &e),
    }
}

// Prefer ordered, authoritative billing. display_artist may legitimately be
// null on multi-artist Releases.
macro_rules! local_releases {
    ($filter:literal) => {
        concat!(
            "SELECT r.id,r.public_api_id,r.title,
    ARRAY(SELECT a.canonical_name FROM release_artists ra JOIN artists a ON a.id=ra.artist_id
       WHERE ra.release_id=r.id ORDER BY ra.presentation_order,ra.id) AS artists,
    COALESCE(r.release_date::text,r.release_date_text) AS date,r.display_artist
    FROM releases r WHERE r.status='active'",
            $filter
        )
    };
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResults {
    cache_key: Option<String>,
    matches: Vec<Match>,
}

impl From<crate::providers::ReviewProviderRelease> for Match {
    fn from(row: crate::providers::ReviewProviderRelease) -> Self {
        Self {
            external_id: row.candidate.external_id,
            title: row.title,
            artists: row.artists,
            date: row.date,
        }
    }
}

type LocalRow = (
    Uuid,
    Uuid,
    String,
    Vec<String>,
    Option<String>,
    Option<String>,
);
fn local_match(row: LocalRow) -> (Match, Uuid) {
    let (id, public, title, mut artists, date, display) = row;
    if artists.is_empty() {
        artists.extend(display);
    }
    (
        Match {
            external_id: public.to_string(),
            title,
            artists,
            date,
        },
        id,
    )
}

fn valid_name(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 512 && !s.chars().any(char::is_control)
}

pub(super) async fn search(
    State(state): State<AdminReviewState>,
    _: Reviewer,
    AxumPath(id): AxumPath<Uuid>,
    Query(q): Query<Search>,
) -> Response {
    if let Err(r) = eligible(&state, id).await {
        return *r;
    }
    let artist = q.artist.trim();
    let title = q.title.trim();
    if !valid_name(title) || (!artist.is_empty() && !valid_name(artist)) {
        return invalid(
            "Enter a Release title and an optional artist (up to 512 characters each).",
        );
    }
    if q.provider == "melodee" {
        let (Ok(title_pattern), Ok(artist_pattern)) = (
            melodee_persistence::contains_pattern(Some(title)),
            melodee_persistence::contains_pattern(Some(artist)),
        ) else {
            return invalid("Catalog search fields can contain up to 200 characters.");
        };
        let result = sqlx::query_as::<_, LocalRow>(local_releases!(
            " AND r.title ILIKE $1
              AND ($2::text IS NULL OR r.display_artist ILIKE $2 OR EXISTS (
                 SELECT 1 FROM release_artists ra JOIN artists a ON a.id=ra.artist_id
                 WHERE ra.release_id=r.id AND a.canonical_name ILIKE $2))
              ORDER BY (lower(r.title)=lower($3)) DESC,r.title,r.public_api_id LIMIT $4"
        ))
        .bind(title_pattern)
        .bind(artist_pattern)
        .bind(title)
        .bind(SEARCH_LIMIT as i64)
        .fetch_all(&state.admin.pool)
        .await;
        return match result {
            Ok(rows) => Json(SearchResults {
                cache_key: None,
                matches: rows.into_iter().map(|row| local_match(row).0).collect(),
            })
            .into_response(),
            Err(error) => store_failure("catalog Release search failed", &error),
        };
    }
    let rt = match runtime(&state).await {
        Ok(rt) => rt,
        Err(r) => return *r,
    };
    match rt.capabilities().await {
        Ok(rows)
            if rows.iter().any(|p| {
                p.provider == q.provider
                    && p.enabled
                    && p.configured
                    && supports_release(&p.provider)
            }) => {}
        Ok(_) => return invalid("This provider is unavailable."),
        Err(e) => return store_failure("provider settings unavailable", &e),
    }
    let query = if q.provider == "musicbrainz" {
        // The existing adapter supplies the release: field prefix.
        let literal = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        if artist.is_empty() {
            literal(title)
        } else {
            format!("{} AND artist:{}", literal(title), literal(artist))
        }
    } else {
        format!("{artist} {title}")
    };
    match rt
        .enrich_release(query.trim(), &q.provider, SEARCH_LIMIT)
        .await
    {
        Ok(result) => {
            let body = result.payload.get("payload").unwrap_or(&result.payload);
            match review_release_candidates(&q.provider, body) {
                Ok(rows) => Json(SearchResults {
                    cache_key: Some(result.cache_key),
                    matches: rows
                        .into_iter()
                        .take(SEARCH_LIMIT)
                        .map(Match::from)
                        .collect(),
                })
                .into_response(),
                Err(error) => store_failure("invalid provider results", &error),
            }
        }
        Err(e) => {
            tracing::warn!(category = e.category, "review provider lookup failed");
            review_error(
                StatusCode::BAD_GATEWAY,
                "provider_unavailable",
                "The provider search failed. Retry or choose another provider.",
            )
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Correction {
    expected_hash: String,
    title: String,
    artists: Vec<String>,
    tracks: Vec<TrackEdit>,
    selection: Option<Selection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrackEdit {
    index: usize,
    title: String,
    disc: Option<u32>,
    number: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    provider: String,
    external_id: String,
    cache_key: Option<String>,
}

async fn resolve(
    pool: &sqlx::PgPool,
    s: &Selection,
) -> Result<(Match, ReviewIdentification), Box<Response>> {
    if !supports_release(&s.provider) {
        return Err(Box::new(invalid("Unknown Release provider.")));
    }
    let (matched, target) = if s.provider == "melodee" {
        let public =
            Uuid::parse_str(&s.external_id).map_err(|_| invalid("Invalid Melodee Release ID."))?;
        let row = sqlx::query_as::<_, LocalRow>(local_releases!(" AND r.public_api_id=$1"))
            .bind(public)
            .fetch_optional(pool)
            .await
            .map_err(|e| store_failure("catalog match unavailable", &e))?
            .ok_or_else(conflict)?;
        let (m, id) = local_match(row);
        (m, Some(id))
    } else {
        let key = s
            .cache_key
            .as_deref()
            .filter(|k| k.len() == 64)
            .ok_or_else(conflict)?;
        let entry = ProviderCacheStore::new()
            .get_if_fresh(pool, &s.provider, key, OffsetDateTime::now_utc())
            .await
            .map_err(|e| store_failure("provider cache unavailable", &e))?
            .ok_or_else(conflict)?;
        if entry.payload["purpose"] != "release_metadata" {
            return Err(Box::new(conflict()));
        }
        let row = review_release_candidates(&s.provider, &entry.payload["payload"])
            .map_err(|e| store_failure("provider match unavailable", &e))?
            .into_iter()
            .take(SEARCH_LIMIT)
            .find(|m| m.candidate.external_id == s.external_id)
            .ok_or_else(conflict)?;
        (Match::from(row), None)
    };
    Ok((
        matched,
        ReviewIdentification {
            provider: s.provider.clone(),
            external_id: s.external_id.clone(),
            target_release_id: target,
        },
    ))
}

fn correct(
    manifest: &mut ReleaseManifest,
    req: &Correction,
) -> Result<serde_json::Value, &'static str> {
    let title = req.title.trim();
    if !valid_name(title)
        || req.artists.is_empty()
        || req.artists.len() > 128
        || req.artists.iter().any(|a| !valid_name(a))
    {
        return Err("Enter a Release title and at least one artist.");
    }
    let artists = req
        .artists
        .iter()
        .map(|a| a.trim().to_owned())
        .collect::<Vec<_>>();
    let mut changes = serde_json::Map::new();
    let release = &mut manifest.release_observation;
    if release.title.as_deref() != Some(title) {
        changes.insert("title".into(), serde_json::json!([release.title, title]));
        release.title = Some(title.to_owned());
    }
    if release.artists != artists {
        changes.insert(
            "artists".into(),
            serde_json::json!([release.artists, artists]),
        );
        release.artists = artists;
        // Catalog billing otherwise derives from original per-file tags.
        // Keep those tags as evidence, including guest Track artist credits.
        if !release.source_labels.is_object() {
            release.source_labels = serde_json::json!({"original":release.source_labels});
        }
        release.source_labels["reviewed_release_billing"] = true.into();
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut tracks = Vec::new();
    for edit in &req.tracks {
        let track = manifest
            .tracks
            .get_mut(edit.index)
            .ok_or("Unknown Track index.")?;
        let title = edit.title.trim();
        if !seen.insert(edit.index)
            || !valid_name(title)
            || [edit.disc, edit.number]
                .into_iter()
                .flatten()
                .any(|n| n == 0 || n > 65535)
        {
            return Err("Each Track needs a unique index, a title and valid positive numbers.");
        }
        let before = serde_json::json!([track.track_title, track.media_number, track.track_number]);
        let after = serde_json::json!([title, edit.disc, edit.number]);
        if before != after {
            tracks.push(serde_json::json!({"index":edit.index,"before":before,"after":after}));
            track.track_title = Some(title.to_owned());
            if track.track_number != edit.number {
                track.sort_order = edit.number;
                track.track_number = edit.number;
            }
            track.media_number = edit.disc;
        }
    }
    if !tracks.is_empty() {
        changes.insert("tracks".into(), tracks.into());
    }
    Ok(changes.into())
}

pub(super) async fn save(
    State(state): State<AdminReviewState>,
    reviewer: Reviewer,
    AxumPath(id): AxumPath<Uuid>,
    Json(mut req): Json<Correction>,
) -> Response {
    let item = match eligible(&state, id).await {
        Ok(item) => item,
        Err(r) => return *r,
    };
    let pool = &state.admin.pool;
    let stored = match WorkItemStore.get_manifest(pool, id).await {
        Ok(Some(m)) => m,
        Ok(None) => return conflict(),
        Err(e) => return store_failure("manifest read failed", &e),
    };
    if stored.manifest_hash != req.expected_hash {
        return conflict();
    }
    let mut manifest: ReleaseManifest = match serde_json::from_value(stored.payload) {
        Ok(m) => m,
        Err(e) => return store_failure("invalid stored manifest", &e),
    };
    let selection = if let Some(s) = &req.selection {
        match resolve(pool, s).await {
            Ok((m, choice)) => {
                req.title = m.title;
                if !m.artists.is_empty() {
                    req.artists = m.artists;
                }
                Some(choice)
            }
            Err(r) => return *r,
        }
    } else {
        None
    };
    let retarget = item.state_reason.as_deref() == Some("review.merge_target_unavailable");
    let changes = if retarget {
        serde_json::Value::Null
    } else {
        if manifest.schema_version != 2
            || manifest.normalization.is_none()
            || manifest.publication.is_some()
        {
            return conflict();
        }
        match correct(&mut manifest, &req) {
            Ok(v) => v,
            Err(e) => return invalid(e),
        }
    };
    let hash = match melodee_media::hash_manifest(&manifest) {
        Ok(h) => h,
        Err(e) => return store_failure("manifest hashing failed", &e),
    };
    let correction = ReviewedManifest {
        expected_hash: &req.expected_hash,
        new_hash: &hash,
        manifest: &manifest,
        changes,
    };
    match save_review_correction(
        pool,
        id,
        reviewer.public_api_id,
        correction,
        selection.as_ref(),
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(WorkStoreError::ManifestConflict(_)) => conflict(),
        Err(WorkStoreError::InvalidRequest(msg)) => invalid(msg),
        Err(e) => work_transition_error(id, e),
    }
}
