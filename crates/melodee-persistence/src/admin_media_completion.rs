//! Bounded cleanup of metadata after a capability-backed catalog commit.
//! No API here creates a completion receipt or authorizes filesystem work.
use crate::{
    AdminMediaReservation, LifecycleMaintenance, WorkItemStore, WorkStoreError,
    admin_media_reservation, work_item_lifecycle,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// Provisional archive-root expectations, not a filesystem capability.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminArchiveRoot {
    pub path_format: String,
    pub path_hex: String,
    pub generation: String,
    pub mount_id: String,
    pub filesystem_magic: String,
}

/// Exact planned archive bytes. This token proves no file exists.
pub struct AdminArchiveIntent {
    reservation_id: Uuid,
    completion_id: Uuid,
    filename: String,
    lock_name: String,
    digest: String,
    bytes: Vec<u8>,
    root: AdminArchiveRoot,
}
impl AdminArchiveIntent {
    pub fn lock_name(&self) -> &str {
        &self.lock_name
    }
    pub fn filename(&self) -> &str {
        &self.filename
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn root(&self) -> &AdminArchiveRoot {
        &self.root
    }
}

fn append_archive_string(out: &mut Vec<u8>, value: &str) -> Result<(), WorkStoreError> {
    let length = u32::try_from(value.len()).map_err(|_| invalid())?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid completed administrative media cleanup")
}

impl WorkItemStore {
    /// Persist a deterministic, bounded archive intent. Metadata supplied for
    /// the root is provisional and must be verified by private filesystem
    /// custody. This method never creates, adopts, or approves an archive.
    pub async fn prepare_admin_completion_archive(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        root: &AdminArchiveRoot,
    ) -> Result<AdminArchiveIntent, WorkStoreError> {
        let hex = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if fence.owner != reservation.owner
            || !matches!(
                root.path_format.as_str(),
                "unix-hex-v1" | "windows-u16-hex-v1"
            )
            || root.path_hex.len() > 131072
            || root.path_hex.len() % 2 != 0
            || !hex(&root.path_hex)
            || root.generation.is_empty()
            || root.generation.len() > 256
            || root.mount_id.len() != 16
            || !hex(&root.mount_id)
            || root.filesystem_magic.len() != 16
            || !hex(&root.filesystem_magic)
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let maintenance_expiry = work_item_lifecycle::lock_fence(&mut tx, fence, true).await?;
        let expiry:Option<time::OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM admin_media_reservations WHERE id=$1 AND owner=$2 AND token=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE").bind(reservation.id).bind(&reservation.owner).bind(reservation.token).fetch_optional(&mut *tx).await?;
        let expiry = expiry.ok_or_else(invalid)?;
        let row:Option<(Uuid,Uuid,Uuid,String,String,String,time::OffsetDateTime)>=sqlx::query_as("SELECT completion_id,snapshot_revision,root_set_revision,kind,actor,reason,completed_at FROM admin_media_completion_receipts WHERE reservation_id=$1 AND cleanup_state='done' FOR UPDATE").bind(reservation.id).fetch_optional(&mut *tx).await?;
        let (completion, snapshot, root_revision, kind, actor, reason, completed) =
            row.ok_or_else(invalid)?;
        let micros =
            i64::try_from(completed.unix_timestamp_nanos() / 1000).map_err(|_| invalid())?;
        let mut bytes = b"melodee-admin-deletion-tombstone-v1\0".to_vec();
        for id in [reservation.id, completion, snapshot, root_revision] {
            bytes.extend_from_slice(id.as_bytes());
        }
        bytes.extend_from_slice(&micros.to_be_bytes());
        for value in [&kind, &actor, &reason] {
            append_archive_string(&mut bytes, value)?;
        }
        if bytes.len() > 8192 {
            return Err(invalid());
        }
        let digest = crate::migrate::checksum_hex(&Sha256::digest(&bytes));
        let filename = format!("admin-tombstone-{}.v1", completion.simple());
        let lock_name = format!("archive-{}.lock", completion.simple());
        let size = i32::try_from(bytes.len()).map_err(|_| invalid())?;
        let inserted=sqlx::query("INSERT INTO admin_media_archive_intents(reservation_id,completion_id,maintenance_token,reservation_token,filename,byte_hash,byte_size,root_path_format,root_path_hex,root_generation,root_mount_id,root_filesystem_magic,lock_name,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'intent') ON CONFLICT(reservation_id) DO NOTHING")
            .bind(reservation.id).bind(completion).bind(fence.token).bind(reservation.token).bind(&filename).bind(&digest).bind(size).bind(&root.path_format).bind(&root.path_hex).bind(&root.generation).bind(&root.mount_id).bind(&root.filesystem_magic).bind(&lock_name).execute(&mut *tx).await?;
        if inserted.rows_affected() == 0 {
            let exact:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_archive_intents WHERE reservation_id=$1 AND completion_id=$2 AND maintenance_token=$3 AND reservation_token=$4 AND filename=$5 AND byte_hash=$6 AND byte_size=$7 AND root_path_format=$8 AND root_path_hex COLLATE \"C\"=$9 COLLATE \"C\" AND root_generation=$10 AND root_mount_id=$11 AND root_filesystem_magic=$12 AND state='intent')")
                .bind(reservation.id).bind(completion).bind(fence.token).bind(reservation.token).bind(&filename).bind(&digest).bind(size).bind(&root.path_format).bind(&root.path_hex).bind(&root.generation).bind(&root.mount_id).bind(&root.filesystem_magic).fetch_one(&mut *tx).await?;
            if !exact {
                return Err(invalid());
            }
        }
        work_item_lifecycle::finish(&mut tx, maintenance_expiry).await?;
        admin_media_reservation::check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(AdminArchiveIntent {
            reservation_id: reservation.id,
            completion_id: completion,
            filename,
            lock_name,
            digest,
            bytes,
            root: root.clone(),
        })
    }
    /// Record a provisional lock inode identity, once, under the original
    /// epochs. This is not proof that the caller holds the filesystem lock.
    pub async fn acknowledge_admin_archive_lock(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        intent: &AdminArchiveIntent,
        lock_generation: &str,
    ) -> Result<(), WorkStoreError> {
        if fence.owner != reservation.owner
            || intent.reservation_id != reservation.id
            || lock_generation.is_empty()
            || lock_generation.len() > 256
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let maintenance_expiry = work_item_lifecycle::lock_fence(&mut tx, fence, true).await?;
        let expiry:Option<time::OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM admin_media_reservations WHERE id=$1 AND owner=$2 AND token=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE").bind(reservation.id).bind(&reservation.owner).bind(reservation.token).fetch_optional(&mut *tx).await?;
        let expiry = expiry.ok_or_else(invalid)?;
        let updated=sqlx::query("UPDATE admin_media_archive_intents SET lock_generation=$6 WHERE reservation_id=$1 AND completion_id=$2 AND maintenance_token=$3 AND reservation_token=$4 AND lock_name=$5 AND state='intent' AND (lock_generation IS NULL OR lock_generation=$6)")
            .bind(reservation.id).bind(intent.completion_id).bind(fence.token).bind(reservation.token).bind(&intent.lock_name).bind(lock_generation).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(invalid());
        }
        work_item_lifecycle::finish(&mut tx, maintenance_expiry).await?;
        admin_media_reservation::check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Revalidate the exact original archive intent and return its recorded
    /// lock identity. A caller must still acquire and verify the actual lock,
    /// pinned archive root and external maintenance fence before any effect.
    pub async fn revalidate_admin_completion_archive(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        intent: &AdminArchiveIntent,
    ) -> Result<Option<String>, WorkStoreError> {
        if fence.owner != reservation.owner || intent.reservation_id != reservation.id {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let maintenance_expiry = work_item_lifecycle::lock_fence(&mut tx, fence, true).await?;
        let expiry: Option<time::OffsetDateTime> = sqlx::query_scalar("SELECT lease_expires_at FROM admin_media_reservations WHERE id=$1 AND owner=$2 AND token=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE")
            .bind(reservation.id).bind(&reservation.owner).bind(reservation.token).fetch_optional(&mut *tx).await?;
        let expiry = expiry.ok_or_else(invalid)?;
        let found: Option<Option<String>> = sqlx::query_scalar("SELECT i.lock_generation FROM admin_media_archive_intents i JOIN admin_media_completion_receipts r ON r.reservation_id=i.reservation_id AND r.completion_id=i.completion_id WHERE i.reservation_id=$1 AND i.completion_id=$2 AND i.maintenance_token=$3 AND i.reservation_token=$4 AND i.filename=$5 AND i.lock_name=$6 AND i.byte_hash=$7 AND i.byte_size=$8 AND i.root_path_format=$9 AND i.root_path_hex COLLATE \"C\"=$10 COLLATE \"C\" AND i.root_generation=$11 AND i.root_mount_id=$12 AND i.root_filesystem_magic=$13 AND i.state='intent' AND r.cleanup_state='done' FOR SHARE OF i,r")
            .bind(reservation.id).bind(intent.completion_id).bind(fence.token).bind(reservation.token).bind(&intent.filename).bind(&intent.lock_name).bind(&intent.digest).bind(i32::try_from(intent.bytes.len()).map_err(|_|invalid())?).bind(&intent.root.path_format).bind(&intent.root.path_hex).bind(&intent.root.generation).bind(&intent.root.mount_id).bind(&intent.root.filesystem_magic).fetch_optional(&mut *tx).await?;
        let lock_generation = found.ok_or_else(invalid)?;
        work_item_lifecycle::finish(&mut tx, maintenance_expiry).await?;
        admin_media_reservation::check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(lock_generation)
    }
    /// Remove at most sixteen proof rows per turn after atomic catalog
    /// finalization. The compact immutable completion tombstone and its
    /// reservation quota remain durable. This method performs no filesystem
    /// work and cannot create a receipt.
    pub async fn cleanup_completed_admin_media_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<bool, WorkStoreError> {
        if fence.owner != reservation.owner {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT singleton FROM admin_media_snapshot_budget WHERE singleton FOR UPDATE")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "SELECT singleton FROM admin_media_page_intent_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "SELECT singleton FROM admin_media_quarantine_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        let maintenance_expiry = work_item_lifecycle::lock_fence(&mut tx, fence, true).await?;
        let reservation_expiry: Option<time::OffsetDateTime> = sqlx::query_scalar("SELECT lease_expires_at FROM admin_media_reservations WHERE id=$1 AND owner=$2 AND token=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE")
            .bind(reservation.id).bind(&reservation.owner).bind(reservation.token).fetch_optional(&mut *tx).await?;
        let reservation_expiry = reservation_expiry.ok_or_else(invalid)?;
        let state: Option<String> = sqlx::query_scalar("SELECT cleanup_state FROM admin_media_completion_receipts WHERE reservation_id=$1 FOR UPDATE")
            .bind(reservation.id).fetch_optional(&mut *tx).await?;
        let state = state.ok_or_else(invalid)?;
        let guarded: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_reservation_items WHERE reservation_id=$1) OR EXISTS(SELECT 1 FROM admin_media_reservation_releases WHERE reservation_id=$1)")
            .bind(reservation.id).fetch_one(&mut *tx).await?;
        if guarded {
            return Err(invalid());
        }
        if state == "done" {
            work_item_lifecycle::finish(&mut tx, maintenance_expiry).await?;
            admin_media_reservation::check_expiry(&mut tx, reservation_expiry).await?;
            tx.commit().await?;
            return Ok(true);
        }
        if state == "pending" {
            let validated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_completion_receipts r JOIN admin_media_snapshots s ON s.reservation_id=r.reservation_id AND s.revision=r.snapshot_revision AND s.root_set_revision=r.root_set_revision AND s.state='frozen' JOIN admin_media_coverage_audits a ON a.reservation_id=s.reservation_id AND a.snapshot_revision=s.revision AND a.phase='complete' WHERE r.reservation_id=$1)")
                .bind(reservation.id).fetch_one(&mut *tx).await?;
            if !validated {
                return Err(invalid());
            }
            sqlx::query("UPDATE admin_media_completion_receipts SET cleanup_state='cleaning' WHERE reservation_id=$1 AND cleanup_state='pending'")
                .bind(reservation.id).execute(&mut *tx).await?;
        } else if state != "cleaning" {
            return Err(invalid());
        }

        let intent: Option<(Uuid, String, i32, i64)> = sqlx::query_as("SELECT id,state,entry_count,logical_bytes FROM admin_media_page_intents WHERE reservation_id=$1 ORDER BY id LIMIT 1 FOR UPDATE")
            .bind(reservation.id).fetch_optional(&mut *tx).await?;
        if let Some((id, state, count, bytes)) = intent {
            if state != "verified" {
                return Err(invalid());
            }
            let uncertain: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_quarantine_intents WHERE intent_id=$1 AND state<>'verified')")
                .bind(id).fetch_one(&mut *tx).await?;
            if uncertain {
                return Err(invalid());
            }
            let quarantines: Vec<i32> = sqlx::query_scalar("DELETE FROM admin_media_quarantine_intents WHERE id IN (SELECT id FROM admin_media_quarantine_intents WHERE intent_id=$1 ORDER BY ordinal LIMIT 16) RETURNING logical_bytes")
                .bind(id).fetch_all(&mut *tx).await?;
            if !quarantines.is_empty() {
                let bytes: i64 = quarantines.iter().map(|n| i64::from(*n)).sum();
                sqlx::query("UPDATE admin_media_quarantine_budget SET entries=entries-$1,bytes=bytes-$2 WHERE singleton")
                    .bind(i64::try_from(quarantines.len()).map_err(|_|invalid())?).bind(bytes).execute(&mut *tx).await?;
                work_item_lifecycle::finish(&mut tx, maintenance_expiry).await?;
                admin_media_reservation::check_expiry(&mut tx, reservation_expiry).await?;
                tx.commit().await?;
                return Ok(false);
            }
            let removed =
                sqlx::query("DELETE FROM admin_media_page_intent_entries WHERE intent_id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            if i64::try_from(removed.rows_affected()).ok() != Some(i64::from(count)) {
                return Err(invalid());
            }
            sqlx::query("DELETE FROM admin_media_page_intents WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE admin_media_page_intent_budget SET intents=intents-1,entries=entries-$1,bytes=bytes-$2 WHERE singleton")
                .bind(i64::from(count)).bind(bytes).execute(&mut *tx).await?;
        } else {
            let mut paths: Vec<String> = sqlx::query_scalar("DELETE FROM admin_media_snapshot_entries WHERE (reservation_id,asset_id) IN (SELECT reservation_id,asset_id FROM admin_media_snapshot_entries WHERE reservation_id=$1 ORDER BY asset_id LIMIT 16) RETURNING relative_path")
                .bind(reservation.id).fetch_all(&mut *tx).await?;
            if paths.is_empty() {
                paths = sqlx::query_scalar("DELETE FROM admin_media_snapshot_objects WHERE (reservation_id,kind,object_id) IN (SELECT reservation_id,kind,object_id FROM admin_media_snapshot_objects WHERE reservation_id=$1 ORDER BY kind COLLATE \"C\",object_id LIMIT 16) RETURNING object_key")
                    .bind(reservation.id).fetch_all(&mut *tx).await?;
            }
            if !paths.is_empty() {
                let bytes = paths
                    .iter()
                    .try_fold(0_i64, |n, p| {
                        i64::try_from(p.len() + 256)
                            .ok()
                            .and_then(|m| n.checked_add(m))
                    })
                    .ok_or_else(invalid)?;
                sqlx::query("UPDATE admin_media_snapshot_budget SET entries=entries-$1,bytes=bytes-$2 WHERE singleton")
                    .bind(i64::try_from(paths.len()).map_err(|_| invalid())?).bind(bytes).execute(&mut *tx).await?;
            } else {
                sqlx::query("DELETE FROM admin_media_coverage_audits WHERE reservation_id=$1")
                    .bind(reservation.id)
                    .execute(&mut *tx)
                    .await?;
                let deleted =
                    sqlx::query("DELETE FROM admin_media_snapshots WHERE reservation_id=$1")
                        .bind(reservation.id)
                        .execute(&mut *tx)
                        .await?;
                if deleted.rows_affected() == 1 {
                    sqlx::query("UPDATE admin_media_snapshot_budget SET snapshots=snapshots-1 WHERE singleton").execute(&mut *tx).await?;
                } else {
                    let roots: Option<Uuid> = sqlx::query_scalar("SELECT revision FROM admin_media_root_sets WHERE reservation_id=$1 FOR UPDATE").bind(reservation.id).fetch_optional(&mut *tx).await?;
                    if let Some(revision) = roots {
                        let removed = sqlx::query("DELETE FROM admin_media_root_entries WHERE (revision,ordinal) IN (SELECT revision,ordinal FROM admin_media_root_entries WHERE revision=$1 ORDER BY ordinal LIMIT 16)").bind(revision).execute(&mut *tx).await?;
                        if removed.rows_affected() == 0 {
                            sqlx::query("DELETE FROM admin_media_root_sets WHERE revision=$1")
                                .bind(revision)
                                .execute(&mut *tx)
                                .await?;
                        }
                    } else {
                        let removed = sqlx::query("DELETE FROM admin_media_delete_targets WHERE (reservation_id,internal_id) IN (SELECT reservation_id,internal_id FROM admin_media_delete_targets WHERE reservation_id=$1 ORDER BY internal_id LIMIT 16)").bind(reservation.id).execute(&mut *tx).await?;
                        if removed.rows_affected() == 0 {
                            sqlx::query(
                                "DELETE FROM admin_media_delete_scopes WHERE reservation_id=$1",
                            )
                            .bind(reservation.id)
                            .execute(&mut *tx)
                            .await?;
                            sqlx::query("UPDATE admin_media_completion_receipts SET cleanup_state='done' WHERE reservation_id=$1 AND cleanup_state='cleaning'").bind(reservation.id).execute(&mut *tx).await?;
                        }
                    }
                }
            }
        }
        work_item_lifecycle::finish(&mut tx, maintenance_expiry).await?;
        admin_media_reservation::check_expiry(&mut tx, reservation_expiry).await?;
        tx.commit().await?;
        Ok(false)
    }
}
