//! Durable, provisional page intent. This metadata is never filesystem proof
//! and no method here authorizes or completes a destructive operation.
use crate::{
    AdminMediaReservation, LifecycleMaintenance, WorkItemStore, WorkStoreError,
    admin_media_reservation, work_item_lifecycle,
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// Provisional configured-root metadata. These caller-provided values are not
/// a proof of an open handle, topology, writer exclusion, or safe deletion.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminMediaRootExpectation {
    pub role: String,
    pub source_library_id: Option<Uuid>,
    pub path_format: String,
    pub path_hex: String,
    pub generation: String,
    pub mount_id: String,
    pub filesystem_magic: String,
}
/// Provisional metadata reported by a filesystem observer. No field proves
/// that the observer still holds a file, parent, root or lifecycle lock.
#[derive(Clone, Copy)]
pub struct AdminQuarantineObservation<'a> {
    pub source_generation: &'a str,
    pub source_parent_generation: &'a str,
    /// Exact reversible native relative path under the page's configured root.
    pub quarantine_parent_relative_hex: &'a str,
    pub quarantine_parent_generation: &'a str,
    pub mount_id: &'a str,
}
/// Bounded page member and its provisional filesystem observations.
pub struct AdminQuarantineRequest<'a> {
    pub ordinal: u16,
    pub id: Uuid,
    pub observation: AdminQuarantineObservation<'a>,
}
/// A planned quarantine move, not proof that the move or verification happened.
pub struct AdminQuarantineIntent {
    id: Uuid,
    leaf_name: String,
}
/// Durable metadata for recovery; the state is not filesystem proof.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminQuarantineBinding {
    pub id: Uuid,
    pub ordinal: i16,
    pub root_set_revision: Uuid,
    pub source_generation: String,
    pub source_parent_generation: String,
    pub quarantine_root_role: String,
    pub quarantine_parent_relative_hex: String,
    pub quarantine_parent_generation: String,
    pub mount_id: String,
    pub leaf_name: String,
    pub state: String,
}
/// Read-only historical observations for a newly fenced recovery worker.
/// No member proves an inode is still the same object after restart.
pub struct AdminQuarantineRecovery {
    pub page_id: Uuid,
    pub binding: AdminQuarantineBinding,
    pub entry: AdminDeletionIntentEntry,
    pub roots: Vec<AdminMediaRootExpectation>,
}
impl AdminQuarantineIntent {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn leaf_name(&self) -> &str {
        &self.leaf_name
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminDeletionPageKind {
    Media,
    Object,
}
impl AdminDeletionPageKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Media => "media",
            Self::Object => "object",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdminDeletionIntentEntry {
    pub kind: String,
    pub object_id: Uuid,
    pub object_key: String,
    pub byte_hash: Option<String>,
    pub byte_size: Option<i64>,
}
/// Opaque handle for exact immutable planned work. It is not a deletion proof.
#[derive(Debug, Clone)]
pub struct AdminDeletionPageIntent {
    id: Uuid,
    reservation_id: Uuid,
    kind: AdminDeletionPageKind,
    snapshot_revision: Uuid,
    root_set_revision: Option<Uuid>,
    entries: Vec<AdminDeletionIntentEntry>,
}
impl AdminDeletionPageIntent {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn reservation_id(&self) -> Uuid {
        self.reservation_id
    }
    pub fn kind(&self) -> AdminDeletionPageKind {
        self.kind
    }
    pub fn snapshot_revision(&self) -> Uuid {
        self.snapshot_revision
    }
    /// Provisional configured-root binding, if one was recorded. Absence and
    /// presence alike grant no filesystem authority.
    pub fn root_set_revision(&self) -> Option<Uuid> {
        self.root_set_revision
    }
    pub fn entries(&self) -> &[AdminDeletionIntentEntry] {
        &self.entries
    }
}
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("administrative deletion page intent changed")
}
async fn fences(
    tx: &mut Transaction<'_, Postgres>,
    fence: &LifecycleMaintenance,
    reservation: &AdminMediaReservation,
) -> Result<(time::OffsetDateTime, time::OffsetDateTime), WorkStoreError> {
    if fence.owner != reservation.owner {
        return Err(invalid());
    }
    Ok((
        work_item_lifecycle::lock_fence(tx, fence, true).await?,
        admin_media_reservation::verify_lease(tx, reservation).await?,
    ))
}
/// Read-only recovery fencing: the same owner plus either generic quiescence
/// or an exact recovery-reservation binding. Deliberately does not grant the
/// mutation paths any authority; they keep using [`fences`].
async fn fences_readonly_recovery(
    tx: &mut Transaction<'_, Postgres>,
    fence: &LifecycleMaintenance,
    reservation: &AdminMediaReservation,
) -> Result<(time::OffsetDateTime, time::OffsetDateTime), WorkStoreError> {
    if fence.owner != reservation.owner {
        return Err(invalid());
    }
    Ok((
        work_item_lifecycle::lock_fence_recovery(tx, fence, reservation.id).await?,
        admin_media_reservation::verify_lease(tx, reservation).await?,
    ))
}
async fn finish(
    tx: &mut Transaction<'_, Postgres>,
    expiry: (time::OffsetDateTime, time::OffsetDateTime),
) -> Result<(), WorkStoreError> {
    work_item_lifecycle::finish(tx, expiry.0).await?;
    admin_media_reservation::check_expiry(tx, expiry.1).await?;
    Ok(())
}
async fn entries(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Vec<AdminDeletionIntentEntry>, WorkStoreError> {
    Ok(sqlx::query_as("SELECT kind,object_id,object_key,byte_hash,byte_size FROM admin_media_page_intent_entries WHERE intent_id=$1 ORDER BY ordinal LIMIT 16").bind(id).fetch_all(&mut **tx).await?)
}
impl WorkItemStore {
    /// Locate retained move metadata after a lease changes. This does not
    /// transfer the old epoch or authorize adoption, deletion, or acknowledgement.
    pub async fn inspect_admin_quarantine_recovery(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        id: Uuid,
    ) -> Result<AdminQuarantineRecovery, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fences_readonly_recovery(&mut tx, fence, reservation).await?;
        let page_id: Option<Uuid> = sqlx::query_scalar("SELECT p.id FROM admin_media_quarantine_intents q JOIN admin_media_page_intents p ON p.id=q.intent_id JOIN admin_media_snapshots s ON s.reservation_id=p.reservation_id AND s.revision=p.snapshot_revision AND s.root_set_revision=p.root_set_revision JOIN admin_media_root_sets r ON r.revision=p.root_set_revision AND r.reservation_id=p.reservation_id WHERE q.id=$1 AND p.reservation_id=$2 AND q.root_set_revision=p.root_set_revision AND q.maintenance_token=p.maintenance_token AND q.reservation_token=p.reservation_token AND r.maintenance_token=p.maintenance_token AND r.reservation_token=p.reservation_token AND p.state IN ('intent','uncertain') AND s.state='frozen' FOR SHARE OF q,p,s,r")
            .bind(id).bind(reservation.id).fetch_optional(&mut *tx).await?;
        let page_id = page_id.ok_or_else(invalid)?;
        crate::admin_media_snapshot::verify_bound_scope(&mut tx, reservation.id).await?;
        let binding:AdminQuarantineBinding=sqlx::query_as("SELECT id,ordinal,root_set_revision,source_generation,source_parent_generation,quarantine_root_role,quarantine_parent_relative_hex,quarantine_parent_generation,mount_id,leaf_name,state FROM admin_media_quarantine_intents WHERE id=$1 FOR SHARE")
            .bind(id).fetch_one(&mut *tx).await?;
        let entry:AdminDeletionIntentEntry=sqlx::query_as("SELECT kind,object_id,object_key,byte_hash,byte_size FROM admin_media_page_intent_entries WHERE intent_id=$1 AND ordinal=$2")
            .bind(page_id).bind(binding.ordinal).fetch_one(&mut *tx).await?;
        let count: i32 =
            sqlx::query_scalar("SELECT entry_count FROM admin_media_root_sets WHERE revision=$1")
                .bind(binding.root_set_revision)
                .fetch_one(&mut *tx)
                .await?;
        let roots:Vec<AdminMediaRootExpectation>=sqlx::query_as("SELECT role,source_library_id,path_format,path_hex,generation,mount_id,filesystem_magic FROM admin_media_root_entries WHERE revision=$1 ORDER BY ordinal LIMIT 133")
            .bind(binding.root_set_revision).fetch_all(&mut *tx).await?;
        if usize::try_from(count).ok() != Some(roots.len()) || roots.len() > 132 {
            return Err(invalid());
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(AdminQuarantineRecovery {
            page_id,
            binding,
            entry,
            roots,
        })
    }
    /// Recover the exact persisted move binding under both original DB epochs.
    /// The returned state is only metadata and never authorizes adoption,
    /// deletion, or an inference from either name being absent.
    pub async fn read_admin_quarantine_binding(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        page: &AdminDeletionPageIntent,
        id: Uuid,
    ) -> Result<AdminQuarantineBinding, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fences_readonly_recovery(&mut tx, fence, reservation).await?;
        let binding:Option<AdminQuarantineBinding>=sqlx::query_as("SELECT q.id,q.ordinal,q.root_set_revision,q.source_generation,q.source_parent_generation,q.quarantine_root_role,q.quarantine_parent_relative_hex,q.quarantine_parent_generation,q.mount_id,q.leaf_name,q.state FROM admin_media_quarantine_intents q JOIN admin_media_page_intents p ON p.id=q.intent_id JOIN admin_media_snapshots s ON s.reservation_id=p.reservation_id AND s.revision=p.snapshot_revision AND s.root_set_revision=p.root_set_revision JOIN admin_media_root_sets r ON r.revision=p.root_set_revision AND r.reservation_id=p.reservation_id WHERE q.id=$1 AND q.intent_id=$2 AND p.reservation_id=$3 AND q.root_set_revision=$4 AND q.maintenance_token=$5 AND q.reservation_token=$6 AND p.maintenance_token=$5 AND p.reservation_token=$6 AND r.maintenance_token=$5 AND r.reservation_token=$6 AND p.snapshot_revision=$7 AND p.state IN ('intent','uncertain') AND s.state='frozen' FOR SHARE OF q,p,s,r")
            .bind(id).bind(page.id).bind(reservation.id).bind(page.root_set_revision.ok_or_else(invalid)?).bind(fence.token).bind(reservation.token).bind(page.snapshot_revision).fetch_optional(&mut *tx).await?;
        let binding = binding.ok_or_else(invalid)?;
        if page.reservation_id != reservation.id || entries(&mut tx, page.id).await? != page.entries
        {
            return Err(invalid());
        }
        validate_page_bindings(&mut tx, reservation.id, &page.entries).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(binding)
    }
    /// Bind one exact immutable page member before any private quarantine
    /// move. The executor must independently verify every observation, hold
    /// its capabilities and retain ambiguous bytes on failure.
    pub async fn prepare_admin_quarantine_intent(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        intent: &AdminDeletionPageIntent,
        request: &AdminQuarantineRequest<'_>,
    ) -> Result<AdminQuarantineIntent, WorkStoreError> {
        let ordinal = request.ordinal;
        let id = request.id;
        let observation = &request.observation;
        let index = usize::from(ordinal).checked_sub(1).ok_or_else(invalid)?;
        let member = intent.entries.get(index).ok_or_else(invalid)?;
        let role = match member.kind.as_str() {
            "media" => "canonical",
            "artwork" => "asset",
            _ => return Err(invalid()),
        };
        let root_revision = intent.root_set_revision.ok_or_else(invalid)?;
        let values = [
            observation.source_generation,
            observation.source_parent_generation,
            observation.quarantine_parent_generation,
        ];
        if values.iter().any(|s| s.is_empty() || s.len() > 256)
            || observation.quarantine_parent_relative_hex.is_empty()
            || observation.quarantine_parent_relative_hex.len() > 262200
            || observation.quarantine_parent_relative_hex.len() % 2 != 0
            || !observation
                .quarantine_parent_relative_hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || observation.mount_id.len() != 16
            || !observation
                .mount_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid());
        }
        let size = values
            .iter()
            .try_fold(256_usize, |n, v| n.checked_add(v.len()))
            .and_then(|n| n.checked_add(observation.quarantine_parent_relative_hex.len()))
            .ok_or_else(invalid)?;
        if size > 263224 {
            return Err(invalid());
        }
        let leaf_name = format!("quarantine-{}.entry", id.simple());
        let mut tx = pool.begin().await?;
        sqlx::query(
            "SELECT singleton FROM admin_media_quarantine_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        if intent.reservation_id != reservation.id {
            return Err(invalid());
        }
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_page_intents i JOIN admin_media_snapshots s ON s.reservation_id=i.reservation_id AND s.revision=i.snapshot_revision AND s.root_set_revision=i.root_set_revision JOIN admin_media_root_sets r ON r.revision=i.root_set_revision AND r.reservation_id=i.reservation_id WHERE i.id=$1 AND i.reservation_id=$2 AND i.maintenance_token=$3 AND i.reservation_token=$4 AND i.snapshot_revision=$5 AND i.root_set_revision=$6 AND i.state='intent' AND s.state='frozen' AND r.maintenance_token=$3 AND r.reservation_token=$4 AND EXISTS(SELECT 1 FROM admin_media_root_entries e WHERE e.revision=r.revision AND e.role=$7 AND e.mount_id=$8) FOR SHARE OF i,s,r)")
            .bind(intent.id).bind(reservation.id).bind(fence.token).bind(reservation.token).bind(intent.snapshot_revision).bind(root_revision).bind(role).bind(observation.mount_id).fetch_one(&mut *tx).await?;
        if !valid || entries(&mut tx, intent.id).await? != intent.entries {
            return Err(invalid());
        }
        validate_page_bindings(&mut tx, reservation.id, &intent.entries).await?;
        let prior:Option<bool>=sqlx::query_scalar("SELECT id=$3 AND root_set_revision=$4 AND maintenance_token=$5 AND reservation_token=$6 AND source_generation=$7 AND source_parent_generation=$8 AND quarantine_parent_generation=$9 AND mount_id=$10 AND leaf_name=$11 AND logical_bytes=$12 AND quarantine_root_role=$13 AND quarantine_parent_relative_hex COLLATE \"C\"=$14 COLLATE \"C\" AND state='intent' FROM admin_media_quarantine_intents WHERE intent_id=$1 AND ordinal=$2 FOR UPDATE")
            .bind(intent.id).bind(i16::try_from(ordinal).map_err(|_|invalid())?).bind(id).bind(root_revision).bind(fence.token).bind(reservation.token).bind(observation.source_generation).bind(observation.source_parent_generation).bind(observation.quarantine_parent_generation).bind(observation.mount_id).bind(&leaf_name).bind(i32::try_from(size).map_err(|_|invalid())?).bind(role).bind(observation.quarantine_parent_relative_hex).fetch_optional(&mut *tx).await?;
        if let Some(exact) = prior {
            if !exact {
                return Err(invalid());
            }
        } else {
            let charged=sqlx::query("UPDATE admin_media_quarantine_budget SET entries=entries+1,bytes=bytes+$1 WHERE singleton AND entries<max_entries AND $1<=max_bytes-bytes").bind(i64::try_from(size).map_err(|_|invalid())?).execute(&mut *tx).await?;
            if charged.rows_affected() != 1 {
                return Err(invalid());
            }
            sqlx::query("INSERT INTO admin_media_quarantine_intents(id,intent_id,ordinal,root_set_revision,maintenance_token,reservation_token,source_generation,source_parent_generation,quarantine_parent_generation,mount_id,leaf_name,logical_bytes,quarantine_root_role,quarantine_parent_relative_hex,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'intent')")
                .bind(id).bind(intent.id).bind(i16::try_from(ordinal).map_err(|_|invalid())?).bind(root_revision).bind(fence.token).bind(reservation.token).bind(observation.source_generation).bind(observation.source_parent_generation).bind(observation.quarantine_parent_generation).bind(observation.mount_id).bind(&leaf_name).bind(i32::try_from(size).map_err(|_|invalid())?).bind(role).bind(observation.quarantine_parent_relative_hex).execute(&mut *tx).await?;
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(AdminQuarantineIntent { id, leaf_name })
    }
    /// Bind bounded provisional root expectations before capturing any page.
    /// Replay requires byte-for-byte identical entries and original DB epochs.
    /// The private executor must independently verify them; this is not proof.
    pub async fn bind_maintenance_media_roots(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        revision: Uuid,
        roots: &[AdminMediaRootExpectation],
    ) -> Result<(), WorkStoreError> {
        use std::collections::BTreeSet;
        if !(5..=132).contains(&roots.len()) {
            return Err(invalid());
        }
        let hex = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let mut keys = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut bytes = 0_usize;
        for r in roots {
            if !matches!(
                r.role.as_str(),
                "source" | "work" | "stage" | "canonical" | "asset" | "lock"
            ) || (r.role != "source" && r.source_library_id.is_some())
                || !keys.insert((r.role.as_str(), r.source_library_id))
                || !matches!(r.path_format.as_str(), "unix-hex-v1" | "windows-u16-hex-v1")
                || r.path_hex.len() % 2 != 0
                || !hex(&r.path_hex)
                || r.generation.is_empty()
                || r.generation.len() > 256
                || r.mount_id.len() != 16
                || !hex(&r.mount_id)
                || r.filesystem_magic.len() != 16
                || !hex(&r.filesystem_magic)
            {
                return Err(invalid());
            }
            bytes = bytes
                .checked_add(r.path_hex.len() + r.generation.len() + 128)
                .ok_or_else(invalid)?;
            if r.role == "source" {
                sources.insert(r.source_library_id);
            }
        }
        if bytes > 131072
            || ["work", "stage", "canonical", "asset", "lock"]
                .iter()
                .any(|role| !keys.contains(&(*role, None)))
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let required: Vec<Option<Uuid>> = sqlx::query_scalar("SELECT DISTINCT source_library_id FROM admin_media_reservation_items WHERE reservation_id=$1")
            .bind(reservation.id).fetch_all(&mut *tx).await?;
        if !required
            .into_iter()
            .collect::<BTreeSet<_>>()
            .is_subset(&sources)
        {
            return Err(invalid());
        }
        let snapshot: Option<(String, i64, Option<Uuid>)> = sqlx::query_as("SELECT state,entries,root_set_revision FROM admin_media_snapshots WHERE reservation_id=$1 FOR UPDATE")
            .bind(reservation.id).fetch_optional(&mut *tx).await?;
        let (state, entries, bound) = snapshot.ok_or_else(invalid)?;
        if let Some(bound) = bound {
            if bound != revision {
                return Err(invalid());
            }
            let existing: Vec<AdminMediaRootExpectation> = sqlx::query_as("SELECT role,source_library_id,path_format,path_hex,generation,mount_id,filesystem_magic FROM admin_media_root_entries WHERE revision=$1 ORDER BY ordinal LIMIT 133")
                .bind(revision).fetch_all(&mut *tx).await?;
            let epochs: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_root_sets WHERE revision=$1 AND reservation_id=$2 AND maintenance_token=$3 AND reservation_token=$4 AND entry_count=$5 AND logical_bytes=$6)")
                .bind(revision).bind(reservation.id).bind(fence.token).bind(reservation.token).bind(i32::try_from(roots.len()).map_err(|_| invalid())?).bind(i64::try_from(bytes).map_err(|_| invalid())?).fetch_one(&mut *tx).await?;
            if !epochs || existing != roots {
                return Err(invalid());
            }
        } else {
            if state != "capturing" || entries != 0 {
                return Err(invalid());
            }
            sqlx::query("INSERT INTO admin_media_root_sets(revision,reservation_id,maintenance_token,reservation_token,entry_count,logical_bytes) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(revision).bind(reservation.id).bind(fence.token).bind(reservation.token).bind(i32::try_from(roots.len()).map_err(|_| invalid())?).bind(i64::try_from(bytes).map_err(|_| invalid())?).execute(&mut *tx).await?;
            for (index, root) in roots.iter().enumerate() {
                sqlx::query("INSERT INTO admin_media_root_entries(revision,ordinal,role,source_library_id,path_format,path_hex,generation,mount_id,filesystem_magic) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                    .bind(revision).bind(i16::try_from(index + 1).map_err(|_| invalid())?).bind(&root.role).bind(root.source_library_id).bind(&root.path_format).bind(&root.path_hex).bind(&root.generation).bind(&root.mount_id).bind(&root.filesystem_magic).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE admin_media_snapshots SET root_set_revision=$2 WHERE reservation_id=$1 AND root_set_revision IS NULL")
                .bind(reservation.id).bind(revision).execute(&mut *tx).await?;
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Read at most 132 provisional roots bound to this exact live intent.
    /// The caller must compare the complete configured root set and reopen
    /// each root under held filesystem capabilities; these rows are not proof.
    pub async fn read_maintenance_intent_roots(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        intent: &AdminDeletionPageIntent,
    ) -> Result<Vec<AdminMediaRootExpectation>, WorkStoreError> {
        let revision = intent.root_set_revision.ok_or_else(invalid)?;
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let count: Option<i32> = sqlx::query_scalar("SELECT r.entry_count FROM admin_media_root_sets r JOIN admin_media_snapshots s ON s.reservation_id=r.reservation_id AND s.root_set_revision=r.revision JOIN admin_media_page_intents i ON i.reservation_id=r.reservation_id AND i.snapshot_revision=s.revision AND i.root_set_revision=r.revision WHERE r.revision=$1 AND r.reservation_id=$2 AND r.maintenance_token=$3 AND r.reservation_token=$4 AND s.state='frozen' AND i.id=$5 AND i.state='intent' AND i.maintenance_token=$3 AND i.reservation_token=$4 AND i.snapshot_revision=$6 FOR SHARE OF r,s,i")
            .bind(revision).bind(reservation.id).bind(fence.token).bind(reservation.token).bind(intent.id).bind(intent.snapshot_revision).fetch_optional(&mut *tx).await?;
        let roots: Vec<AdminMediaRootExpectation> = sqlx::query_as("SELECT role,source_library_id,path_format,path_hex,generation,mount_id,filesystem_magic FROM admin_media_root_entries WHERE revision=$1 ORDER BY ordinal LIMIT 133")
            .bind(revision).fetch_all(&mut *tx).await?;
        if intent.reservation_id != reservation.id
            || count.and_then(|c| usize::try_from(c).ok()) != Some(roots.len())
            || roots.len() > 132
        {
            return Err(invalid());
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(roots)
    }
    /// Audit at most sixteen frozen closure entries against exact verified
    /// intents. Completion is bookkeeping, never filesystem authority. In
    /// particular, the provisional public acknowledgement APIs cannot make
    /// an entry pass without its corresponding trusted verified intent.
    pub async fn audit_maintenance_deletion_coverage_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let revision: Option<Uuid> = sqlx::query_scalar("SELECT revision FROM admin_media_snapshots WHERE reservation_id=$1 AND state='frozen' FOR UPDATE")
            .bind(reservation.id).fetch_optional(&mut *tx).await?;
        let revision = revision.ok_or_else(invalid)?;
        crate::admin_media_snapshot::verify_bound_scope(&mut tx, reservation.id).await?;
        sqlx::query("INSERT INTO admin_media_coverage_audits(reservation_id,snapshot_revision,phase) VALUES($1,$2,'media') ON CONFLICT(reservation_id) DO NOTHING")
            .bind(reservation.id).bind(revision).execute(&mut *tx).await?;
        let (saved_revision, phase, last_media, last_kind, last_object): (Uuid, String, Option<Uuid>, Option<String>, Option<Uuid>) =
            sqlx::query_as("SELECT snapshot_revision,phase,last_media,last_kind,last_object FROM admin_media_coverage_audits WHERE reservation_id=$1 FOR UPDATE")
                .bind(reservation.id).fetch_one(&mut *tx).await?;
        if saved_revision != revision {
            return Err(invalid());
        }
        match phase.as_str() {
            "media" => {
                let rows: Vec<(Uuid, bool)> = sqlx::query_as("SELECT s.asset_id, EXISTS(SELECT 1 FROM admin_media_page_intents i JOIN admin_media_page_intent_entries e ON e.intent_id=i.id WHERE i.id=s.acknowledgement AND i.reservation_id=s.reservation_id AND i.snapshot_revision=$3 AND i.kind='media' AND i.state='verified' AND i.entry_count=(SELECT count(*) FROM admin_media_page_intent_entries n WHERE n.intent_id=i.id) AND e.kind='media' AND e.object_id=s.asset_id AND e.object_key COLLATE \"C\"=s.relative_path COLLATE \"C\" AND e.byte_hash=s.byte_hash AND e.byte_size=s.byte_size AND EXISTS(SELECT 1 FROM admin_media_quarantine_intents q WHERE q.intent_id=i.id AND q.ordinal=e.ordinal AND q.root_set_revision=i.root_set_revision AND q.maintenance_token=i.maintenance_token AND q.reservation_token=i.reservation_token AND q.state='verified')) FROM admin_media_snapshot_entries s WHERE s.reservation_id=$1 AND ($2::uuid IS NULL OR s.asset_id>$2) ORDER BY s.asset_id LIMIT 16")
                    .bind(reservation.id).bind(last_media).bind(revision).fetch_all(&mut *tx).await?;
                if rows.iter().any(|(_, verified)| !verified) {
                    return Err(invalid());
                }
                if let Some((last, _)) = rows.last() {
                    sqlx::query("UPDATE admin_media_coverage_audits SET last_media=$2 WHERE reservation_id=$1").bind(reservation.id).bind(last).execute(&mut *tx).await?;
                } else {
                    sqlx::query("UPDATE admin_media_coverage_audits SET phase='objects' WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                }
            }
            "objects" => {
                let rows: Vec<(String, Uuid, bool)> = sqlx::query_as("SELECT s.kind,s.object_id, EXISTS(SELECT 1 FROM admin_media_page_intents i JOIN admin_media_page_intent_entries e ON e.intent_id=i.id WHERE i.id=s.acknowledgement AND i.reservation_id=s.reservation_id AND i.snapshot_revision=$4 AND i.kind='object' AND i.state='verified' AND i.entry_count=(SELECT count(*) FROM admin_media_page_intent_entries n WHERE n.intent_id=i.id) AND e.kind=s.kind AND e.object_id=s.object_id AND e.object_key COLLATE \"C\"=s.object_key COLLATE \"C\" AND e.byte_hash IS NOT DISTINCT FROM s.byte_hash AND e.byte_size IS NOT DISTINCT FROM s.byte_size AND s.kind='artwork' AND EXISTS(SELECT 1 FROM admin_media_quarantine_intents q WHERE q.intent_id=i.id AND q.ordinal=e.ordinal AND q.root_set_revision=i.root_set_revision AND q.maintenance_token=i.maintenance_token AND q.reservation_token=i.reservation_token AND q.state='verified')) FROM admin_media_snapshot_objects s WHERE s.reservation_id=$1 AND ($2::text IS NULL OR (s.kind COLLATE \"C\",s.object_id)>($2 COLLATE \"C\",$3)) ORDER BY s.kind COLLATE \"C\",s.object_id LIMIT 16")
                    .bind(reservation.id).bind(last_kind).bind(last_object).bind(revision).fetch_all(&mut *tx).await?;
                if rows.iter().any(|(_, _, verified)| !verified) {
                    return Err(invalid());
                }
                if let Some((kind, id, _)) = rows.last() {
                    sqlx::query("UPDATE admin_media_coverage_audits SET last_kind=$2,last_object=$3 WHERE reservation_id=$1").bind(reservation.id).bind(kind).bind(id).execute(&mut *tx).await?;
                } else {
                    sqlx::query("UPDATE admin_media_coverage_audits SET phase='complete' WHERE reservation_id=$1").bind(reservation.id).execute(&mut *tx).await?;
                }
            }
            "complete" => {}
            _ => return Err(invalid()),
        }
        let complete: bool = sqlx::query_scalar(
            "SELECT phase='complete' FROM admin_media_coverage_audits WHERE reservation_id=$1",
        )
        .bind(reservation.id)
        .fetch_one(&mut *tx)
        .await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(complete)
    }
    /// Persist one exact page before filesystem work. Existing intent replay is
    /// permitted only for the same reservation and both original live epochs.
    pub async fn prepare_maintenance_deletion_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        kind: AdminDeletionPageKind,
        id: Uuid,
    ) -> Result<Option<AdminDeletionPageIntent>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "SELECT singleton FROM admin_media_page_intent_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let snapshot:Option<(Uuid,Option<Uuid>)>=sqlx::query_as("SELECT revision,root_set_revision FROM admin_media_snapshots WHERE reservation_id=$1 AND state='frozen' FOR UPDATE").bind(reservation.id).fetch_optional(&mut *tx).await?;
        let (snapshot_revision, root_set_revision) = snapshot.ok_or_else(invalid)?;
        let prior:Option<(Uuid,String,Uuid,Uuid,Uuid,Option<Uuid>,String,i32)>=sqlx::query_as("SELECT reservation_id,kind,maintenance_token,reservation_token,snapshot_revision,root_set_revision,state,entry_count FROM admin_media_page_intents WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        if let Some((
            owner,
            prior_kind,
            maintenance_token,
            reservation_token,
            prior_revision,
            prior_root_set,
            state,
            count,
        )) = prior
        {
            if owner != reservation.id
                || prior_kind != kind.as_str()
                || maintenance_token != fence.token
                || reservation_token != reservation.token
                || prior_revision != snapshot_revision
                || prior_root_set != root_set_revision
                || state != "intent"
            {
                return Err(invalid());
            }
            let rows = entries(&mut tx, id).await?;
            if usize::try_from(count).ok() != Some(rows.len()) {
                return Err(invalid());
            }
            validate_page_bindings(&mut tx, reservation.id, &rows).await?;
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(Some(AdminDeletionPageIntent {
                id,
                reservation_id: reservation.id,
                kind,
                snapshot_revision,
                root_set_revision,
                entries: rows,
            }));
        }
        let open:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_page_intents WHERE reservation_id=$1 AND state IN ('intent','uncertain'))").bind(reservation.id).fetch_one(&mut *tx).await?;
        if open {
            return Err(invalid());
        }
        let rows:Vec<AdminDeletionIntentEntry>=match kind {
            AdminDeletionPageKind::Media=>sqlx::query_as("SELECT 'media'::text AS kind,asset_id AS object_id,relative_path AS object_key,byte_hash,byte_size FROM admin_media_snapshot_entries WHERE reservation_id=$1 AND acknowledgement IS NULL ORDER BY asset_id LIMIT 16 FOR UPDATE").bind(reservation.id).fetch_all(&mut *tx).await?,
            AdminDeletionPageKind::Object=>sqlx::query_as("SELECT kind,object_id,object_key,byte_hash,byte_size FROM admin_media_snapshot_objects WHERE reservation_id=$1 AND acknowledgement IS NULL ORDER BY kind COLLATE \"C\",object_id LIMIT 16 FOR UPDATE").bind(reservation.id).fetch_all(&mut *tx).await?,
        };
        if rows.is_empty() {
            finish(&mut tx, expiry).await?;
            tx.commit().await?;
            return Ok(None);
        }
        validate_page_bindings(&mut tx, reservation.id, &rows).await?;
        let bytes = rows
            .iter()
            .try_fold(256_i64, |sum, row| {
                i64::try_from(row.object_key.len() + 256)
                    .ok()
                    .and_then(|cost| sum.checked_add(cost))
            })
            .ok_or_else(invalid)?;
        let count = i64::try_from(rows.len()).map_err(|_| invalid())?;
        let charged=sqlx::query("UPDATE admin_media_page_intent_budget SET intents=intents+1,entries=entries+$1,bytes=bytes+$2 WHERE singleton AND intents<max_intents AND $1<=max_entries-entries AND $2<=max_bytes-bytes").bind(count).bind(bytes).execute(&mut *tx).await?;
        if charged.rows_affected() != 1 {
            return Err(invalid());
        }
        sqlx::query("INSERT INTO admin_media_page_intents(id,reservation_id,kind,maintenance_token,reservation_token,snapshot_revision,root_set_revision,state,entry_count,logical_bytes) VALUES($1,$2,$3,$4,$5,$6,$7,'intent',$8,$9)")
            .bind(id).bind(reservation.id).bind(kind.as_str()).bind(fence.token).bind(reservation.token).bind(snapshot_revision).bind(root_set_revision).bind(i32::try_from(count).map_err(|_|invalid())?).bind(bytes).execute(&mut *tx).await?;
        let ordinals: Vec<i16> = (1..=i16::try_from(rows.len()).map_err(|_| invalid())?).collect();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        let ids: Vec<Uuid> = rows.iter().map(|r| r.object_id).collect();
        let keys: Vec<&str> = rows.iter().map(|r| r.object_key.as_str()).collect();
        let hashes: Vec<Option<&str>> = rows.iter().map(|r| r.byte_hash.as_deref()).collect();
        let sizes: Vec<Option<i64>> = rows.iter().map(|r| r.byte_size).collect();
        sqlx::query("INSERT INTO admin_media_page_intent_entries(intent_id,ordinal,kind,object_id,object_key,byte_hash,byte_size) SELECT $1,x.ordinal,x.kind,x.object_id,x.object_key,x.byte_hash,x.byte_size FROM unnest($2::smallint[],$3::text[],$4::uuid[],$5::text[],$6::text[],$7::bigint[]) x(ordinal,kind,object_id,object_key,byte_hash,byte_size)")
            .bind(id).bind(&ordinals).bind(&kinds).bind(&ids).bind(&keys).bind(&hashes).bind(&sizes).execute(&mut *tx).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(Some(AdminDeletionPageIntent {
            id,
            reservation_id: reservation.id,
            kind,
            snapshot_revision,
            root_set_revision,
            entries: rows,
        }))
    }
    /// Recheck exact immutable intent and both DB epochs. A private executor
    /// must call this after acquiring its filesystem locks and before work; the
    /// return value still does not prove that those locks are held.
    pub async fn revalidate_maintenance_deletion_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        intent: &AdminDeletionPageIntent,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_page_intents WHERE id=$1 AND reservation_id=$2 AND kind=$3 AND maintenance_token=$4 AND reservation_token=$5 AND snapshot_revision=$7 AND root_set_revision IS NOT DISTINCT FROM $8 AND state='intent' AND entry_count=$6 AND EXISTS(SELECT 1 FROM admin_media_snapshots s WHERE s.reservation_id=$2 AND s.revision=$7 AND s.root_set_revision IS NOT DISTINCT FROM $8 AND s.state='frozen') FOR SHARE)")
            .bind(intent.id).bind(reservation.id).bind(intent.kind.as_str()).bind(fence.token).bind(reservation.token).bind(i32::try_from(intent.entries.len()).map_err(|_|invalid())?).bind(intent.snapshot_revision).bind(intent.root_set_revision).fetch_one(&mut *tx).await?;
        if !valid
            || intent.reservation_id != reservation.id
            || entries(&mut tx, intent.id).await? != intent.entries
        {
            return Err(invalid());
        }
        validate_page_bindings(&mut tx, reservation.id, &intent.entries).await?;
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Preserve a page whose filesystem outcome cannot be proven. No later
    /// epoch may silently adopt or clear it.
    pub async fn quarantine_maintenance_deletion_page(
        &self,
        pool: &PgPool,
        fence: &LifecycleMaintenance,
        reservation: &AdminMediaReservation,
        intent: &AdminDeletionPageIntent,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fences(&mut tx, fence, reservation).await?;
        let updated=sqlx::query("UPDATE admin_media_page_intents SET state='uncertain' WHERE id=$1 AND reservation_id=$2 AND kind=$3 AND maintenance_token=$4 AND reservation_token=$5 AND snapshot_revision=$6 AND state IN ('intent','uncertain')")
            .bind(intent.id).bind(reservation.id).bind(intent.kind.as_str()).bind(fence.token).bind(reservation.token).bind(intent.snapshot_revision).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 || entries(&mut tx, intent.id).await? != intent.entries {
            return Err(invalid());
        }
        finish(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
}

// The DB check detects changed catalog identities before filesystem work. The
// external maintenance fence and held filesystem locks remain mandatory after
// this transaction commits.
async fn validate_page_bindings(
    tx: &mut Transaction<'_, Postgres>,
    reservation: Uuid,
    rows: &[AdminDeletionIntentEntry],
) -> Result<(), WorkStoreError> {
    crate::admin_media_snapshot::verify_bound_scope(tx, reservation).await?;
    if rows.is_empty() || rows.len() > 16 {
        return Err(invalid());
    }
    let media: Vec<&AdminDeletionIntentEntry> =
        rows.iter().filter(|row| row.kind == "media").collect();
    if !media.is_empty() {
        let ids: Vec<Uuid> = media.iter().map(|row| row.object_id).collect();
        let paths: Vec<&str> = media.iter().map(|row| row.object_key.as_str()).collect();
        let hashes: Vec<Option<&str>> = media.iter().map(|row| row.byte_hash.as_deref()).collect();
        let sizes: Vec<Option<i64>> = media.iter().map(|row| row.byte_size).collect();
        let matches:i64=sqlx::query_scalar("SELECT count(*) FROM unnest($2::uuid[],$3::text[],$4::text[],$5::bigint[]) x(id,path,hash,size) JOIN media_assets a ON a.id=x.id JOIN admin_media_delete_scopes sc ON sc.reservation_id=$1 WHERE a.canonical_relative_path COLLATE \"C\"=x.path COLLATE \"C\" AND a.byte_hash=x.hash AND a.byte_size=x.size AND EXISTS(SELECT 1 FROM track_media_assets l JOIN tracks t ON t.id=l.track_id WHERE l.media_asset_id=a.id AND ((sc.kind='track' AND EXISTS(SELECT 1 FROM admin_media_delete_targets target WHERE target.reservation_id=$1 AND target.internal_id=t.id)) OR (sc.kind<>'track' AND EXISTS(SELECT 1 FROM admin_media_reservation_releases r WHERE r.reservation_id=$1 AND r.release_id=t.release_id)))) AND NOT EXISTS(SELECT 1 FROM track_media_assets l JOIN tracks t ON t.id=l.track_id WHERE l.media_asset_id=a.id AND ((sc.kind='track' AND NOT EXISTS(SELECT 1 FROM admin_media_delete_targets target WHERE target.reservation_id=$1 AND target.internal_id=t.id)) OR (sc.kind<>'track' AND NOT EXISTS(SELECT 1 FROM admin_media_reservation_releases r WHERE r.reservation_id=$1 AND r.release_id=t.release_id))))")
            .bind(reservation).bind(&ids).bind(&paths).bind(&hashes).bind(&sizes).fetch_one(&mut **tx).await?;
        if usize::try_from(matches).ok() != Some(media.len()) {
            return Err(invalid());
        }
    }
    let artwork: Vec<String> = rows
        .iter()
        .filter(|row| row.kind == "artwork")
        .map(|row| row.object_key.clone())
        .collect();
    let exclusive = if artwork.is_empty() {
        std::collections::HashSet::new()
    } else {
        crate::admin_media_snapshot::exclusive_artwork(tx, reservation, &artwork).await?
    };
    for row in rows.iter().filter(|row| row.kind != "media") {
        let valid:bool=match row.kind.as_str() {
            "canonical_directory"=>sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_reservation_releases r JOIN release_publication_plans p ON p.release_id=r.release_id WHERE r.reservation_id=$1 AND r.release_id=$2 AND p.target_relative_path COLLATE \"C\"=$3 COLLATE \"C\")")
                .bind(reservation).bind(row.object_id).bind(&row.object_key).fetch_one(&mut **tx).await?,
            "staging_directory"=>sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_reservation_releases r JOIN release_publication_plans p ON p.release_id=r.release_id JOIN release_work_item_staging_handoffs h ON h.work_item_id=p.work_item_id WHERE r.reservation_id=$1 AND r.release_id=$2 AND h.staging_relative_path COLLATE \"C\"=$3 COLLATE \"C\")")
                .bind(reservation).bind(row.object_id).bind(&row.object_key).fetch_one(&mut **tx).await?,
            "artwork"=>{
                if !exclusive.contains(&row.object_key) { false } else {
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM asset_files f WHERE f.asset_key=$3 AND split_part(f.asset_key,'-',1)=$4 AND f.size_bytes::bigint=$5 AND (EXISTS(SELECT 1 FROM catalog_entity_images i WHERE i.asset_key=f.asset_key AND ((i.release_id=$2 AND EXISTS(SELECT 1 FROM admin_media_reservation_releases r WHERE r.reservation_id=$1 AND r.release_id=i.release_id)) OR (i.artist_id=$2 AND EXISTS(SELECT 1 FROM admin_media_delete_targets t JOIN admin_media_delete_scopes s USING(reservation_id) WHERE t.reservation_id=$1 AND s.kind='artist' AND t.internal_id=i.artist_id)))) OR EXISTS(SELECT 1 FROM catalog_provider_images i WHERE i.id=$2 AND i.asset_key=f.asset_key AND ((i.entity_type='release' AND EXISTS(SELECT 1 FROM admin_media_reservation_releases r WHERE r.reservation_id=$1 AND r.release_id=i.entity_id)) OR (i.entity_type='artist' AND EXISTS(SELECT 1 FROM admin_media_delete_targets t JOIN admin_media_delete_scopes s USING(reservation_id) WHERE t.reservation_id=$1 AND s.kind='artist' AND t.internal_id=i.entity_id))))))")
                        .bind(reservation).bind(row.object_id).bind(&row.object_key).bind(&row.byte_hash).bind(row.byte_size).fetch_one(&mut **tx).await?
                }
            }
            _=>false,
        };
        if !valid {
            return Err(invalid());
        }
    }
    Ok(())
}
