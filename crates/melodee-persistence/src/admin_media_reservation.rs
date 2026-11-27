//! Durable, bounded administrative media-operation intent. A reservation is
//! not a filesystem capability; callers must separately hold and verify all
//! relevant lifecycle locks through actual worker termination and cleanup.
use crate::{WorkItemStore, WorkStoreError, work_items};
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const MAX_SCOPE: usize = 128;
#[derive(Debug, Clone)]
pub struct AdminMediaReservation {
    pub(crate) id: Uuid,
    pub(crate) owner: String,
    pub(crate) token: Uuid,
}
impl AdminMediaReservation {
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Transfer the original database fence values to the private custody
    /// executor. These credentials do not establish any filesystem authority.
    #[cfg(target_os = "linux")]
    pub fn custody_credentials(
        &self,
        fence: &crate::LifecycleMaintenance,
    ) -> Result<melodee_custody::MaintenanceCredentials, WorkStoreError> {
        if self.owner != fence.owner {
            return Err(invalid());
        }
        melodee_custody::MaintenanceCredentials::from_parts(
            self.owner.clone(),
            self.id,
            self.token,
            fence.token,
        )
        .map_err(|_| invalid())
    }
}
/// Immutable exact identities to compare with held private lifecycle locks.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminReservedLifecycle {
    pub work_item_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub source_path_format: i16,
    pub lock_name: String,
    pub lock_root_generation: String,
    pub lock_generation: String,
    pub stage_root_generation: String,
}
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid or conflicting administrative media reservation")
}
fn valid_scope(ids: &[Uuid]) -> bool {
    if ids.len() > MAX_SCOPE {
        return false;
    }
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.len() == ids.len()
}
/// The release lock follows the item row lock. Both reservation creation and
/// publication binding take this advisory lock, then perform a fresh query.
pub(crate) async fn lock_release(
    tx: &mut Transaction<'_, Postgres>,
    release: Uuid,
) -> Result<(), WorkStoreError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('melodee.admin-release:' || $1::uuid::text,0))").bind(release).execute(&mut **tx).await?;
    Ok(())
}
pub(crate) async fn ensure_release_unreserved(
    tx: &mut Transaction<'_, Postgres>,
    release: Uuid,
) -> Result<(), WorkStoreError> {
    lock_release(tx, release).await?;
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM admin_media_reservation_releases WHERE release_id=$1)",
    )
    .bind(release)
    .fetch_one(&mut **tx)
    .await?;
    if blocked {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) async fn lock_budget(tx: &mut Transaction<'_, Postgres>) -> Result<(), WorkStoreError> {
    sqlx::query("SELECT singleton FROM admin_media_reservation_budget WHERE singleton FOR UPDATE")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub(crate) async fn verify_lease(
    tx: &mut Transaction<'_, Postgres>,
    reservation: &AdminMediaReservation,
) -> Result<OffsetDateTime, WorkStoreError> {
    let expiry:Option<OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM admin_media_reservations r WHERE id=$1 AND owner=$2 AND token=$3 AND lease_expires_at>clock_timestamp() AND NOT EXISTS(SELECT 1 FROM admin_media_completion_receipts c WHERE c.reservation_id=r.id) FOR UPDATE")
        .bind(reservation.id).bind(&reservation.owner).bind(reservation.token).fetch_optional(&mut **tx).await?;
    expiry.ok_or_else(invalid)
}
pub(crate) async fn check_expiry(
    tx: &mut Transaction<'_, Postgres>,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !live {
        return Err(invalid());
    }
    Ok(())
}
/// Lock a single-item review intent before its final transaction. The caller
/// must still hold the filesystem lifecycle capability throughout completion.
pub(crate) async fn lock_review_reservation(
    tx: &mut Transaction<'_, Postgres>,
    reservation: &AdminMediaReservation,
    item: Uuid,
) -> Result<OffsetDateTime, WorkStoreError> {
    lock_budget(tx).await?;
    let expiry = verify_lease(tx, reservation).await?;
    let valid:bool=sqlx::query_scalar("SELECT (SELECT count(*) FROM admin_media_reservation_items WHERE reservation_id=$1)=1 AND EXISTS(SELECT 1 FROM admin_media_reservation_items WHERE reservation_id=$1 AND work_item_id=$2) AND NOT EXISTS(SELECT 1 FROM admin_media_reservation_releases WHERE reservation_id=$1)")
        .bind(reservation.id).bind(item).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(invalid());
    }
    Ok(expiry)
}
pub(crate) async fn consume_review_reservation(
    tx: &mut Transaction<'_, Postgres>,
    reservation: &AdminMediaReservation,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    sqlx::query("DELETE FROM admin_media_reservation_items WHERE reservation_id=$1")
        .bind(reservation.id)
        .execute(&mut **tx)
        .await?;
    let removed =
        sqlx::query("DELETE FROM admin_media_reservations WHERE id=$1 AND token=$2 AND owner=$3")
            .bind(reservation.id)
            .bind(reservation.token)
            .bind(&reservation.owner)
            .execute(&mut **tx)
            .await?;
    if removed.rows_affected() != 1 {
        return Err(invalid());
    }
    sqlx::query(
        "UPDATE admin_media_reservation_budget SET reservations=reservations-1 WHERE singleton",
    )
    .execute(&mut **tx)
    .await?;
    check_expiry(tx, expiry).await?;
    Ok(())
}
impl WorkItemStore {
    /// Reserve a complete release/item scope before filesystem work. Membership
    /// is derived again under ordered release locks; no caller-supplied path is
    /// accepted as authority. Expired intents remain durable and fail closed.
    pub async fn begin_admin_media_reservation(
        &self,
        pool: &PgPool,
        items: &[Uuid],
        releases: &[Uuid],
        owner: &str,
        duration: Duration,
    ) -> Result<AdminMediaReservation, WorkStoreError> {
        self.begin_admin_media_reservation_inner(pool, items, releases, owner, duration, None)
            .await
    }
    /// Begin under an already externally quiesced maintenance fence. This
    /// does not itself prove that old processes or filesystem workers stopped.
    pub async fn begin_maintenance_admin_media_reservation(
        &self,
        pool: &PgPool,
        items: &[Uuid],
        releases: &[Uuid],
        owner: &str,
        duration: Duration,
        fence: &crate::LifecycleMaintenance,
    ) -> Result<AdminMediaReservation, WorkStoreError> {
        self.begin_admin_media_reservation_inner(
            pool,
            items,
            releases,
            owner,
            duration,
            Some(fence),
        )
        .await
    }
    async fn begin_admin_media_reservation_inner(
        &self,
        pool: &PgPool,
        items: &[Uuid],
        releases: &[Uuid],
        owner: &str,
        duration: Duration,
        maintenance: Option<&crate::LifecycleMaintenance>,
    ) -> Result<AdminMediaReservation, WorkStoreError> {
        let ms = i64::try_from(duration.whole_milliseconds()).map_err(|_| invalid())?;
        if !valid_scope(items)
            || !valid_scope(releases)
            || (items.is_empty() && releases.is_empty() && maintenance.is_none())
            || owner.trim().is_empty()
            || owner.len() > 512
            || !(1..=3_600_000).contains(&ms)
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let maintenance_expiry = if let Some(fence) = maintenance {
            if fence.owner != owner {
                return Err(invalid());
            }
            Some(crate::work_item_lifecycle::lock_fence(&mut tx, fence, true).await?)
        } else {
            crate::work_item_lifecycle::admit_runtime(&mut tx).await?;
            None
        };
        lock_budget(&mut tx).await?;
        let initial:Vec<Uuid>=sqlx::query_scalar("SELECT work_item_id FROM catalog_imports WHERE release_id=ANY($1) UNION SELECT work_item_id FROM release_publication_plans WHERE release_id=ANY($1) ORDER BY work_item_id LIMIT 129").bind(releases).fetch_all(&mut *tx).await?;
        let mut scope = items.to_vec();
        scope.extend(initial);
        scope.sort_unstable();
        scope.dedup();
        if scope.len() > MAX_SCOPE {
            return Err(invalid());
        }
        let rows: Vec<work_items::WorkItemRow> = sqlx::query_as(
            "SELECT * FROM release_work_items WHERE id=ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(&scope)
        .fetch_all(&mut *tx)
        .await?;
        if rows.len() != scope.len() {
            return Err(invalid());
        }
        for row in rows {
            let item = melodee_domain::ReleaseWorkItem::try_from(row)?;
            work_items::try_lock_unguarded_occurrence(&mut tx, &item).await?;
            let blocked:bool=sqlx::query_scalar("SELECT protected_custody_required OR COALESCE(lease_expires_at>clock_timestamp(),false) OR EXISTS(SELECT 1 FROM inbound_retained_sources r WHERE r.status='active' AND r.source_library_id IS NOT DISTINCT FROM $2 AND ((ingest_occurrence_hash(r.source_path)=ingest_occurrence_hash($3) AND r.source_path COLLATE \"C\"=$3 COLLATE \"C\") OR (r.source_path_format=0 AND strpos(r.source_path,chr(65533))>0))) OR EXISTS(SELECT 1 FROM admin_media_reservation_items g WHERE g.source_library_id IS NOT DISTINCT FROM $2 AND ingest_occurrence_hash(g.source_path)=ingest_occurrence_hash($3) AND g.source_path COLLATE \"C\"=$3 COLLATE \"C\") FROM release_work_items WHERE id=$1")
                .bind(item.id).bind(item.source_library_id).bind(&item.source_path).fetch_one(&mut *tx).await?;
            if blocked {
                return Err(invalid());
            }
            let competing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM release_work_items w WHERE w.id<>$1 AND w.source_library_id IS NOT DISTINCT FROM $2 AND ingest_occurrence_hash(w.source_path)=ingest_occurrence_hash($3) AND w.source_path COLLATE \"C\"=$3 COLLATE \"C\" AND w.inbound_active)")
                .bind(item.id).bind(item.source_library_id).bind(&item.source_path).fetch_one(&mut *tx).await?;
            if competing {
                return Err(invalid());
            }
        }
        let mut ordered_releases = releases.to_vec();
        ordered_releases.sort_unstable();
        for release in &ordered_releases {
            ensure_release_unreserved(&mut tx, *release).await?;
        }
        let final_scope:Vec<Uuid>=sqlx::query_scalar("SELECT work_item_id FROM catalog_imports WHERE release_id=ANY($1) UNION SELECT work_item_id FROM release_publication_plans WHERE release_id=ANY($1) ORDER BY work_item_id LIMIT 129").bind(releases).fetch_all(&mut *tx).await?;
        if final_scope
            .iter()
            .any(|id| scope.binary_search(id).is_err())
        {
            return Err(invalid());
        }
        let count=sqlx::query("UPDATE admin_media_reservation_budget SET reservations=reservations+1 WHERE singleton AND reservations<max_reservations").execute(&mut *tx).await?;
        if count.rows_affected() != 1 {
            return Err(invalid());
        }
        let id = Uuid::now_v7();
        let token = Uuid::now_v7();
        let expiry:OffsetDateTime=sqlx::query_scalar("INSERT INTO admin_media_reservations(id,owner,token,lease_expires_at) VALUES($1,$2,$3,clock_timestamp()+($4::bigint*interval '1 millisecond')) RETURNING lease_expires_at").bind(id).bind(owner).bind(token).bind(ms).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO admin_media_reservation_items(work_item_id,reservation_id,source_library_id,source_path) SELECT id,$2,source_library_id,source_path FROM release_work_items WHERE id=ANY($1)").bind(&scope).bind(id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO admin_media_reservation_releases(release_id,reservation_id) SELECT unnest($1::uuid[]),$2").bind(&ordered_releases).bind(id).execute(&mut *tx).await?;
        check_expiry(&mut tx, expiry).await?;
        if let Some(maintenance_expiry) = maintenance_expiry {
            crate::work_item_lifecycle::finish(&mut tx, maintenance_expiry).await?;
        }
        tx.commit().await?;
        Ok(AdminMediaReservation {
            id,
            owner: owner.to_owned(),
            token,
        })
    }
    /// Reclaim an expired intent only after the private caller has acquired
    /// and revalidated all filesystem lifecycle locks. Database expiry cannot
    /// revoke an older process's open descriptors. This rotates the token and
    /// leaves the exact reserved scope unchanged.
    pub async fn reclaim_admin_media_reservation(
        &self,
        pool: &PgPool,
        id: Uuid,
        owner: &str,
        duration: Duration,
    ) -> Result<AdminMediaReservation, WorkStoreError> {
        let ms = i64::try_from(duration.whole_milliseconds()).map_err(|_| invalid())?;
        if owner.trim().is_empty() || owner.len() > 512 || !(1..=3_600_000).contains(&ms) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let token = Uuid::now_v7();
        let expiry:Option<OffsetDateTime>=sqlx::query_scalar("UPDATE admin_media_reservations SET owner=$2,token=$3,lease_expires_at=clock_timestamp()+($4::bigint*interval '1 millisecond') WHERE id=$1 AND lease_expires_at<=clock_timestamp() RETURNING lease_expires_at")
            .bind(id).bind(owner).bind(token).bind(ms).fetch_optional(&mut *tx).await?;
        let expiry = expiry.ok_or_else(invalid)?;
        check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(AdminMediaReservation {
            id,
            owner: owner.to_owned(),
            token,
        })
    }

    /// Read at most 16 exact identities in work-item order. Every item in the
    /// page must have a ready registry entry; absence is never interpreted as
    /// permission to proceed without a lock.
    pub async fn admin_reservation_lifecycle_page(
        &self,
        pool: &PgPool,
        reservation: &AdminMediaReservation,
        after: Option<Uuid>,
    ) -> Result<Vec<AdminReservedLifecycle>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = verify_lease(&mut tx, reservation).await?;
        let ids:Vec<Uuid>=sqlx::query_scalar("SELECT work_item_id FROM admin_media_reservation_items WHERE reservation_id=$1 AND ($2::uuid IS NULL OR work_item_id>$2) ORDER BY work_item_id LIMIT 16").bind(reservation.id).bind(after).fetch_all(&mut *tx).await?;
        let rows:Vec<AdminReservedLifecycle>=sqlx::query_as("SELECT l.work_item_id,l.source_library_id,l.source_path,l.source_path_format,l.lock_name,l.lock_root_generation,l.lock_generation,l.stage_root_generation FROM work_item_lifecycle_locks l JOIN admin_media_reservation_items g USING(work_item_id) JOIN release_work_items w ON w.id=l.work_item_id WHERE g.reservation_id=$1 AND l.work_item_id=ANY($2) AND l.state='ready' AND NOT EXISTS(SELECT 1 FROM work_item_lifecycle_quarantine q WHERE q.work_item_id=l.work_item_id) AND l.source_library_id IS NOT DISTINCT FROM g.source_library_id AND l.source_path COLLATE \"C\"=g.source_path COLLATE \"C\" AND w.source_library_id IS NOT DISTINCT FROM l.source_library_id AND w.source_path COLLATE \"C\"=l.source_path COLLATE \"C\" AND w.discovery_path_format=l.source_path_format ORDER BY l.work_item_id")
            .bind(reservation.id).bind(&ids).fetch_all(&mut *tx).await?;
        if rows.len() != ids.len() {
            return Err(invalid());
        }
        check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Recheck the complete exact scope after private callers hold every lock.
    /// This transaction does not itself acquire or prove a filesystem lock.
    pub async fn revalidate_admin_reservation_lifecycles(
        &self,
        pool: &PgPool,
        reservation: &AdminMediaReservation,
        held: &[AdminReservedLifecycle],
    ) -> Result<(), WorkStoreError> {
        if held.len() > MAX_SCOPE
            || held
                .windows(2)
                .any(|pair| pair[0].work_item_id >= pair[1].work_item_id)
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = verify_lease(&mut tx, reservation).await?;
        let rows:Vec<AdminReservedLifecycle>=sqlx::query_as("SELECT l.work_item_id,l.source_library_id,l.source_path,l.source_path_format,l.lock_name,l.lock_root_generation,l.lock_generation,l.stage_root_generation FROM admin_media_reservation_items g JOIN work_item_lifecycle_locks l USING(work_item_id) JOIN release_work_items w ON w.id=l.work_item_id WHERE g.reservation_id=$1 AND l.state='ready' AND NOT EXISTS(SELECT 1 FROM work_item_lifecycle_quarantine q WHERE q.work_item_id=l.work_item_id) AND l.source_library_id IS NOT DISTINCT FROM g.source_library_id AND l.source_path COLLATE \"C\"=g.source_path COLLATE \"C\" AND w.source_library_id IS NOT DISTINCT FROM l.source_library_id AND w.source_path COLLATE \"C\"=l.source_path COLLATE \"C\" AND w.discovery_path_format=l.source_path_format ORDER BY l.work_item_id FOR SHARE OF w,l")
            .bind(reservation.id).fetch_all(&mut *tx).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM admin_media_reservation_items WHERE reservation_id=$1",
        )
        .bind(reservation.id)
        .fetch_one(&mut *tx)
        .await?;
        if rows != held || i64::try_from(rows.len()).ok() != Some(count) {
            return Err(invalid());
        }
        check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Renew only the original live lease. Expiration never clears its intent.
    pub async fn renew_admin_media_reservation(
        &self,
        pool: &PgPool,
        reservation: &AdminMediaReservation,
        duration: Duration,
    ) -> Result<(), WorkStoreError> {
        let ms = i64::try_from(duration.whole_milliseconds()).map_err(|_| invalid())?;
        if !(1..=3_600_000).contains(&ms) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = verify_lease(&mut tx, reservation).await?;
        sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()+($2::bigint*interval '1 millisecond') WHERE id=$1").bind(reservation.id).bind(ms).execute(&mut *tx).await?;
        check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Release after the private caller has terminated all filesystem work and
    /// durably completed or safely abandoned the operation under its locks.
    pub async fn finish_admin_media_reservation(
        &self,
        pool: &PgPool,
        reservation: &AdminMediaReservation,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_budget(&mut tx).await?;
        let expiry = verify_lease(&mut tx, reservation).await?;
        sqlx::query("DELETE FROM admin_media_reservation_items WHERE reservation_id=$1")
            .bind(reservation.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM admin_media_reservation_releases WHERE reservation_id=$1")
            .bind(reservation.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM admin_media_reservations WHERE id=$1 AND token=$2")
            .bind(reservation.id)
            .bind(reservation.token)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE admin_media_reservation_budget SET reservations=reservations-1 WHERE singleton",
        )
        .execute(&mut *tx)
        .await?;
        check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
}
