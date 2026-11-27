//! Provisional bounded release-media deletion closure under an externally
//! quiesced maintenance fence. Metadata and acknowledgements are not proof
//! that filesystem work was safe; production routing remains disconnected.
use crate::{
    AdminMediaReservation, LifecycleMaintenance, WorkItemStore, WorkStoreError,
    admin_media_reservation, work_item_lifecycle,
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminCatalogDeleteKind {
    Release,
    Track,
    Artist,
}
impl AdminCatalogDeleteKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Track => "track",
            Self::Artist => "artist",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminMediaSnapshotEntry {
    pub asset_id: Uuid,
    pub relative_path: String,
    pub byte_hash: String,
    pub byte_size: i64,
}
/// Exact offline deletion object, with storage namespace made explicit.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminMediaSnapshotObject {
    pub kind: String,
    pub object_id: Uuid,
    pub object_key: String,
    pub byte_hash: Option<String>,
    pub byte_size: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMediaObjectKey {
    pub kind: String,
    pub object_id: Uuid,
}
#[derive(sqlx::FromRow)]
struct CaptureRow {
    release_id: Uuid,
    ordinal: i32,
    track_id: Uuid,
    asset_id: Uuid,
    purpose: String,
    relative_path: String,
    byte_hash: String,
    byte_size: i64,
}
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid administrative media snapshot")
}
async fn budget(tx: &mut Transaction<'_, Postgres>) -> Result<(), WorkStoreError> {
    sqlx::query("SELECT singleton FROM admin_media_snapshot_budget WHERE singleton FOR UPDATE")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn fences(
    tx: &mut Transaction<'_, Postgres>,
    fence: &LifecycleMaintenance,
    reservation: &AdminMediaReservation,
) -> Result<(time::OffsetDateTime, time::OffsetDateTime), WorkStoreError> {
    if fence.owner != reservation.owner {
        return Err(invalid());
    }
    let maintenance = work_item_lifecycle::lock_fence(tx, fence, true).await?;
    let admin = admin_media_reservation::verify_lease(tx, reservation).await?;
    Ok((maintenance, admin))
}
async fn finish(
    tx: &mut Transaction<'_, Postgres>,
    expiry: (time::OffsetDateTime, time::OffsetDateTime),
) -> Result<(), WorkStoreError> {
    work_item_lifecycle::finish(tx, expiry.0).await?;
    admin_media_reservation::check_expiry(tx, expiry.1).await?;
    Ok(())
}
async fn record_object(
    tx: &mut Transaction<'_, Postgres>,
    reservation: Uuid,
    object: &AdminMediaSnapshotObject,
) -> Result<(), WorkStoreError> {
    let cost = i64::try_from(object.object_key.len() + 256).map_err(|_| invalid())?;
    let inserted=sqlx::query("INSERT INTO admin_media_snapshot_objects(reservation_id,kind,object_id,object_key,byte_hash,byte_size) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
        .bind(reservation).bind(&object.kind).bind(object.object_id).bind(&object.object_key).bind(&object.byte_hash).bind(object.byte_size).execute(&mut **tx).await?;
    if inserted.rows_affected() == 1 {
        let charged=sqlx::query("UPDATE admin_media_snapshot_budget SET entries=entries+1,bytes=bytes+$1 WHERE singleton AND entries<max_entries AND $1<=max_bytes-bytes").bind(cost).execute(&mut **tx).await?;
        if charged.rows_affected() != 1 {
            return Err(invalid());
        }
        sqlx::query("UPDATE admin_media_snapshots SET entries=entries+1,bytes=bytes+$2 WHERE reservation_id=$1").bind(reservation).bind(cost).execute(&mut **tx).await?;
    } else {
        let same:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_snapshot_objects WHERE reservation_id=$1 AND kind=$2 AND (object_id=$3 OR ($2='artwork' AND object_key=$4)) AND object_key COLLATE \"C\"=$4 COLLATE \"C\" AND byte_hash IS NOT DISTINCT FROM $5 AND byte_size IS NOT DISTINCT FROM $6)")
            .bind(reservation).bind(&object.kind).bind(object.object_id).bind(&object.object_key).bind(&object.byte_hash).bind(object.byte_size).fetch_one(&mut **tx).await?;
        if !same {
            return Err(invalid());
        }
    }
    Ok(())
}
pub(crate) async fn exclusive_artwork(
    tx: &mut Transaction<'_, Postgres>,
    reservation: Uuid,
    keys: &[String],
) -> Result<std::collections::HashSet<String>, WorkStoreError> {
    let keys:Vec<String>=sqlx::query_scalar("SELECT DISTINCT k.asset_key FROM unnest($2::text[]) k(asset_key) WHERE NOT EXISTS(SELECT 1 FROM catalog_entity_images i WHERE i.asset_key=k.asset_key AND ((i.release_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM admin_media_reservation_releases r WHERE r.reservation_id=$1 AND r.release_id=i.release_id)) OR (i.artist_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM admin_media_delete_targets t JOIN admin_media_delete_scopes sc USING(reservation_id) WHERE t.reservation_id=$1 AND sc.kind='artist' AND t.internal_id=i.artist_id)))) AND NOT EXISTS(SELECT 1 FROM catalog_provider_images i WHERE i.asset_key=k.asset_key AND ((i.entity_type='release' AND NOT EXISTS(SELECT 1 FROM admin_media_reservation_releases r WHERE r.reservation_id=$1 AND r.release_id=i.entity_id)) OR (i.entity_type='artist' AND NOT EXISTS(SELECT 1 FROM admin_media_delete_targets t JOIN admin_media_delete_scopes sc USING(reservation_id) WHERE t.reservation_id=$1 AND sc.kind='artist' AND t.internal_id=i.entity_id)))) AND NOT EXISTS(SELECT 1 FROM native_user_avatars WHERE asset_key=k.asset_key) AND NOT EXISTS(SELECT 1 FROM native_theme_assets WHERE asset_key=k.asset_key) AND NOT EXISTS(SELECT 1 FROM native_playlists WHERE asset_key=k.asset_key) AND NOT EXISTS(SELECT 1 FROM native_users WHERE asset_key=k.asset_key) AND NOT EXISTS(SELECT 1 FROM release_work_item_manifest_evidence WHERE asset_key=k.asset_key) AND NOT EXISTS(SELECT 1 FROM release_work_item_manifest_evidence_revision WHERE asset_key=k.asset_key)")
        .bind(reservation).bind(keys).fetch_all(&mut **tx).await?;
    Ok(keys.into_iter().collect())
}
// Revalidate bounded target identities and their complete owned-Release set
// before permitting any filesystem page. The maintenance fence must also be
// externally enforced against all old writers throughout filesystem work.
pub(crate) async fn verify_bound_scope(
    tx: &mut Transaction<'_, Postgres>,
    reservation: Uuid,
) -> Result<(), WorkStoreError> {
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM admin_media_delete_scopes WHERE reservation_id=$1 FOR SHARE",
    )
    .bind(reservation)
    .fetch_optional(&mut **tx)
    .await?;
    let kind = kind.ok_or_else(invalid)?;
    let saved:Vec<(Uuid,Uuid,Option<Uuid>)>=sqlx::query_as("SELECT internal_id,public_id,parent_release_id FROM admin_media_delete_targets WHERE reservation_id=$1 ORDER BY internal_id LIMIT 129").bind(reservation).fetch_all(&mut **tx).await?;
    if saved.is_empty() || saved.len() > 128 {
        return Err(invalid());
    }
    let ids: Vec<Uuid> = saved.iter().map(|row| row.0).collect();
    let current: Vec<(Uuid, Uuid, Option<Uuid>)> = match kind.as_str() {
        "release" => {
            sqlx::query_as(
                "SELECT id,public_api_id,NULL::uuid FROM releases WHERE id=ANY($1) ORDER BY id",
            )
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?
        }
        "track" => {
            sqlx::query_as(
                "SELECT id,public_api_id,release_id FROM tracks WHERE id=ANY($1) ORDER BY id",
            )
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?
        }
        "artist" => {
            sqlx::query_as(
                "SELECT id,public_api_id,NULL::uuid FROM artists WHERE id=ANY($1) ORDER BY id",
            )
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?
        }
        _ => return Err(invalid()),
    };
    if saved != current {
        return Err(invalid());
    }
    let mut releases:Vec<Uuid>=match kind.as_str() {
        "release"=>ids.clone(),
        "track"=>saved.iter().filter_map(|row|row.2).collect(),
        "artist"=>sqlx::query_scalar("SELECT DISTINCT billing.release_id FROM release_artists billing WHERE billing.artist_id=ANY($1) AND billing.role='primary' AND NOT EXISTS(SELECT 1 FROM release_artists other WHERE other.release_id=billing.release_id AND other.role='primary' AND other.artist_id<>ALL($1)) ORDER BY billing.release_id LIMIT 129").bind(&ids).fetch_all(&mut **tx).await?,
        _=>return Err(invalid()),
    };
    releases.sort_unstable();
    releases.dedup();
    let reserved:Vec<Uuid>=sqlx::query_scalar("SELECT release_id FROM admin_media_reservation_releases WHERE reservation_id=$1 ORDER BY release_id LIMIT 129").bind(reservation).fetch_all(&mut **tx).await?;
    if releases.len() > 128 || releases != reserved {
        return Err(invalid());
    }
    Ok(())
}
impl WorkItemStore {
    /// Bind an exact bounded catalog target set to an externally quiesced
    /// reservation. The derived Release set must match the reserved scope.
    pub async fn bind_maintenance_catalog_targets(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        kind: AdminCatalogDeleteKind,
        public_ids: &[Uuid],
    ) -> Result<(), WorkStoreError> {
        if public_ids.is_empty() || public_ids.len() > 128 {
            return Err(invalid());
        }
        let mut unique = public_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != public_ids.len() {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let targets:Vec<(Uuid,Uuid,Option<Uuid>)>=match kind {
            AdminCatalogDeleteKind::Release=>sqlx::query_as("SELECT id,public_api_id,NULL::uuid FROM releases WHERE public_api_id=ANY($1) ORDER BY id").bind(public_ids).fetch_all(&mut *tx).await?,
            AdminCatalogDeleteKind::Track=>sqlx::query_as("SELECT id,public_api_id,release_id FROM tracks WHERE public_api_id=ANY($1) ORDER BY id").bind(public_ids).fetch_all(&mut *tx).await?,
            AdminCatalogDeleteKind::Artist=>sqlx::query_as("SELECT id,public_api_id,NULL::uuid FROM artists WHERE public_api_id=ANY($1) ORDER BY id").bind(public_ids).fetch_all(&mut *tx).await?,
        };
        if targets.len() != public_ids.len() {
            return Err(invalid());
        }
        let target_ids: Vec<Uuid> = targets.iter().map(|row| row.0).collect();
        let mut releases:Vec<Uuid>=match kind {
            AdminCatalogDeleteKind::Release=>target_ids.clone(),
            AdminCatalogDeleteKind::Track=>targets.iter().filter_map(|row|row.2).collect(),
            AdminCatalogDeleteKind::Artist=>sqlx::query_scalar("SELECT DISTINCT billing.release_id FROM release_artists billing WHERE billing.artist_id=ANY($1) AND billing.role='primary' AND NOT EXISTS(SELECT 1 FROM release_artists other WHERE other.release_id=billing.release_id AND other.role='primary' AND other.artist_id<>ALL($1)) ORDER BY billing.release_id LIMIT 129").bind(&target_ids).fetch_all(&mut *tx).await?,
        };
        releases.sort_unstable();
        releases.dedup();
        if releases.len() > 128 {
            return Err(invalid());
        }
        let reserved:Vec<Uuid>=sqlx::query_scalar("SELECT release_id FROM admin_media_reservation_releases WHERE reservation_id=$1 ORDER BY release_id").bind(reservation.id).fetch_all(&mut *tx).await?;
        if reserved != releases {
            return Err(invalid());
        }
        let prior: Option<String> = sqlx::query_scalar(
            "SELECT kind FROM admin_media_delete_scopes WHERE reservation_id=$1 FOR UPDATE",
        )
        .bind(reservation.id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(prior) = prior {
            let saved:Vec<(Uuid,Uuid,Option<Uuid>)>=sqlx::query_as("SELECT internal_id,public_id,parent_release_id FROM admin_media_delete_targets WHERE reservation_id=$1 ORDER BY internal_id").bind(reservation.id).fetch_all(&mut *tx).await?;
            if prior != kind.as_str() || saved != targets {
                return Err(invalid());
            }
        } else {
            let snapshot_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM admin_media_snapshots WHERE reservation_id=$1)",
            )
            .bind(reservation.id)
            .fetch_one(&mut *tx)
            .await?;
            if snapshot_exists {
                return Err(invalid());
            }
            sqlx::query("INSERT INTO admin_media_delete_scopes(reservation_id,kind) VALUES($1,$2)")
                .bind(reservation.id)
                .bind(kind.as_str())
                .execute(&mut *tx)
                .await?;
            let internals: Vec<Uuid> = targets.iter().map(|r| r.0).collect();
            let publics: Vec<Uuid> = targets.iter().map(|r| r.1).collect();
            let parents: Vec<Option<Uuid>> = targets.iter().map(|r| r.2).collect();
            sqlx::query("INSERT INTO admin_media_delete_targets(reservation_id,internal_id,public_id,parent_release_id) SELECT $1,t.internal_id,t.public_id,t.parent_release_id FROM unnest($2::uuid[],$3::uuid[],$4::uuid[]) t(internal_id,public_id,parent_release_id)")
                .bind(reservation.id).bind(&internals).bind(&publics).bind(&parents).execute(&mut *tx).await?;
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Create an empty provisional release-media snapshot; replay is idempotent.
    pub async fn begin_maintenance_media_snapshot(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        budget(&mut tx).await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let scoped: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM admin_media_delete_scopes WHERE reservation_id=$1)",
        )
        .bind(reservation.id)
        .fetch_one(&mut *tx)
        .await?;
        if !scoped {
            return Err(invalid());
        }
        verify_bound_scope(&mut tx, reservation.id).await?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT state FROM admin_media_snapshots WHERE reservation_id=$1")
                .bind(reservation.id)
                .fetch_optional(&mut *tx)
                .await?;
        if existing.as_deref() == Some("discarding") {
            return Err(invalid());
        }
        if existing.is_none() {
            let updated=sqlx::query("UPDATE admin_media_snapshot_budget SET snapshots=snapshots+1 WHERE singleton AND snapshots<max_snapshots").execute(&mut *tx).await?;
            if updated.rows_affected() != 1 {
                return Err(invalid());
            }
            sqlx::query(
                "INSERT INTO admin_media_snapshots(reservation_id,state) VALUES($1,'capturing')",
            )
            .bind(reservation.id)
            .execute(&mut *tx)
            .await?;
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Visit at most 16 indexed track/asset relationships. Shared assets outside
    /// the reserved release set are excluded; repeated assets are deduplicated.
    /// Returns true only on the explicit empty-page freeze turn.
    pub async fn capture_maintenance_media_snapshot_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        budget(&mut tx).await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        #[allow(clippy::type_complexity)]
        let snapshot:Option<(String,String,Option<Uuid>,Option<i32>,Option<Uuid>,Option<Uuid>,Option<String>)>=sqlx::query_as("SELECT state,capture_phase,cursor_release,cursor_ordinal,cursor_track,cursor_asset,cursor_purpose FROM admin_media_snapshots WHERE reservation_id=$1 FOR UPDATE").bind(reservation.id).fetch_optional(&mut *tx).await?;
        let Some((state, phase, release, ordinal, track, asset, purpose)) = snapshot else {
            return Err(invalid());
        };
        if state == "frozen" {
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(true);
        }
        if state != "capturing" {
            return Err(invalid());
        }
        if phase != "media" {
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return self
                .capture_maintenance_aux_snapshot_page(pool, fence, reservation)
                .await;
        }
        let page:Vec<CaptureRow>=sqlx::query_as("SELECT r.release_id,t.ordinal,t.id AS track_id,a.id AS asset_id,l.purpose,a.canonical_relative_path AS relative_path,a.byte_hash,a.byte_size FROM admin_media_reservation_releases r JOIN admin_media_delete_scopes scope ON scope.reservation_id=r.reservation_id JOIN tracks t ON t.release_id=r.release_id JOIN track_media_assets l ON l.track_id=t.id JOIN media_assets a ON a.id=l.media_asset_id WHERE r.reservation_id=$1 AND (scope.kind<>'track' OR EXISTS(SELECT 1 FROM admin_media_delete_targets target WHERE target.reservation_id=$1 AND target.internal_id=t.id)) AND ($2::uuid IS NULL OR (r.release_id,t.ordinal,t.id,a.id,l.purpose)>($2,$3,$4,$5,$6)) AND NOT EXISTS(SELECT 1 FROM track_media_assets other JOIN tracks other_track ON other_track.id=other.track_id WHERE other.media_asset_id=a.id AND ((scope.kind='track' AND NOT EXISTS(SELECT 1 FROM admin_media_delete_targets target WHERE target.reservation_id=$1 AND target.internal_id=other.track_id)) OR (scope.kind<>'track' AND NOT EXISTS(SELECT 1 FROM admin_media_reservation_releases included WHERE included.reservation_id=$1 AND included.release_id=other_track.release_id)))) ORDER BY r.release_id,t.ordinal,t.id,a.id,l.purpose LIMIT 16")
            .bind(reservation.id).bind(release).bind(ordinal).bind(track).bind(asset).bind(purpose).fetch_all(&mut *tx).await?;
        if page.is_empty() {
            sqlx::query("UPDATE admin_media_snapshots SET capture_phase='directories' WHERE reservation_id=$1")
                .bind(reservation.id)
                .execute(&mut *tx)
                .await?;
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(false);
        }
        // Deduplicate within the page and reject changed evidence before a
        // set-based insert. The budget row serializes conflicting captures.
        let mut unique = std::collections::BTreeMap::new();
        for row in &page {
            if let Some(previous) = unique.insert(row.asset_id, row) {
                let previous: &CaptureRow = previous;
                if previous.relative_path != row.relative_path
                    || previous.byte_hash != row.byte_hash
                    || previous.byte_size != row.byte_size
                {
                    return Err(invalid());
                }
            }
        }
        let ids: Vec<Uuid> = unique.keys().copied().collect();
        let paths: Vec<&str> = unique
            .values()
            .map(|row| row.relative_path.as_str())
            .collect();
        let hashes: Vec<&str> = unique.values().map(|row| row.byte_hash.as_str()).collect();
        let sizes: Vec<i64> = unique.values().map(|row| row.byte_size).collect();
        let mismatch:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM unnest($2::uuid[],$3::text[],$4::text[],$5::bigint[]) x(id,path,hash,size) JOIN admin_media_snapshot_entries e ON e.reservation_id=$1 AND e.asset_id=x.id WHERE e.relative_path COLLATE \"C\"<>x.path COLLATE \"C\" OR e.byte_hash<>x.hash OR e.byte_size<>x.size)")
            .bind(reservation.id).bind(&ids).bind(&paths).bind(&hashes).bind(&sizes).fetch_one(&mut *tx).await?;
        if mismatch {
            return Err(invalid());
        }
        let inserted:Vec<String>=sqlx::query_scalar("INSERT INTO admin_media_snapshot_entries(reservation_id,asset_id,relative_path,byte_hash,byte_size) SELECT $1,x.id,x.path,x.hash,x.size FROM unnest($2::uuid[],$3::text[],$4::text[],$5::bigint[]) x(id,path,hash,size) ON CONFLICT(reservation_id,asset_id) DO NOTHING RETURNING relative_path")
            .bind(reservation.id).bind(&ids).bind(&paths).bind(&hashes).bind(&sizes).fetch_all(&mut *tx).await?;
        if !inserted.is_empty() {
            let count = i64::try_from(inserted.len()).map_err(|_| invalid())?;
            let bytes = inserted
                .iter()
                .try_fold(0_i64, |sum, path| {
                    i64::try_from(path.len() + 256)
                        .ok()
                        .and_then(|cost| sum.checked_add(cost))
                })
                .ok_or_else(invalid)?;
            let charged=sqlx::query("UPDATE admin_media_snapshot_budget SET entries=entries+$1,bytes=bytes+$2 WHERE singleton AND $1<=max_entries-entries AND $2<=max_bytes-bytes")
                .bind(count).bind(bytes).execute(&mut *tx).await?;
            if charged.rows_affected() != 1 {
                return Err(invalid());
            }
            sqlx::query("UPDATE admin_media_snapshots SET entries=entries+$2,bytes=bytes+$3 WHERE reservation_id=$1").bind(reservation.id).bind(count).bind(bytes).execute(&mut *tx).await?;
        }
        let last = page.last().ok_or_else(invalid)?;
        sqlx::query("UPDATE admin_media_snapshots SET cursor_release=$2,cursor_ordinal=$3,cursor_track=$4,cursor_asset=$5,cursor_purpose=$6 WHERE reservation_id=$1")
            .bind(reservation.id).bind(last.release_id).bind(last.ordinal).bind(last.track_id).bind(last.asset_id).bind(&last.purpose).execute(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(false)
    }
    async fn capture_maintenance_aux_snapshot_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        budget(&mut tx).await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let row:Option<(String,String,Option<Uuid>,Option<Uuid>)>=sqlx::query_as("SELECT state,capture_phase,aux_release,aux_id FROM admin_media_snapshots WHERE reservation_id=$1 FOR UPDATE").bind(reservation.id).fetch_optional(&mut *tx).await?;
        let (state, phase, last_release, last_id) = row.ok_or_else(invalid)?;
        if state == "frozen" {
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(true);
        }
        if state != "capturing" {
            return Err(invalid());
        }
        let kind: String = sqlx::query_scalar(
            "SELECT kind FROM admin_media_delete_scopes WHERE reservation_id=$1",
        )
        .bind(reservation.id)
        .fetch_one(&mut *tx)
        .await?;
        if kind == "track" {
            verify_bound_scope(&mut tx, reservation.id).await?;
            sqlx::query("UPDATE admin_media_snapshots SET state='frozen',capture_phase='complete' WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(true);
        }
        match phase.as_str() {
            "directories" => {
                let rows:Vec<(Uuid,Option<String>,Option<String>)>=sqlx::query_as("SELECT r.release_id,p.target_relative_path,h.staging_relative_path FROM admin_media_reservation_releases r LEFT JOIN release_publication_plans p ON p.release_id=r.release_id LEFT JOIN release_work_item_staging_handoffs h ON h.work_item_id=p.work_item_id WHERE r.reservation_id=$1 AND ($2::uuid IS NULL OR r.release_id>$2) ORDER BY r.release_id LIMIT 8")
                    .bind(reservation.id).bind(last_release).fetch_all(&mut *tx).await?;
                if rows.is_empty() {
                    sqlx::query("UPDATE admin_media_snapshots SET capture_phase='entity_images',aux_release=NULL,aux_id=NULL WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                } else {
                    for (release, canonical, staging) in &rows {
                        for (kind, path) in [
                            ("canonical_directory", canonical),
                            ("staging_directory", staging),
                        ] {
                            if let Some(path) = path {
                                record_object(
                                    &mut tx,
                                    reservation.id,
                                    &AdminMediaSnapshotObject {
                                        kind: kind.to_owned(),
                                        object_id: *release,
                                        object_key: path.clone(),
                                        byte_hash: None,
                                        byte_size: None,
                                    },
                                )
                                .await?;
                            }
                        }
                    }
                    let last = rows.last().ok_or_else(invalid)?.0;
                    sqlx::query(
                        "UPDATE admin_media_snapshots SET aux_release=$2 WHERE reservation_id=$1",
                    )
                    .bind(reservation.id)
                    .bind(last)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            "entity_images" => {
                let rows:Vec<(Uuid,String,i64)>=sqlx::query_as("SELECT i.release_id,i.asset_key,f.size_bytes::bigint FROM admin_media_reservation_releases r JOIN catalog_entity_images i ON i.release_id=r.release_id JOIN asset_files f ON f.asset_key=i.asset_key WHERE r.reservation_id=$1 AND ($2::uuid IS NULL OR i.release_id>$2) ORDER BY i.release_id LIMIT 16")
                    .bind(reservation.id).bind(last_release).fetch_all(&mut *tx).await?;
                if rows.is_empty() {
                    sqlx::query("UPDATE admin_media_snapshots SET capture_phase='provider_images',aux_release=NULL,aux_id=NULL WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                } else {
                    let keys: Vec<String> = rows.iter().map(|row| row.1.clone()).collect();
                    let exclusive = exclusive_artwork(&mut tx, reservation.id, &keys).await?;
                    for (id, key, size) in &rows {
                        if exclusive.contains(key) {
                            let digest = key.split_once('-').ok_or_else(invalid)?.0.to_owned();
                            record_object(
                                &mut tx,
                                reservation.id,
                                &AdminMediaSnapshotObject {
                                    kind: "artwork".to_owned(),
                                    object_id: *id,
                                    object_key: key.clone(),
                                    byte_hash: Some(digest),
                                    byte_size: Some(*size),
                                },
                            )
                            .await?;
                        }
                    }
                    let last = rows.last().ok_or_else(invalid)?.0;
                    sqlx::query(
                        "UPDATE admin_media_snapshots SET aux_release=$2 WHERE reservation_id=$1",
                    )
                    .bind(reservation.id)
                    .bind(last)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            "provider_images" => {
                let rows:Vec<(Uuid,Uuid,String,i64)>=sqlx::query_as("SELECT i.entity_id,i.id,i.asset_key,f.size_bytes::bigint FROM admin_media_reservation_releases r JOIN catalog_provider_images i ON i.entity_type='release' AND i.entity_id=r.release_id JOIN asset_files f ON f.asset_key=i.asset_key WHERE r.reservation_id=$1 AND ($2::uuid IS NULL OR (i.entity_id,i.id)>($2,$3)) ORDER BY i.entity_id,i.id LIMIT 16")
                    .bind(reservation.id).bind(last_release).bind(last_id).fetch_all(&mut *tx).await?;
                if rows.is_empty() && kind == "artist" {
                    sqlx::query("UPDATE admin_media_snapshots SET capture_phase='artist_images',aux_release=NULL,aux_id=NULL WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                    finish(&mut tx, expiry).await?;
                    tx.commit().await?;
                    return Ok(false);
                }
                if rows.is_empty() {
                    verify_bound_scope(&mut tx, reservation.id).await?;
                    sqlx::query("UPDATE admin_media_snapshots SET state='frozen',capture_phase='complete',aux_release=NULL,aux_id=NULL WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                    finish(&mut tx, expiry).await?;
                    tx.commit().await?;
                    return Ok(true);
                }
                let keys: Vec<String> = rows.iter().map(|row| row.2.clone()).collect();
                let exclusive = exclusive_artwork(&mut tx, reservation.id, &keys).await?;
                for (_, id, key, size) in &rows {
                    if exclusive.contains(key) {
                        let digest = key.split_once('-').ok_or_else(invalid)?.0.to_owned();
                        record_object(
                            &mut tx,
                            reservation.id,
                            &AdminMediaSnapshotObject {
                                kind: "artwork".to_owned(),
                                object_id: *id,
                                object_key: key.clone(),
                                byte_hash: Some(digest),
                                byte_size: Some(*size),
                            },
                        )
                        .await?;
                    }
                }
                let last = rows.last().ok_or_else(invalid)?;
                sqlx::query("UPDATE admin_media_snapshots SET aux_release=$2,aux_id=$3 WHERE reservation_id=$1").bind(reservation.id).bind(last.0).bind(last.1).execute(&mut *tx).await?;
            }
            "artist_images" => {
                let rows:Vec<(Uuid,String,i64)>=sqlx::query_as("SELECT i.artist_id,i.asset_key,f.size_bytes::bigint FROM admin_media_delete_targets t JOIN catalog_entity_images i ON i.artist_id=t.internal_id JOIN asset_files f ON f.asset_key=i.asset_key WHERE t.reservation_id=$1 AND ($2::uuid IS NULL OR i.artist_id>$2) ORDER BY i.artist_id LIMIT 16")
                    .bind(reservation.id).bind(last_release).fetch_all(&mut *tx).await?;
                if rows.is_empty() {
                    sqlx::query("UPDATE admin_media_snapshots SET capture_phase='artist_provider_images',aux_release=NULL,aux_id=NULL WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                } else {
                    let keys: Vec<String> = rows.iter().map(|row| row.1.clone()).collect();
                    let exclusive = exclusive_artwork(&mut tx, reservation.id, &keys).await?;
                    for (id, key, size) in &rows {
                        if exclusive.contains(key) {
                            let digest = key.split_once('-').ok_or_else(invalid)?.0.to_owned();
                            record_object(
                                &mut tx,
                                reservation.id,
                                &AdminMediaSnapshotObject {
                                    kind: "artwork".to_owned(),
                                    object_id: *id,
                                    object_key: key.clone(),
                                    byte_hash: Some(digest),
                                    byte_size: Some(*size),
                                },
                            )
                            .await?;
                        }
                    }
                    let last = rows.last().ok_or_else(invalid)?.0;
                    sqlx::query(
                        "UPDATE admin_media_snapshots SET aux_release=$2 WHERE reservation_id=$1",
                    )
                    .bind(reservation.id)
                    .bind(last)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            "artist_provider_images" => {
                let rows:Vec<(Uuid,Uuid,String,i64)>=sqlx::query_as("SELECT i.entity_id,i.id,i.asset_key,f.size_bytes::bigint FROM admin_media_delete_targets t JOIN catalog_provider_images i ON i.entity_type='artist' AND i.entity_id=t.internal_id JOIN asset_files f ON f.asset_key=i.asset_key WHERE t.reservation_id=$1 AND ($2::uuid IS NULL OR (i.entity_id,i.id)>($2,$3)) ORDER BY i.entity_id,i.id LIMIT 16")
                    .bind(reservation.id).bind(last_release).bind(last_id).fetch_all(&mut *tx).await?;
                if rows.is_empty() {
                    verify_bound_scope(&mut tx, reservation.id).await?;
                    sqlx::query("UPDATE admin_media_snapshots SET state='frozen',capture_phase='complete',aux_release=NULL,aux_id=NULL WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                    finish(&mut tx, expiry).await?;
                    tx.commit().await?;
                    return Ok(true);
                }
                let keys: Vec<String> = rows.iter().map(|row| row.2.clone()).collect();
                let exclusive = exclusive_artwork(&mut tx, reservation.id, &keys).await?;
                for (_, id, key, size) in &rows {
                    if exclusive.contains(key) {
                        let digest = key.split_once('-').ok_or_else(invalid)?.0.to_owned();
                        record_object(
                            &mut tx,
                            reservation.id,
                            &AdminMediaSnapshotObject {
                                kind: "artwork".to_owned(),
                                object_id: *id,
                                object_key: key.clone(),
                                byte_hash: Some(digest),
                                byte_size: Some(*size),
                            },
                        )
                        .await?;
                    }
                }
                let last = rows.last().ok_or_else(invalid)?;
                sqlx::query("UPDATE admin_media_snapshots SET aux_release=$2,aux_id=$3 WHERE reservation_id=$1").bind(reservation.id).bind(last.0).bind(last.1).execute(&mut *tx).await?;
            }
            _ => return Err(invalid()),
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(false)
    }

    /// Read one bounded pending page. Filesystem capabilities must independently
    /// validate root, paths and content while holding all lifecycle locks.
    pub async fn pending_maintenance_media_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<Vec<AdminMediaSnapshotEntry>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let frozen:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_snapshots WHERE reservation_id=$1 AND state='frozen')").bind(reservation.id).fetch_one(&mut *tx).await?;
        if !frozen {
            return Err(invalid());
        }
        let rows=sqlx::query_as("SELECT asset_id,relative_path,byte_hash,byte_size FROM admin_media_snapshot_entries WHERE reservation_id=$1 AND acknowledgement IS NULL ORDER BY asset_id LIMIT 16").bind(reservation.id).fetch_all(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Acknowledge exactly the current page after private filesystem verification.
    /// Replaying the same acknowledgement ID and exact page is idempotent.
    pub async fn acknowledge_maintenance_media_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        ack: Uuid,
        assets: &[Uuid],
    ) -> Result<(), WorkStoreError> {
        if assets.is_empty() || assets.len() > 16 || assets.windows(2).any(|p| p[0] >= p[1]) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let frozen:Option<bool>=sqlx::query_scalar("SELECT true FROM admin_media_snapshots WHERE reservation_id=$1 AND state='frozen' FOR UPDATE").bind(reservation.id).fetch_optional(&mut *tx).await?;
        if frozen.is_none() {
            return Err(invalid());
        }
        let intent_open:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_page_intents WHERE reservation_id=$1 AND state IN ('intent','uncertain'))").bind(reservation.id).fetch_one(&mut *tx).await?;
        if intent_open {
            return Err(invalid());
        }

        let replay:Vec<Uuid>=sqlx::query_scalar("SELECT asset_id FROM admin_media_snapshot_entries WHERE reservation_id=$1 AND acknowledgement=$2 ORDER BY asset_id LIMIT 17").bind(reservation.id).bind(ack).fetch_all(&mut *tx).await?;
        if !replay.is_empty() {
            if replay != assets {
                return Err(invalid());
            }
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(());
        }
        let pending:Vec<Uuid>=sqlx::query_scalar("SELECT asset_id FROM admin_media_snapshot_entries WHERE reservation_id=$1 AND acknowledgement IS NULL ORDER BY asset_id LIMIT 16 FOR UPDATE").bind(reservation.id).fetch_all(&mut *tx).await?;
        if pending != assets {
            return Err(invalid());
        }
        sqlx::query("UPDATE admin_media_snapshot_entries SET acknowledgement=$2 WHERE reservation_id=$1 AND asset_id=ANY($3) AND acknowledgement IS NULL").bind(reservation.id).bind(ack).bind(assets).execute(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Return at most 16 typed directory/artwork objects after closure freeze.
    pub async fn pending_maintenance_object_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<Vec<AdminMediaSnapshotObject>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let frozen:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_snapshots WHERE reservation_id=$1 AND state='frozen')").bind(reservation.id).fetch_one(&mut *tx).await?;
        if !frozen {
            return Err(invalid());
        }
        let rows=sqlx::query_as("SELECT kind,object_id,object_key,byte_hash,byte_size FROM admin_media_snapshot_objects WHERE reservation_id=$1 AND acknowledgement IS NULL ORDER BY kind COLLATE \"C\",object_id LIMIT 16").bind(reservation.id).fetch_all(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Acknowledge the exact current typed page. The caller must hold its
    /// filesystem capabilities and verify every object before this DB update.
    pub async fn acknowledge_maintenance_object_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        ack: Uuid,
        objects: &[AdminMediaObjectKey],
    ) -> Result<(), WorkStoreError> {
        if objects.is_empty()
            || objects.len() > 16
            || objects
                .windows(2)
                .any(|p| (&p[0].kind, p[0].object_id) >= (&p[1].kind, p[1].object_id))
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let frozen:Option<bool>=sqlx::query_scalar("SELECT true FROM admin_media_snapshots WHERE reservation_id=$1 AND state='frozen' FOR UPDATE").bind(reservation.id).fetch_optional(&mut *tx).await?;
        if frozen.is_none() {
            return Err(invalid());
        }
        let intent_open:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_page_intents WHERE reservation_id=$1 AND state IN ('intent','uncertain'))").bind(reservation.id).fetch_one(&mut *tx).await?;
        if intent_open {
            return Err(invalid());
        }

        let replay:Vec<(String,Uuid)>=sqlx::query_as("SELECT kind,object_id FROM admin_media_snapshot_objects WHERE reservation_id=$1 AND acknowledgement=$2 ORDER BY kind COLLATE \"C\",object_id LIMIT 17").bind(reservation.id).bind(ack).fetch_all(&mut *tx).await?;
        let expected: Vec<(String, Uuid)> = objects
            .iter()
            .map(|item| (item.kind.clone(), item.object_id))
            .collect();
        if !replay.is_empty() {
            if replay != expected {
                return Err(invalid());
            }
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(());
        }
        let pending:Vec<(String,Uuid)>=sqlx::query_as("SELECT kind,object_id FROM admin_media_snapshot_objects WHERE reservation_id=$1 AND acknowledgement IS NULL ORDER BY kind COLLATE \"C\",object_id LIMIT 16 FOR UPDATE").bind(reservation.id).fetch_all(&mut *tx).await?;
        if pending != expected {
            return Err(invalid());
        }
        let kinds: Vec<&str> = objects.iter().map(|item| item.kind.as_str()).collect();
        let ids: Vec<Uuid> = objects.iter().map(|item| item.object_id).collect();
        sqlx::query("UPDATE admin_media_snapshot_objects o SET acknowledgement=$2 FROM unnest($3::text[],$4::uuid[]) k(kind,id) WHERE o.reservation_id=$1 AND o.kind=k.kind AND o.object_id=k.id AND o.acknowledgement IS NULL").bind(reservation.id).bind(ack).bind(&kinds).bind(&ids).execute(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Bounded rollback before any filesystem page has been acknowledged.
    /// Once an acknowledgement exists the durable snapshot is retained for
    /// completion or explicit repair; it may not be silently abandoned.
    pub async fn discard_maintenance_media_snapshot_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        budget(&mut tx).await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM admin_media_snapshots WHERE reservation_id=$1 FOR UPDATE",
        )
        .bind(reservation.id)
        .fetch_optional(&mut *tx)
        .await?;
        if state.is_none() {
            let complete = discard_detached_roots(&mut tx, reservation.id).await?;
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(complete);
        }
        let has_intent: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM admin_media_page_intents WHERE reservation_id=$1)",
        )
        .bind(reservation.id)
        .fetch_one(&mut *tx)
        .await?;
        if has_intent {
            return Err(invalid());
        }

        let acknowledged:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_snapshot_entries WHERE reservation_id=$1 AND acknowledgement IS NOT NULL) OR EXISTS(SELECT 1 FROM admin_media_snapshot_objects WHERE reservation_id=$1 AND acknowledgement IS NOT NULL)").bind(reservation.id).fetch_one(&mut *tx).await?;
        if acknowledged {
            return Err(invalid());
        }
        if state.as_deref() != Some("discarding") {
            sqlx::query("DELETE FROM admin_media_coverage_audits WHERE reservation_id=$1")
                .bind(reservation.id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE admin_media_snapshots SET state='discarding' WHERE reservation_id=$1",
            )
            .bind(reservation.id)
            .execute(&mut *tx)
            .await?;
        }
        let mut paths:Vec<String>=sqlx::query_scalar("DELETE FROM admin_media_snapshot_entries WHERE (reservation_id,asset_id) IN (SELECT reservation_id,asset_id FROM admin_media_snapshot_entries WHERE reservation_id=$1 ORDER BY asset_id LIMIT 16) RETURNING relative_path").bind(reservation.id).fetch_all(&mut *tx).await?;
        if paths.is_empty() {
            paths=sqlx::query_scalar("DELETE FROM admin_media_snapshot_objects WHERE (reservation_id,kind,object_id) IN (SELECT reservation_id,kind,object_id FROM admin_media_snapshot_objects WHERE reservation_id=$1 ORDER BY kind COLLATE \"C\",object_id LIMIT 16) RETURNING object_key").bind(reservation.id).fetch_all(&mut *tx).await?;
        }
        if !paths.is_empty() {
            let rows = i64::try_from(paths.len()).map_err(|_| invalid())?;
            let bytes = paths
                .iter()
                .try_fold(0_i64, |sum, path| {
                    i64::try_from(path.len() + 256)
                        .ok()
                        .and_then(|cost| sum.checked_add(cost))
                })
                .ok_or_else(invalid)?;
            sqlx::query("UPDATE admin_media_snapshot_budget SET entries=entries-$1,bytes=bytes-$2 WHERE singleton").bind(rows).bind(bytes).execute(&mut *tx).await?;
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(false);
        }
        sqlx::query("DELETE FROM admin_media_coverage_audits WHERE reservation_id=$1")
            .bind(reservation.id)
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query(
            "DELETE FROM admin_media_snapshots WHERE reservation_id=$1 AND state='discarding'",
        )
        .bind(reservation.id)
        .execute(&mut *tx)
        .await?;
        if deleted.rows_affected() != 1 {
            return Err(invalid());
        }
        sqlx::query("UPDATE admin_media_snapshot_budget SET snapshots=snapshots-1 WHERE singleton")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM admin_media_delete_targets WHERE reservation_id=$1")
            .bind(reservation.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM admin_media_delete_scopes WHERE reservation_id=$1")
            .bind(reservation.id)
            .execute(&mut *tx)
            .await?;
        let complete = discard_detached_roots(&mut tx, reservation.id).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(complete)
    }
}

// Snapshot removal is permitted only before an intent or acknowledgement.
// Detached root metadata consequently represents no attempted filesystem
// work. Bound deletions to sixteen records so interruption is replayable.
async fn discard_detached_roots(
    tx: &mut Transaction<'_, Postgres>,
    reservation: Uuid,
) -> Result<bool, WorkStoreError> {
    let revision: Option<Uuid> = sqlx::query_scalar(
        "SELECT revision FROM admin_media_root_sets WHERE reservation_id=$1 FOR UPDATE",
    )
    .bind(reservation)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(revision) = revision else {
        return Ok(true);
    };
    let rows = sqlx::query("DELETE FROM admin_media_root_entries WHERE (revision,ordinal) IN (SELECT revision,ordinal FROM admin_media_root_entries WHERE revision=$1 ORDER BY ordinal LIMIT 16)")
        .bind(revision).execute(&mut **tx).await?;
    if rows.rows_affected() != 0 {
        return Ok(false);
    }
    sqlx::query("DELETE FROM admin_media_root_sets WHERE revision=$1")
        .bind(revision)
        .execute(&mut **tx)
        .await?;
    Ok(true)
}
