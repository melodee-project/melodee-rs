//! Maintenance-fenced lifecycle identity metadata. No method establishes that
//! filesystem locks are held, workers have terminated, or namespaces are safe.
use crate::{ProtectedClaimEpoch, WorkItemStore, WorkStoreError};
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct LifecycleMaintenance {
    pub(crate) owner: String,
    pub(crate) token: Uuid,
}
/// Explicit operator assertions; PostgreSQL cannot verify that old writers
/// or filesystem workers have actually stopped.
pub struct LifecycleQuiescenceAttestation<'a> {
    pub reason: &'a str,
    pub backup_reference: &'a str,
    pub stop_all_writers: bool,
    pub inspection_complete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct WorkItemLifecycleIdentity {
    pub work_item_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub source_path_format: i16,
    pub lock_name: String,
    pub state: String,
    pub lock_root_generation: Option<String>,
    pub lock_generation: Option<String>,
    pub stage_root_generation: Option<String>,
}
/// Lease for one provisional new-item lifecycle job. It carries no authority
/// over an existing filesystem object and its fields cannot be fabricated.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LifecycleProvisionLease {
    work_item_id: Uuid,
    owner: String,
    token: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub source_path_format: i16,
    pub lock_name: String,
}
impl LifecycleProvisionLease {
    pub fn work_item_id(&self) -> Uuid {
        self.work_item_id
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleQuarantineReason {
    Ambiguous,
    Missing,
    Changed,
    Unreadable,
    Unsupported,
}
impl LifecycleQuarantineReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ambiguous => "ambiguous",
            Self::Missing => "missing",
            Self::Changed => "changed",
            Self::Unreadable => "unreadable",
            Self::Unsupported => "unsupported",
        }
    }
}
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("lifecycle maintenance fence or identity changed")
}
fn valid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512 && !s.contains('\0')
}
pub(crate) async fn lock_fence(
    tx: &mut Transaction<'_, Postgres>,
    fence: &LifecycleMaintenance,
    quiesced: bool,
) -> Result<OffsetDateTime, WorkStoreError> {
    let expiry:Option<OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM work_item_lifecycle_maintenance WHERE singleton AND owner=$1 AND token=$2 AND lease_expires_at>clock_timestamp() AND (NOT $3 OR quiesced) FOR UPDATE")
        .bind(&fence.owner).bind(fence.token).bind(quiesced).fetch_optional(&mut **tx).await?.flatten();
    expiry.ok_or_else(invalid)
}
/// Fencing for read-only recovery observation only: a live fence that is
/// either generically quiesced or bound to exactly this recovery reservation.
/// Mutation paths must use [`lock_fence`], which never accepts the recovery
/// binding, so a recovery attestation cannot authorize adoption or deletion.
pub(crate) async fn lock_fence_recovery(
    tx: &mut Transaction<'_, Postgres>,
    fence: &LifecycleMaintenance,
    reservation_id: Uuid,
) -> Result<OffsetDateTime, WorkStoreError> {
    let expiry:Option<OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM work_item_lifecycle_maintenance WHERE singleton AND owner=$1 AND token=$2 AND lease_expires_at>clock_timestamp() AND (quiesced OR (recovery_reservation_id=$3 AND NOT quiesced)) FOR UPDATE")
        .bind(&fence.owner).bind(fence.token).bind(reservation_id).fetch_optional(&mut **tx).await?.flatten();
    expiry.ok_or_else(invalid)
}
pub(crate) async fn finish(
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
fn millis(duration: Duration) -> Result<i64, WorkStoreError> {
    let value = i64::try_from(duration.whole_milliseconds()).map_err(|_| invalid())?;
    if !(1..=3_600_000).contains(&value) {
        return Err(invalid());
    }
    Ok(value)
}
async fn ensure_next(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<(), WorkStoreError> {
    let cursor: Option<Uuid> =
        sqlx::query_scalar("SELECT cursor_id FROM work_item_lifecycle_maintenance WHERE singleton")
            .fetch_one(&mut **tx)
            .await?;
    let next: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM release_work_items WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 1",
    )
    .bind(cursor)
    .fetch_optional(&mut **tx)
    .await?;
    if next != Some(id) {
        return Err(invalid());
    }
    Ok(())
}
/// Shared transaction admission serializes new work with activation of the
/// maintenance fence. It does not replace externally enforced quiescence.
pub(crate) async fn admit_runtime(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared(1869834358,1)")
        .execute(&mut **tx)
        .await?;
    let active: bool = sqlx::query_scalar(
        "SELECT owner IS NOT NULL FROM work_item_lifecycle_maintenance WHERE singleton",
    )
    .fetch_one(&mut **tx)
    .await?;
    if active {
        return Err(sqlx::Error::Protocol(
            "lifecycle maintenance is active".to_owned(),
        ));
    }
    Ok(())
}
async fn lock_provision(
    tx: &mut Transaction<'_, Postgres>,
    lease: &LifecycleProvisionLease,
) -> Result<OffsetDateTime, WorkStoreError> {
    let expiry:Option<OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM work_item_lifecycle_provision_jobs WHERE work_item_id=$1 AND owner=$2 AND token=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE")
        .bind(lease.work_item_id).bind(&lease.owner).bind(lease.token).fetch_optional(&mut **tx).await?.flatten();
    expiry.ok_or_else(invalid)
}
impl WorkItemStore {
    /// Enter a durable fence before provisioning. A lost or expired owner does
    /// not automatically clear the fence. Caller must arrange maintenance stop.
    pub async fn begin_lifecycle_maintenance(
        &self,
        pool: &PgPool,
        owner: &str,
        duration: Duration,
    ) -> Result<LifecycleMaintenance, WorkStoreError> {
        let ms = millis(duration)?;
        if owner.trim().is_empty() || owner.len() > 512 {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(1869834358,1)")
            .execute(&mut *tx)
            .await?;
        let token = Uuid::now_v7();
        let expiry:Option<OffsetDateTime>=sqlx::query_scalar("UPDATE work_item_lifecycle_maintenance SET owner=$1,token=$2,lease_expires_at=clock_timestamp()+($3::bigint*interval '1 millisecond'),quiesced=false,pass_complete=false WHERE singleton AND owner IS NULL RETURNING lease_expires_at")
            .bind(owner).bind(token).bind(ms).fetch_optional(&mut *tx).await?;
        let expiry = expiry.ok_or_else(invalid)?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(LifecycleMaintenance {
            owner: owner.to_owned(),
            token,
        })
    }
    /// Recover an expired fence after external maintenance has verified all
    /// preceding filesystem work terminated. Recovery revokes quiescence.
    pub async fn reclaim_lifecycle_maintenance(
        &self,
        pool: &PgPool,
        owner: &str,
        duration: Duration,
    ) -> Result<LifecycleMaintenance, WorkStoreError> {
        let ms = millis(duration)?;
        if owner.trim().is_empty() || owner.len() > 512 {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let token = Uuid::now_v7();
        let expiry:Option<OffsetDateTime>=sqlx::query_scalar("UPDATE work_item_lifecycle_maintenance SET owner=$1,token=$2,lease_expires_at=clock_timestamp()+($3::bigint*interval '1 millisecond'),quiesced=false,recovery_reservation_id=NULL,recovery_reservation_token=NULL WHERE singleton AND owner IS NOT NULL AND lease_expires_at<=clock_timestamp() RETURNING lease_expires_at")
            .bind(owner).bind(token).bind(ms).fetch_optional(&mut *tx).await?;
        let expiry = expiry.ok_or_else(invalid)?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(LifecycleMaintenance {
            owner: owner.to_owned(),
            token,
        })
    }
    /// The caller must first establish externally that all old binaries and
    /// filesystem workers are stopped. A database lease check is not proof of
    /// that condition; it is an additional fail-closed check only.
    pub async fn confirm_lifecycle_quiescence(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        attestation: &LifecycleQuiescenceAttestation<'_>,
    ) -> Result<(), WorkStoreError> {
        self.confirm_lifecycle_quiescence_inner(pool, fence, attestation, None)
            .await
    }
    /// Record a fresh external stop-all-writers attestation for recovery of
    /// exactly one reclaimed reservation. A database lease is not proof that
    /// earlier filesystem workers have stopped, and this method grants no
    /// authority to adopt, delete, or acknowledge their filesystem work. It
    /// keeps generic quiescence unset and binds the attestation to the exact
    /// reservation, so generic maintenance stays unavailable.
    pub async fn confirm_lifecycle_recovery_quiescence(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &crate::AdminMediaReservation,
        attestation: &LifecycleQuiescenceAttestation<'_>,
    ) -> Result<(), WorkStoreError> {
        self.confirm_lifecycle_quiescence_inner(pool, fence, attestation, Some(reservation))
            .await
    }
    async fn confirm_lifecycle_quiescence_inner(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        attestation: &LifecycleQuiescenceAttestation<'_>,
        recovery: Option<&crate::AdminMediaReservation>,
    ) -> Result<(), WorkStoreError> {
        if !valid(attestation.reason)
            || attestation.reason.trim().is_empty()
            || !valid(attestation.backup_reference)
            || attestation.backup_reference.trim().is_empty()
            || !attestation.stop_all_writers
            || !attestation.inspection_complete
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = lock_fence(&mut tx, fence, false).await?;
        let recovery_expiry = if let Some(reservation) = recovery {
            if reservation.owner != fence.owner {
                return Err(invalid());
            }
            Some(crate::admin_media_reservation::verify_lease(&mut tx, reservation).await?)
        } else {
            None
        };
        let recovery_id = recovery.map(|r| r.id);
        let recovery_token = recovery.map(|r| r.token);
        let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM release_work_items WHERE lease_expires_at>clock_timestamp()) OR EXISTS(SELECT 1 FROM admin_media_reservations WHERE id IS DISTINCT FROM $1)")
            .bind(recovery_id).fetch_one(&mut *tx).await?;
        if live {
            return Err(invalid());
        }
        sqlx::query("SELECT singleton FROM work_item_lifecycle_attestation_budget WHERE singleton FOR UPDATE").execute(&mut *tx).await?;
        #[allow(clippy::type_complexity)]
        let existing:Option<(String,String,String,Option<Uuid>,Option<Uuid>)>=sqlx::query_as("SELECT actor,reason,backup_reference,recovery_reservation_id,recovery_reservation_token FROM work_item_lifecycle_attestations WHERE token=$1").bind(fence.token).fetch_optional(&mut *tx).await?;
        if let Some((actor, reason, backup, previous_id, previous_token)) = existing {
            if actor != fence.owner
                || reason != attestation.reason
                || backup != attestation.backup_reference
                || previous_id != recovery_id
                || previous_token != recovery_token
            {
                return Err(invalid());
            }
        } else {
            let count=sqlx::query("UPDATE work_item_lifecycle_attestation_budget SET entries=entries+1 WHERE singleton AND entries<max_entries").execute(&mut *tx).await?;
            if count.rows_affected() != 1 {
                return Err(invalid());
            }
            sqlx::query("INSERT INTO work_item_lifecycle_attestations(token,actor,reason,backup_reference,recovery_reservation_id,recovery_reservation_token,stop_all_writers,inspection_complete) VALUES($1,$2,$3,$4,$5,$6,true,true)")
                .bind(fence.token).bind(&fence.owner).bind(attestation.reason).bind(attestation.backup_reference).bind(recovery_id).bind(recovery_token).execute(&mut *tx).await?;
        }
        // Recovery and generic quiescence are mutually exclusive fence states:
        // constraint (2) on the maintenance table forbids a recovery binding
        // together with quiesced=true, so a recovery attestation can never be
        // mistaken for authorization of unrelated generic maintenance pages,
        // acknowledgements, or a second reservation.
        match recovery {
            Some(reservation) => {
                let bound_id: Option<Uuid> = sqlx::query_scalar(
                    "SELECT recovery_reservation_id FROM work_item_lifecycle_maintenance WHERE singleton",
                )
                .fetch_one(&mut *tx)
                .await?;
                let bound_token: Option<Uuid> = sqlx::query_scalar(
                    "SELECT recovery_reservation_token FROM work_item_lifecycle_maintenance WHERE singleton",
                )
                .fetch_one(&mut *tx)
                .await?;
                if bound_id.is_some()
                    && (bound_id != Some(reservation.id) || bound_token != Some(reservation.token))
                {
                    return Err(invalid());
                }
                sqlx::query("UPDATE work_item_lifecycle_maintenance SET quiesced=false,recovery_reservation_id=$1,recovery_reservation_token=$2 WHERE singleton")
                    .bind(reservation.id)
                    .bind(reservation.token)
                    .execute(&mut *tx)
                    .await?;
            }
            None => {
                sqlx::query("UPDATE work_item_lifecycle_maintenance SET quiesced=true,recovery_reservation_id=NULL,recovery_reservation_token=NULL WHERE singleton")
                    .execute(&mut *tx)
                    .await?;
            }
        }
        finish(&mut tx, expiry).await?;
        if let Some(expiry) = recovery_expiry {
            crate::admin_media_reservation::check_expiry(&mut tx, expiry).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Peek one work item after the durable cursor. Cursor advancement only
    /// follows acknowledgement or explicit quarantine, and wraps on completion.
    pub async fn next_lifecycle_provision_item(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
    ) -> Result<Option<Uuid>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = lock_fence(&mut tx, fence, true).await?;
        let cursor: Option<Uuid> = sqlx::query_scalar(
            "SELECT cursor_id FROM work_item_lifecycle_maintenance WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        let next:Option<Uuid>=sqlx::query_scalar("SELECT id FROM release_work_items WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 1").bind(cursor).fetch_optional(&mut *tx).await?;
        if next.is_none() {
            sqlx::query(
                "UPDATE work_item_lifecycle_maintenance SET cursor_id=NULL,pass_complete=true WHERE singleton",
            )
            .execute(&mut *tx)
            .await?;
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(next)
    }
    /// Reserve immutable native occurrence identity before creating a lock.
    pub async fn reserve_work_item_lifecycle(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        id: Uuid,
    ) -> Result<WorkItemLifecycleIdentity, WorkStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT singleton FROM work_item_lifecycle_budget WHERE singleton FOR UPDATE")
            .execute(&mut *tx)
            .await?;
        let expiry = lock_fence(&mut tx, fence, true).await?;
        ensure_next(&mut tx, id).await?;
        let prior: Option<WorkItemLifecycleIdentity> = sqlx::query_as(
            "SELECT * FROM work_item_lifecycle_locks WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = prior {
            let binding:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM release_work_items WHERE id=$1 AND source_library_id IS NOT DISTINCT FROM $2 AND source_path COLLATE \"C\"=$3 COLLATE \"C\" AND discovery_path_format=$4)")
                .bind(id).bind(row.source_library_id).bind(&row.source_path).bind(row.source_path_format).fetch_one(&mut *tx).await?;
            if !binding {
                return Err(invalid());
            }
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(row);
        }
        let item:Option<(Option<Uuid>,String,i16)>=sqlx::query_as("SELECT source_library_id,source_path,discovery_path_format FROM release_work_items WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let (library, path, format) = item.ok_or_else(invalid)?;
        if format == 0 && path.contains('\u{fffd}') {
            return Err(invalid());
        }
        let count=sqlx::query("UPDATE work_item_lifecycle_budget SET entries=entries+1 WHERE singleton AND entries<max_entries").execute(&mut *tx).await?;
        if count.rows_affected() != 1 {
            return Err(invalid());
        }
        let row:WorkItemLifecycleIdentity=sqlx::query_as("INSERT INTO work_item_lifecycle_locks(work_item_id,source_library_id,source_path,source_path_format,lock_name,state) VALUES($1,$2,$3,$4,$5,'reserved') RETURNING *")
            .bind(id).bind(library).bind(path).bind(format).bind(format!("work-{}.lock",id.simple())).fetch_one(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(row)
    }
    /// Bind exact verified filesystem identities once. Replays require a full
    /// equality match, and an ambiguous interrupted create stays reserved.
    pub async fn acknowledge_work_item_lifecycle(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        id: Uuid,
        lock_root: &str,
        lock_inode: &str,
        stage_root: &str,
    ) -> Result<WorkItemLifecycleIdentity, WorkStoreError> {
        if !valid(lock_root) || !valid(lock_inode) || !valid(stage_root) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = lock_fence(&mut tx, fence, true).await?;
        let mut row: WorkItemLifecycleIdentity = sqlx::query_as(
            "SELECT * FROM work_item_lifecycle_locks WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(invalid)?;
        let binding:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM release_work_items WHERE id=$1 AND source_library_id IS NOT DISTINCT FROM $2 AND source_path COLLATE \"C\"=$3 COLLATE \"C\" AND discovery_path_format=$4)")
            .bind(id).bind(row.source_library_id).bind(&row.source_path).bind(row.source_path_format).fetch_one(&mut *tx).await?;
        if !binding {
            return Err(invalid());
        }
        let cursor: Option<Uuid> = sqlx::query_scalar(
            "SELECT cursor_id FROM work_item_lifecycle_maintenance WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        let replay = row.state == "ready" && cursor.is_some_and(|previous| id <= previous);
        if !replay {
            ensure_next(&mut tx, id).await?;
        }
        if row.state == "reserved" {
            row=sqlx::query_as("UPDATE work_item_lifecycle_locks SET state='ready',lock_root_generation=$2,lock_generation=$3,stage_root_generation=$4 WHERE work_item_id=$1 RETURNING *").bind(id).bind(lock_root).bind(lock_inode).bind(stage_root).fetch_one(&mut *tx).await?;
        } else if row.lock_root_generation.as_deref() != Some(lock_root)
            || row.lock_generation.as_deref() != Some(lock_inode)
            || row.stage_root_generation.as_deref() != Some(stage_root)
        {
            return Err(invalid());
        }
        if !replay {
            sqlx::query("UPDATE work_item_lifecycle_maintenance SET cursor_id=$1,pass_complete=false WHERE singleton")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM work_item_lifecycle_quarantine WHERE work_item_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(row)
    }
    /// Durably defer an unresolved item while advancing one position. The
    /// next pass wraps and retries it; quarantine never authorizes filesystem use.
    pub async fn quarantine_lifecycle_provision_item(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        id: Uuid,
        reason: LifecycleQuarantineReason,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = lock_fence(&mut tx, fence, true).await?;
        ensure_next(&mut tx, id).await?;
        sqlx::query("INSERT INTO work_item_lifecycle_quarantine(work_item_id,reason) VALUES($1,$2) ON CONFLICT(work_item_id) DO UPDATE SET reason=EXCLUDED.reason,updated_at=clock_timestamp()")
            .bind(id).bind(reason.as_str()).execute(&mut *tx).await?;
        sqlx::query("UPDATE work_item_lifecycle_maintenance SET cursor_id=$1,pass_complete=false WHERE singleton")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Keep an existing maintenance fence alive without reviving an expired one.
    pub async fn renew_lifecycle_maintenance(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        duration: Duration,
    ) -> Result<(), WorkStoreError> {
        let ms = millis(duration)?;
        let mut tx = pool.begin().await?;
        let expiry = lock_fence(&mut tx, fence, false).await?;
        sqlx::query("UPDATE work_item_lifecycle_maintenance SET lease_expires_at=clock_timestamp()+($1::bigint*interval '1 millisecond') WHERE singleton").bind(ms).execute(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Release the metadata-provisioning fence only after a full externally
    /// quiesced pass with no pending or quarantined identities. This does not
    /// activate runtime enforcement; lifecycle participation is not complete.
    pub async fn finish_lifecycle_maintenance(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = lock_fence(&mut tx, fence, true).await?;
        let complete:bool=sqlx::query_scalar("SELECT pass_complete AND NOT EXISTS(SELECT 1 FROM work_item_lifecycle_locks WHERE state='reserved') AND NOT EXISTS(SELECT 1 FROM work_item_lifecycle_quarantine) AND NOT EXISTS(SELECT 1 FROM admin_media_reservations) FROM work_item_lifecycle_maintenance WHERE singleton").fetch_one(&mut *tx).await?;
        if !complete {
            return Err(invalid());
        }
        sqlx::query("UPDATE work_item_lifecycle_maintenance SET owner=NULL,token=NULL,lease_expires_at=NULL,quiesced=false,recovery_reservation_id=NULL,recovery_reservation_token=NULL,cursor_id=NULL,pass_complete=false WHERE singleton").execute(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Claim a bounded page of newly discovered items whose lifecycle locks
    /// have not been established. Filesystem creation remains private and
    /// exclusive; a pre-existing unacknowledged inode must be quarantined.
    pub async fn claim_lifecycle_provision_jobs(
        &self,
        pool: &PgPool,
        owner: &str,
        limit: usize,
        duration: Duration,
    ) -> Result<Vec<LifecycleProvisionLease>, WorkStoreError> {
        let ms = millis(duration)?;
        if owner.trim().is_empty() || owner.len() > 512 || limit == 0 || limit > 16 {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        admit_runtime(&mut tx).await?;
        let rows:Vec<LifecycleProvisionLease>=sqlx::query_as("WITH candidates AS (SELECT work_item_id FROM work_item_lifecycle_provision_jobs WHERE due_at<=clock_timestamp() AND (token IS NULL OR lease_expires_at<=clock_timestamp()) ORDER BY due_at,work_item_id LIMIT $2 FOR UPDATE SKIP LOCKED), claimed AS (UPDATE work_item_lifecycle_provision_jobs j SET owner=$1,token=gen_random_uuid(),lease_expires_at=clock_timestamp()+($3::bigint*interval '1 millisecond') FROM candidates c WHERE j.work_item_id=c.work_item_id RETURNING j.work_item_id,j.owner,j.token) SELECT c.work_item_id,c.owner,c.token,l.source_library_id,l.source_path,l.source_path_format,l.lock_name FROM claimed c JOIN work_item_lifecycle_locks l ON l.work_item_id=c.work_item_id WHERE l.state='reserved' ORDER BY c.work_item_id")
            .bind(owner).bind(i64::try_from(limit).map_err(|_|invalid())?).bind(ms).fetch_all(&mut *tx).await?;
        // Do not commit claims that expired while waiting for database work.
        for row in &rows {
            let _: OffsetDateTime = lock_provision(&mut tx, row).await?;
        }
        tx.commit().await?;
        Ok(rows)
    }
    /// Bind a private exclusive-create observation under the original job
    /// token. This is metadata only and is not a proof the caller holds a lock.
    pub async fn acknowledge_lifecycle_provision(
        &self,
        pool: &PgPool,
        lease: &LifecycleProvisionLease,
        lock_root: &str,
        lock_inode: &str,
        stage_root: &str,
    ) -> Result<(), WorkStoreError> {
        if !valid(lock_root) || !valid(lock_inode) || !valid(stage_root) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        admit_runtime(&mut tx).await?;
        // Lost-ack replay verifies every immutable identity; it grants no new
        // filesystem authority and cannot be used with a stale job token.
        let replay:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_item_lifecycle_locks l JOIN release_work_items w ON w.id=l.work_item_id WHERE l.work_item_id=$1 AND l.state='ready' AND l.completed_token=$2 AND l.completed_owner=$3 AND l.lock_root_generation=$4 AND l.lock_generation=$5 AND l.stage_root_generation=$6 AND l.source_library_id IS NOT DISTINCT FROM w.source_library_id AND l.source_path COLLATE \"C\"=w.source_path COLLATE \"C\" AND l.source_path_format=w.discovery_path_format)")
            .bind(lease.work_item_id).bind(lease.token).bind(&lease.owner).bind(lock_root).bind(lock_inode).bind(stage_root).fetch_one(&mut *tx).await?;
        if replay {
            tx.commit().await?;
            return Ok(());
        }
        let expiry = lock_provision(&mut tx, lease).await?;
        sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
            .bind(lease.work_item_id)
            .fetch_one(&mut *tx)
            .await?;
        let changed=sqlx::query("UPDATE work_item_lifecycle_locks l SET state='ready',lock_root_generation=$2,lock_generation=$3,stage_root_generation=$4,completed_token=$5,completed_owner=$6 FROM release_work_items w WHERE l.work_item_id=$1 AND w.id=l.work_item_id AND l.state='reserved' AND l.source_library_id IS NOT DISTINCT FROM w.source_library_id AND l.source_path COLLATE \"C\"=w.source_path COLLATE \"C\" AND l.source_path_format=w.discovery_path_format")
            .bind(lease.work_item_id).bind(lock_root).bind(lock_inode).bind(stage_root).bind(lease.token).bind(&lease.owner).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(invalid());
        }
        sqlx::query(
            "DELETE FROM work_item_lifecycle_provision_jobs WHERE work_item_id=$1 AND token=$2",
        )
        .bind(lease.work_item_id)
        .bind(lease.token)
        .execute(&mut *tx)
        .await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Keep an uncertain or failed creation durable and retry it after a
    /// positive bounded delay. Never infer a missing inode from this signal.
    pub async fn defer_lifecycle_provision(
        &self,
        pool: &PgPool,
        lease: &LifecycleProvisionLease,
        reason: LifecycleQuarantineReason,
        delay: Duration,
    ) -> Result<(), WorkStoreError> {
        let ms = millis(delay)?;
        let mut tx = pool.begin().await?;
        admit_runtime(&mut tx).await?;
        let expiry = lock_provision(&mut tx, lease).await?;
        sqlx::query("UPDATE work_item_lifecycle_provision_jobs SET owner=NULL,token=NULL,lease_expires_at=NULL,due_at=clock_timestamp()+($2::bigint*interval '1 millisecond'),retry_count=LEAST(retry_count::bigint+1,2147483647)::integer,last_reason=$3 WHERE work_item_id=$1").bind(lease.work_item_id).bind(ms).bind(reason.as_str()).execute(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Read exact ready identity under the original claim. Call again after
    /// acquiring the private lock and compare every field before touching media.
    pub async fn claimed_work_item_lifecycle(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
    ) -> Result<WorkItemLifecycleIdentity, WorkStoreError> {
        let mut tx = pool.begin().await?;
        admit_runtime(&mut tx).await?;
        let expiry:Option<OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1 AND lease_owner=$2 AND protected_attempt_lease_epoch=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE")
            .bind(claim.item_id).bind(&claim.owner).bind(claim.epoch).fetch_optional(&mut *tx).await?.flatten();
        let expiry = expiry.ok_or(WorkStoreError::LeaseExpired(claim.item_id))?;
        let row:WorkItemLifecycleIdentity=sqlx::query_as("SELECT l.* FROM work_item_lifecycle_locks l JOIN release_work_items w ON w.id=l.work_item_id WHERE l.work_item_id=$1 AND l.state='ready' AND NOT EXISTS(SELECT 1 FROM work_item_lifecycle_quarantine q WHERE q.work_item_id=l.work_item_id) AND l.source_library_id IS NOT DISTINCT FROM w.source_library_id AND l.source_path COLLATE \"C\"=w.source_path COLLATE \"C\" AND l.source_path_format=w.discovery_path_format FOR SHARE OF l")
            .bind(claim.item_id).fetch_optional(&mut *tx).await?.ok_or_else(invalid)?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(row)
    }
    /// Record an exact previously acknowledged identity that failed later
    /// filesystem revalidation. It remains immutable but cannot authorize
    /// claims or lock use while quarantined.
    pub async fn invalidate_work_item_lifecycle(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        expected: &WorkItemLifecycleIdentity,
        reason: LifecycleQuarantineReason,
    ) -> Result<(), WorkStoreError> {
        if expected.state != "ready" {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = lock_fence(&mut tx, fence, true).await?;
        let row: Option<WorkItemLifecycleIdentity> = sqlx::query_as(
            "SELECT * FROM work_item_lifecycle_locks WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(expected.work_item_id)
        .fetch_optional(&mut *tx)
        .await?;
        if row.as_ref() != Some(expected) {
            return Err(invalid());
        }
        sqlx::query("INSERT INTO work_item_lifecycle_quarantine(work_item_id,reason) VALUES($1,$2) ON CONFLICT(work_item_id) DO UPDATE SET reason=EXCLUDED.reason,updated_at=clock_timestamp()")
            .bind(expected.work_item_id).bind(reason.as_str()).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE work_item_lifecycle_maintenance SET pass_complete=false WHERE singleton",
        )
        .execute(&mut *tx)
        .await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
}
