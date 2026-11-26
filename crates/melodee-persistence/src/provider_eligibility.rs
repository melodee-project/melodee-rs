//! Revisioned external-provider eligibility, backfill progress and the
//! installed-backlog reconciliation (ADR-0212, PRD-EXT-001..004,
//! PRD-WORK-003, PRD-INGEST-010).
//!
//! The record is deliberately tiny: one boolean pair per provider plus a
//! revision counter. Credentials never enter this table, a log line, or an
//! API response — eligibility is the only derived fact that leaves the
//! settings resolver.
//!
//! Three invariants define the module:
//!
//! 1. **Unknown means "not yet resolved".** An empty `provider_eligibility`
//!    table is a fresh installation: scheduling keeps every known provider so
//!    imports become playable, and the first reconciliation reconciles the
//!    backlog. A resolved table is authoritative.
//! 2. **One transition, one revision.** Each effective change increments the
//!    installation revision inside a transaction that locks the state row, so
//!    concurrent reconcilers serialize and repeated refreshes are no-ops.
//! 3. **No eligible work is forgotten.** Disablement turns never-attempted
//!    work into nothing and historically attempted work into `dormant`;
//!    re-enablement revives it exactly once and starts a revisioned backfill
//!    that covers the entities that were not imported while the provider was
//!    off.

use std::collections::BTreeMap;

use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::KNOWN_PROVIDERS;
use crate::provider_enrichment::{EnrichmentEntityType, EnrichmentPurpose};

/// Page size for a keyset backfill. One page is one transaction.
pub const BACKFILL_PAGE_SIZE: i64 = 250;

#[derive(Debug, Error)]
pub enum ProviderEligibilityError {
    #[error("provider eligibility database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("provider eligibility request is invalid")]
    InvalidRequest,
}

/// Stored effective eligibility for one provider.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ProviderEligibility {
    pub provider: String,
    pub revision: i64,
    pub metadata_eligible: bool,
    pub artwork_eligible: bool,
    pub observed_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl ProviderEligibility {
    #[must_use]
    pub const fn eligible_for(&self, purpose: EnrichmentPurpose) -> bool {
        match purpose {
            EnrichmentPurpose::ArtistMetadata | EnrichmentPurpose::ReleaseMetadata => {
                self.metadata_eligible
            }
            EnrichmentPurpose::ArtistArtwork | EnrichmentPurpose::ReleaseArtwork => {
                self.artwork_eligible
            }
        }
    }
}

/// Installation-wide eligibility state, including the resolver diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ProviderEligibilityState {
    pub revision: i64,
    pub refreshed_at: Option<OffsetDateTime>,
    pub last_error: Option<String>,
    pub last_error_at: Option<OffsetDateTime>,
}

/// What one reconciliation pass changed. Used by tests and operator output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EligibilityRefreshOutcome {
    pub revision: i64,
    pub changed_providers: Vec<String>,
    pub backfills_created: u64,
    pub dormant_revived: u64,
    pub dormant_suspended: u64,
    pub never_attempted_removed: u64,
    pub backfills_fenced: u64,
}

/// One running backfill and its keyset cursor.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ProviderBackfill {
    pub id: Uuid,
    pub provider: String,
    pub provider_revision: i64,
    pub entity_type: String,
    pub status: String,
    pub last_entity_id: Option<Uuid>,
    pub entities_scanned: i64,
    pub tasks_enqueued: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

/// Result of advancing one backfill page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillPage {
    pub provider: String,
    pub provider_revision: i64,
    pub entity_type: EnrichmentEntityType,
    pub entities: u32,
    pub tasks_enqueued: u64,
    pub completed: bool,
    pub last_entity_id: Option<Uuid>,
}

/// Per-provider runnable/dormant/retry/failed counters for operator views.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ProviderQueueStats {
    pub provider: String,
    pub runnable: i64,
    pub running: i64,
    pub dormant: i64,
    pub retry: i64,
    pub failed: i64,
    pub succeeded: i64,
    pub oldest_runnable_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderEligibilityStore;

impl ProviderEligibilityStore {
    /// Current per-provider eligibility, ordered by provider.
    pub async fn list(pool: &PgPool) -> Result<Vec<ProviderEligibility>, ProviderEligibilityError> {
        Ok(sqlx::query_as::<_, ProviderEligibility>(
            "SELECT provider, revision, metadata_eligible, artwork_eligible,
                    observed_at, updated_at
               FROM provider_eligibility
              ORDER BY provider",
        )
        .fetch_all(pool)
        .await?)
    }

    /// Installation state, creating the singleton if an operator removed it.
    pub async fn state(
        pool: &PgPool,
    ) -> Result<ProviderEligibilityState, ProviderEligibilityError> {
        let row = sqlx::query_as::<_, ProviderEligibilityState>(
            "SELECT revision, refreshed_at, last_error, last_error_at
               FROM provider_eligibility_state WHERE singleton",
        )
        .fetch_optional(pool)
        .await?;
        match row {
            Some(state) => Ok(state),
            None => {
                sqlx::query(
                    "INSERT INTO provider_eligibility_state (singleton, revision)
                     VALUES (true, 0) ON CONFLICT DO NOTHING",
                )
                .execute(pool)
                .await?;
                Ok(sqlx::query_as::<_, ProviderEligibilityState>(
                    "SELECT revision, refreshed_at, last_error, last_error_at
                       FROM provider_eligibility_state WHERE singleton",
                )
                .fetch_one(pool)
                .await?)
            }
        }
    }

    /// True when eligibility has been resolved at least once.
    pub async fn is_resolved(pool: &PgPool) -> Result<bool, ProviderEligibilityError> {
        Ok(
            sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM provider_eligibility)")
                .fetch_one(pool)
                .await?,
        )
    }

    /// Records a resolver failure without touching the last known state, so a
    /// transient settings-read error cannot silently disable enrichment.
    pub async fn record_resolver_failure(
        pool: &PgPool,
        message: &str,
        now: OffsetDateTime,
    ) -> Result<(), ProviderEligibilityError> {
        sqlx::query(
            "INSERT INTO provider_eligibility_state (singleton, revision)
             VALUES (true, 0) ON CONFLICT DO NOTHING",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "UPDATE provider_eligibility_state
                SET last_error = $1, last_error_at = $2
              WHERE singleton",
        )
        .bind(message.chars().take(1_000).collect::<String>())
        .bind(now)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Applies a resolved eligibility snapshot.
    ///
    /// Serialized by the singleton state row. Unchanged providers cost one
    /// comparison; changed providers receive a new revision, their backlog is
    /// reconciled, and eligible providers receive a durable backfill.
    pub async fn refresh(
        pool: &PgPool,
        resolved: &[ResolvedProviderEligibilityOwned],
        now: OffsetDateTime,
    ) -> Result<EligibilityRefreshOutcome, ProviderEligibilityError> {
        if resolved.is_empty()
            || resolved
                .iter()
                .any(|entry| !KNOWN_PROVIDERS.contains(&entry.provider.as_str()))
        {
            return Err(ProviderEligibilityError::InvalidRequest);
        }
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO provider_eligibility_state (singleton, revision)
             VALUES (true, 0) ON CONFLICT DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        let mut outcome = EligibilityRefreshOutcome {
            revision: sqlx::query_scalar::<_, i64>(
                "SELECT revision FROM provider_eligibility_state
                  WHERE singleton FOR UPDATE",
            )
            .fetch_one(&mut *tx)
            .await?,
            ..EligibilityRefreshOutcome::default()
        };
        let existing = sqlx::query_as::<_, (String, i64, bool, bool)>(
            "SELECT provider, revision, metadata_eligible, artwork_eligible
               FROM provider_eligibility",
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|(provider, revision, metadata, artwork)| (provider, (revision, metadata, artwork)))
        .collect::<BTreeMap<_, _>>();

        for entry in resolved {
            let current = existing.get(&entry.provider).copied();
            if current.is_some_and(|(_, metadata, artwork)| {
                metadata == entry.metadata_eligible && artwork == entry.artwork_eligible
            }) {
                continue;
            }
            outcome.revision += 1;
            let revision = outcome.revision;
            sqlx::query(
                "INSERT INTO provider_eligibility (
                    provider, revision, metadata_eligible, artwork_eligible,
                    observed_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$5)
                 ON CONFLICT (provider) DO UPDATE
                 SET revision = EXCLUDED.revision,
                     metadata_eligible = EXCLUDED.metadata_eligible,
                     artwork_eligible = EXCLUDED.artwork_eligible,
                     observed_at = EXCLUDED.observed_at,
                     updated_at = EXCLUDED.updated_at",
            )
            .bind(&entry.provider)
            .bind(revision)
            .bind(entry.metadata_eligible)
            .bind(entry.artwork_eligible)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            outcome.changed_providers.push(entry.provider.clone());
            reconcile_provider(
                &mut tx,
                &entry.provider,
                revision,
                entry.metadata_eligible,
                entry.artwork_eligible,
                now,
                &mut outcome,
            )
            .await?;
        }

        if !outcome.changed_providers.is_empty() {
            sqlx::query(
                "UPDATE provider_eligibility_state
                    SET revision = $1, refreshed_at = $2, last_error = NULL,
                        last_error_at = NULL
                  WHERE singleton",
            )
            .bind(outcome.revision)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE provider_eligibility_state
                    SET refreshed_at = $1, last_error = NULL, last_error_at = NULL
                  WHERE singleton",
            )
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    /// Advances one running backfill by at most one keyset page.
    ///
    /// The backfill row is locked with `SKIP LOCKED`, so concurrent
    /// reconcilers advance distinct backfills. Enqueued tasks and the cursor
    /// commit together: a crash re-runs the page, and uniqueness makes the
    /// replay a no-op.
    pub async fn advance_backfill(
        pool: &PgPool,
        now: OffsetDateTime,
        page_size: i64,
    ) -> Result<Option<BackfillPage>, ProviderEligibilityError> {
        let page_size = page_size.clamp(1, BACKFILL_PAGE_SIZE);
        let mut tx = pool.begin().await?;
        let row = sqlx::query_as::<_, ProviderBackfill>(
            "SELECT id, provider, provider_revision, entity_type, status,
                    last_entity_id, entities_scanned, tasks_enqueued,
                    created_at, updated_at, completed_at
               FROM provider_enrichment_backfills
              WHERE status = 'running'
              ORDER BY created_at, id
              FOR UPDATE SKIP LOCKED
              LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(backfill) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let entity_type = EnrichmentEntityType::parse(&backfill.entity_type)
            .ok_or(ProviderEligibilityError::InvalidRequest)?;
        let entities: Vec<Uuid> = match entity_type {
            EnrichmentEntityType::Artist => {
                sqlx::query_scalar(
                    "SELECT id FROM artists
                      WHERE status IN ('active', 'verified')
                        AND ($1::uuid IS NULL OR id > $1)
                      ORDER BY id
                      LIMIT $2",
                )
                .bind(backfill.last_entity_id)
                .bind(page_size)
                .fetch_all(&mut *tx)
                .await?
            }
            EnrichmentEntityType::Release => {
                sqlx::query_scalar(
                    "SELECT id FROM releases
                      WHERE status = 'active'
                        AND ($1::uuid IS NULL OR id > $1)
                      ORDER BY id
                      LIMIT $2",
                )
                .bind(backfill.last_entity_id)
                .bind(page_size)
                .fetch_all(&mut *tx)
                .await?
            }
        };
        let enqueued = enqueue_for_entities(
            &mut tx,
            entity_type,
            &entities,
            Some(&backfill.provider),
            now,
        )
        .await?;
        let last_entity_id = entities.last().copied().or(backfill.last_entity_id);
        let short_page = i64::try_from(entities.len()).unwrap_or(0) < page_size;
        sqlx::query(
            "UPDATE provider_enrichment_backfills
                SET last_entity_id = $2,
                    entities_scanned = entities_scanned + $3,
                    tasks_enqueued = tasks_enqueued + $4,
                    status = CASE WHEN $5 THEN 'completed' ELSE 'running' END,
                    completed_at = CASE WHEN $5 THEN $6 ELSE NULL END,
                    updated_at = $6
              WHERE id = $1",
        )
        .bind(backfill.id)
        .bind(last_entity_id)
        .bind(i64::try_from(entities.len()).unwrap_or(0))
        .bind(i64::try_from(enqueued).unwrap_or(0))
        .bind(short_page)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(BackfillPage {
            provider: backfill.provider,
            provider_revision: backfill.provider_revision,
            entity_type,
            entities: u32::try_from(entities.len()).unwrap_or(u32::MAX),
            tasks_enqueued: enqueued,
            completed: short_page,
            last_entity_id,
        }))
    }

    /// True when any backfill is still running.
    pub async fn has_running_backfill(pool: &PgPool) -> Result<bool, ProviderEligibilityError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM provider_enrichment_backfills WHERE status = 'running')",
        )
        .fetch_one(pool)
        .await?)
    }

    pub async fn list_backfills(
        pool: &PgPool,
    ) -> Result<Vec<ProviderBackfill>, ProviderEligibilityError> {
        Ok(sqlx::query_as::<_, ProviderBackfill>(
            "SELECT id, provider, provider_revision, entity_type, status,
                    last_entity_id, entities_scanned, tasks_enqueued,
                    created_at, updated_at, completed_at
               FROM provider_enrichment_backfills
              ORDER BY provider, provider_revision, entity_type",
        )
        .fetch_all(pool)
        .await?)
    }

    /// Runnable/queued/dormant/retry/failed counters per provider for the
    /// Admin processing views. Excludes `melodee` self-search, which never
    /// creates background work.
    pub async fn queue_stats(
        pool: &PgPool,
    ) -> Result<Vec<ProviderQueueStats>, ProviderEligibilityError> {
        Ok(sqlx::query_as::<_, ProviderQueueStats>(
            "SELECT provider,
                    count(*) FILTER (WHERE status IN ('queued','retry')
                                       AND next_attempt_at <= now()) AS runnable,
                    count(*) FILTER (WHERE status = 'running') AS running,
                    count(*) FILTER (WHERE status = 'dormant') AS dormant,
                    count(*) FILTER (WHERE status = 'retry') AS retry,
                    count(*) FILTER (WHERE status = 'failed') AS failed,
                    count(*) FILTER (WHERE status = 'succeeded') AS succeeded,
                    min(created_at) FILTER (
                        WHERE status IN ('queued','retry') AND next_attempt_at <= now()
                    ) AS oldest_runnable_at
               FROM provider_enrichment_tasks
              WHERE provider <> 'melodee'
              GROUP BY provider
              ORDER BY provider",
        )
        .fetch_all(pool)
        .await?)
    }

    /// Oldest runnable job across eligible providers, the operator's
    /// enrichment-lag signal. Tracks provider lag, never the time to
    /// playable, which is measured separately.
    pub async fn oldest_runnable_at(
        pool: &PgPool,
    ) -> Result<Option<OffsetDateTime>, ProviderEligibilityError> {
        Ok(sqlx::query_scalar::<_, Option<OffsetDateTime>>(
            "SELECT min(created_at) FROM provider_enrichment_tasks
              WHERE status IN ('queued','retry') AND next_attempt_at <= now()
                AND provider <> 'melodee'",
        )
        .fetch_one(pool)
        .await?)
    }
}

/// Owned form of a resolved eligibility entry, so callers can build a
/// snapshot without borrowing the descriptor table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderEligibilityOwned {
    pub provider: String,
    pub metadata_eligible: bool,
    pub artwork_eligible: bool,
}

impl From<crate::providers::ResolvedProviderEligibility> for ResolvedProviderEligibilityOwned {
    fn from(value: crate::providers::ResolvedProviderEligibility) -> Self {
        Self {
            provider: value.provider.to_owned(),
            metadata_eligible: value.metadata_eligible,
            artwork_eligible: value.artwork_eligible,
        }
    }
}

/// Reconciles one provider's installed backlog after an eligibility change.
async fn reconcile_provider(
    tx: &mut Transaction<'_, Postgres>,
    provider: &str,
    revision: i64,
    metadata_eligible: bool,
    artwork_eligible: bool,
    now: OffsetDateTime,
    outcome: &mut EligibilityRefreshOutcome,
) -> Result<(), ProviderEligibilityError> {
    let mut suspended_purposes: Vec<&'static str> = Vec::new();
    let mut revived_purposes: Vec<&'static str> = Vec::new();
    if metadata_eligible {
        revived_purposes.extend([
            EnrichmentPurpose::ArtistMetadata.as_str(),
            EnrichmentPurpose::ReleaseMetadata.as_str(),
        ]);
    } else {
        suspended_purposes.extend([
            EnrichmentPurpose::ArtistMetadata.as_str(),
            EnrichmentPurpose::ReleaseMetadata.as_str(),
        ]);
    }
    if artwork_eligible {
        revived_purposes.extend([
            EnrichmentPurpose::ArtistArtwork.as_str(),
            EnrichmentPurpose::ReleaseArtwork.as_str(),
        ]);
    } else {
        suspended_purposes.extend([
            EnrichmentPurpose::ArtistArtwork.as_str(),
            EnrichmentPurpose::ReleaseArtwork.as_str(),
        ]);
    }

    // Never-attempted queued work carries no evidence worth keeping: it is
    // removed only after proving zero attempts and no lease.
    if !suspended_purposes.is_empty() {
        let removed = sqlx::query(
            "DELETE FROM provider_enrichment_tasks task
              WHERE task.provider = $1
                AND task.purpose = ANY($2)
                AND task.status = 'queued'
                AND task.attempt_count = 0
                AND task.lease_owner IS NULL
                AND NOT EXISTS (
                    SELECT 1 FROM provider_enrichment_attempts attempt
                     WHERE attempt.task_id = task.id
                )",
        )
        .bind(provider)
        .bind(&suspended_purposes)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        outcome.never_attempted_removed += removed;

        // Historically attempted unfinished work is suspended, not deleted:
        // its identity, attempts and retry budget survive for diagnosis and a
        // later enablement.
        let suspended = sqlx::query(
            "UPDATE provider_enrichment_tasks task
                SET status = 'dormant', dormant_at = $3, updated_at = $3,
                    lease_owner = NULL, lease_acquired_at = NULL,
                    lease_expires_at = NULL, provider_revision = $4
              WHERE task.provider = $1
                AND task.purpose = ANY($2)
                AND task.status IN ('queued', 'retry')
                AND task.attempt_count > 0",
        )
        .bind(provider)
        .bind(&suspended_purposes)
        .bind(now)
        .bind(revision)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        outcome.dormant_suspended += suspended;

        // A historical success whose only outcome was `disabled` is not
        // satisfaction: the provider never produced evidence.
        let disabled_only = sqlx::query(
            "UPDATE provider_enrichment_tasks task
                SET status = 'dormant', dormant_at = $3, updated_at = $3,
                    completed_at = NULL, provider_revision = $4
              WHERE task.provider = $1
                AND task.purpose = ANY($2)
                AND task.status = 'succeeded'
                AND EXISTS (
                    SELECT 1 FROM provider_enrichment_attempts attempt
                     WHERE attempt.task_id = task.id
                )
                AND NOT EXISTS (
                    SELECT 1 FROM provider_enrichment_attempts attempt
                     WHERE attempt.task_id = task.id AND attempt.outcome <> 'disabled'
                )",
        )
        .bind(provider)
        .bind(&suspended_purposes)
        .bind(now)
        .bind(revision)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        outcome.dormant_suspended += disabled_only;
    }

    if !revived_purposes.is_empty() {
        let revived = sqlx::query(
            "UPDATE provider_enrichment_tasks
                SET status = 'queued', dormant_at = NULL, next_attempt_at = $3,
                    updated_at = $3, provider_revision = $4
              WHERE provider = $1 AND purpose = ANY($2) AND status = 'dormant'",
        )
        .bind(provider)
        .bind(&revived_purposes)
        .bind(now)
        .bind(revision)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        outcome.dormant_revived += revived;

        // Enabling (or partially re-enabling) starts exactly one backfill per
        // entity type for this revision. `ON CONFLICT DO NOTHING` makes a
        // repeated reconciler a no-op.
        for entity_type in [EnrichmentEntityType::Artist, EnrichmentEntityType::Release] {
            let created = sqlx::query(
                "INSERT INTO provider_enrichment_backfills (
                    id, provider, provider_revision, entity_type, status,
                    created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,'running',$5,$5)
                 ON CONFLICT (provider, provider_revision, entity_type) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(provider)
            .bind(revision)
            .bind(entity_type.as_str())
            .bind(now)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            outcome.backfills_created += created;
        }
    }

    if suspended_purposes.len() == 4 {
        // Fully ineligible: fence any in-progress scan so a disabled provider
        // cannot keep enqueueing work.
        outcome.backfills_fenced += sqlx::query(
            "UPDATE provider_enrichment_backfills
                SET status = 'fenced', updated_at = $2
              WHERE provider = $1 AND status = 'running'",
        )
        .bind(provider)
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    }
    Ok(())
}

/// The single set-based scheduling statement shared by catalog import and
/// backfill. One statement per entity type, independent of the number of
/// entities or providers.
async fn enqueue_for_entities(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: EnrichmentEntityType,
    entities: &[Uuid],
    provider: Option<&str>,
    now: OffsetDateTime,
) -> Result<u64, ProviderEligibilityError> {
    enqueue_for_entities_with(tx, entity_type, entities, provider, now, false).await
}

async fn enqueue_for_entities_with(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: EnrichmentEntityType,
    entities: &[Uuid],
    provider: Option<&str>,
    now: OffsetDateTime,
    legacy: bool,
) -> Result<u64, ProviderEligibilityError> {
    if entities.is_empty() {
        return Ok(0);
    }
    let metadata_purpose = match entity_type {
        EnrichmentEntityType::Artist => EnrichmentPurpose::ArtistMetadata.as_str(),
        EnrichmentEntityType::Release => EnrichmentPurpose::ReleaseMetadata.as_str(),
    };
    let artwork_purpose = match entity_type {
        EnrichmentEntityType::Artist => EnrichmentPurpose::ArtistArtwork.as_str(),
        EnrichmentEntityType::Release => EnrichmentPurpose::ReleaseArtwork.as_str(),
    };
    if legacy {
        // The provider-eligibility record arrives with migration 0057. A
        // candidate binary may still import against a schema that predates it
        // (historical upgrade tests and a staged operator upgrade); scheduling
        // then keeps the pre-ADR shape: every known provider, one bounded
        // statement, no eligibility join.
        return Ok(sqlx::query(
            "WITH entities AS (SELECT DISTINCT unnest($1::uuid[]) AS entity_id),
             eligible AS (
                 SELECT known.provider, purpose.name
                   FROM unnest($4::text[]) AS known(provider)
                   CROSS JOIN (VALUES ($2), ($3)) AS purpose(name)
             )
             INSERT INTO provider_enrichment_tasks (
                 id, entity_type, entity_id, provider, purpose, request_key,
                 status, attempt_count, next_attempt_at, created_at, updated_at
             )
             SELECT uuidv7(), $5, entities.entity_id, eligible.provider, eligible.name,
                    $5 || ':' || entities.entity_id::text || ':' || eligible.provider
                       || ':' || eligible.name,
                    'queued', 0, $6, $6, $6
               FROM entities CROSS JOIN eligible
             ON CONFLICT (entity_type, entity_id, provider, purpose, request_key)
             DO NOTHING",
        )
        .bind(entities)
        .bind(metadata_purpose)
        .bind(artwork_purpose)
        .bind(KNOWN_PROVIDERS)
        .bind(entity_type.as_str())
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected());
    }
    Ok(sqlx::query(
        "WITH entities AS (SELECT DISTINCT unnest($1::uuid[]) AS entity_id),
         eligible AS (
             SELECT eligibility.provider, eligibility.revision, purpose.name, purpose.eligible
               FROM provider_eligibility eligibility
               CROSS JOIN LATERAL (VALUES
                   ($3, eligibility.metadata_eligible),
                   ($4, eligibility.artwork_eligible)
               ) AS purpose(name, eligible)
              WHERE purpose.eligible
                AND ($2::text IS NULL OR eligibility.provider = $2)
             UNION ALL
             SELECT known.provider, NULL::bigint, purpose.name, true
               FROM unnest($5::text[]) AS known(provider)
               CROSS JOIN (VALUES ($3), ($4)) AS purpose(name)
              WHERE NOT EXISTS (SELECT 1 FROM provider_eligibility)
                AND ($2::text IS NULL OR known.provider = $2)
         )
         INSERT INTO provider_enrichment_tasks (
             id, entity_type, entity_id, provider, purpose, request_key,
             status, attempt_count, next_attempt_at, created_at, updated_at,
             provider_revision
         )
         SELECT uuidv7(), $6, entities.entity_id, eligible.provider, eligible.name,
                $6 || ':' || entities.entity_id::text || ':' || eligible.provider
                   || ':' || eligible.name,
                'queued', 0, $7, $7, $7, eligible.revision
           FROM entities CROSS JOIN eligible
         ON CONFLICT (entity_type, entity_id, provider, purpose, request_key)
         DO NOTHING",
    )
    .bind(entities)
    .bind(provider)
    .bind(metadata_purpose)
    .bind(artwork_purpose)
    .bind(KNOWN_PROVIDERS)
    .bind(entity_type.as_str())
    .bind(now)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

/// Schedules provider work for the entities one catalog import touched, from
/// inside the catalog transaction.
///
/// Provider eligibility is joined in SQL; no per-provider loop, network I/O,
/// capability resolution or catalog scan runs here. Unknown eligibility (a
/// fresh installation) keeps every known provider so media becomes playable.
pub async fn schedule_catalog_enrichment(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    artist_ids: &[Uuid],
    now: OffsetDateTime,
) -> Result<u64, ProviderEligibilityError> {
    // One cheap catalog lookup decides whether the revisioned eligibility
    // record is present. The import statements themselves stay bounded.
    let legacy =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('public.provider_eligibility') IS NULL")
            .fetch_one(&mut **tx)
            .await?;
    let artists = enqueue_for_entities_with(
        tx,
        EnrichmentEntityType::Artist,
        artist_ids,
        None,
        now,
        legacy,
    )
    .await?;
    let releases = enqueue_for_entities_with(
        tx,
        EnrichmentEntityType::Release,
        &[release_id],
        None,
        now,
        legacy,
    )
    .await?;
    Ok(artists + releases)
}
