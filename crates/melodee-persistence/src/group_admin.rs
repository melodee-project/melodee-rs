//! Operator library-access-group administration over the group
//! schema (migration 0010): `native_library_access_groups`,
//! `native_user_library_groups`, `native_library_group_libraries`
//! (PRD-CLI-001, PRD-AUTH-004).
//!
//! CLI home. (admin API) owns the route layer and was assigned
//! admin.rs/native routes only; this module is the shared persistence
//! implementation it can reuse. Semantics mirror the admin API contract:
//! delete fails closed while the group still has members or library
//! bindings, and every mutation is bounded and audit-timestamped.

use sqlx::{FromRow, PgPool};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::normalize_login;

/// Bound for operator-supplied group names.
pub const MAX_GROUP_NAME_LENGTH: usize = 200;

/// One access group with its membership/binding footprint for operator
/// impact reporting (a delete is never attempted blind).
#[derive(Clone, Debug, Eq, FromRow, PartialEq)]
pub struct StoredAccessGroup {
    pub id: Uuid,
    pub public_api_id: Uuid,
    pub name: String,
    pub member_count: i64,
    pub binding_count: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Error)]
pub enum GroupAdminError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("group name must be non-blank and at most 200 characters")]
    InvalidGroupName,
    #[error("no group with that exact name exists")]
    GroupNotFound,
    #[error("group name is ambiguous; resolve by listing groups first")]
    AmbiguousGroupName,
    #[error("a group with that exact name already exists")]
    GroupExists,
    #[error("no user with that username exists")]
    UserNotFound,
    #[error("group still has members; remove them before deleting")]
    GroupHasMembers,
    #[error("group still has library bindings; unbind them before deleting")]
    GroupHasBindings,
}

/// Public member facts of one group; internal user ids never leave the store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessGroupMember {
    pub public_api_id: Uuid,
    pub username: String,
}

/// Full operator view of one group (admin API detail route).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessGroupDetail {
    pub group: StoredAccessGroup,
    pub members: Vec<AccessGroupMember>,
    pub library_ids: Vec<Uuid>,
}

/// Stateless entry point for group administration.
#[derive(Clone, Copy, Debug, Default)]
pub struct GroupAdminStore;

#[derive(FromRow)]
struct GroupRow {
    id: Uuid,
    public_api_id: Uuid,
    name: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl GroupAdminStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Lists every access group with member/binding counts, ordered by name.
    /// Bounded by the group namespace (one row per access group).
    pub async fn list_groups(
        &self,
        pool: &PgPool,
    ) -> Result<Vec<StoredAccessGroup>, GroupAdminError> {
        Ok(sqlx::query_as::<_, StoredAccessGroup>(
            "SELECT g.id,g.public_api_id,g.name,
                    (SELECT count(*) FROM native_user_library_groups m WHERE m.group_id=g.id) AS member_count,
                    (SELECT count(*) FROM native_library_group_libraries b WHERE b.group_id=g.id) AS binding_count,
                    g.created_at,g.updated_at
             FROM native_library_access_groups g ORDER BY g.name,g.id",
        )
        .fetch_all(pool)
        .await?)
    }

    /// Creates a group with fresh v7 identities. Fails closed on an exact
    /// name conflict so CLI name resolution can never become ambiguous.
    pub async fn create_group(
        &self,
        pool: &PgPool,
        name: &str,
        now: OffsetDateTime,
    ) -> Result<StoredAccessGroup, GroupAdminError> {
        let name = valid_group_name(name)?;
        let mut tx = pool.begin().await?;
        // Serialize name-uniqueness across concurrent operator processes.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('native_library_access_groups_name'))")
            .execute(&mut *tx)
            .await?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM native_library_access_groups WHERE name=$1)",
        )
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            tx.rollback().await?;
            return Err(GroupAdminError::GroupExists);
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO native_library_access_groups(id,public_api_id,name,created_at,updated_at)
             VALUES($1,$2,$3,$4,$4)",
        )
        .bind(id)
        .bind(Uuid::now_v7())
        .bind(name)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let group = resolve_group(pool, name).await?;
        group_with_counts(pool, group).await
    }

    /// Deletes a group only when it has no members and no library bindings
    /// (fail closed: an access-control group is never silently detached).
    /// The row lock serializes the footprint check with concurrent
    /// membership/binding inserts, which take the same row's FK share lock.
    pub async fn delete_group(&self, pool: &PgPool, name: &str) -> Result<(), GroupAdminError> {
        let name = valid_group_name(name)?;
        let mut tx = pool.begin().await?;
        let group_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM native_library_access_groups WHERE name=$1 ORDER BY id FOR UPDATE",
        )
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(GroupAdminError::GroupNotFound)?;
        let (members, bindings) = group_footprint_tx(&mut tx, group_id).await?;
        if members > 0 {
            tx.rollback().await?;
            return Err(GroupAdminError::GroupHasMembers);
        }
        if bindings > 0 {
            tx.rollback().await?;
            return Err(GroupAdminError::GroupHasBindings);
        }
        sqlx::query("DELETE FROM native_library_access_groups WHERE id=$1")
            .bind(group_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Adds a user (resolved by login-normalized username) to a group.
    /// Idempotent: returns `false` when the membership already existed.
    pub async fn add_user(
        &self,
        pool: &PgPool,
        group: &str,
        username: &str,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let group = resolve_group(pool, valid_group_name(group)?).await?;
        let user_id = resolve_user(pool, username).await?;
        let inserted = sqlx::query(
            "INSERT INTO native_user_library_groups(user_id,group_id,created_at)
             VALUES($1,$2,$3) ON CONFLICT (user_id,group_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(group.id)
        .bind(now)
        .execute(pool)
        .await?;
        touch_group(pool, group.id, now).await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Removes a user's membership. Returns `false` when none existed.
    pub async fn remove_user(
        &self,
        pool: &PgPool,
        group: &str,
        username: &str,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let group = resolve_group(pool, valid_group_name(group)?).await?;
        let user_id = resolve_user(pool, username).await?;
        let removed =
            sqlx::query("DELETE FROM native_user_library_groups WHERE user_id=$1 AND group_id=$2")
                .bind(user_id)
                .bind(group.id)
                .execute(pool)
                .await?;
        touch_group(pool, group.id, now).await?;
        Ok(removed.rows_affected() == 1)
    }

    /// Binds a library to a group. Once any group binds a library, the
    /// access function requires membership in a bound group — this
    /// is the captured restrictive transition, never a silent widen.
    /// Idempotent: returns `false` when the binding already existed.
    pub async fn bind_library(
        &self,
        pool: &PgPool,
        group: &str,
        library_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let group = resolve_group(pool, valid_group_name(group)?).await?;
        let inserted = sqlx::query(
            "INSERT INTO native_library_group_libraries(group_id,library_id,imported_at)
             VALUES($1,$2,$3) ON CONFLICT (group_id,library_id) DO NOTHING",
        )
        .bind(group.id)
        .bind(library_id)
        .bind(now)
        .execute(pool)
        .await?;
        touch_group(pool, group.id, now).await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Removes a library binding. Returns `false` when none existed.
    pub async fn unbind_library(
        &self,
        pool: &PgPool,
        group: &str,
        library_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let group = resolve_group(pool, valid_group_name(group)?).await?;
        let removed = sqlx::query(
            "DELETE FROM native_library_group_libraries WHERE group_id=$1 AND library_id=$2",
        )
        .bind(group.id)
        .bind(library_id)
        .execute(pool)
        .await?;
        touch_group(pool, group.id, now).await?;
        Ok(removed.rows_affected() == 1)
    }

    // ------------------------------------------------------------------
    // admin API surface: public-id-addressed variants. HTTP routes
    // address groups and users by immutable public ids (names are display
    // values); semantics are identical to the name-addressed CLI methods.
    // ------------------------------------------------------------------

    /// One group with member public ids and library bindings (operator view).
    pub async fn group_detail(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<AccessGroupDetail>, GroupAdminError> {
        let Some(group) = resolve_group_by_public_id(pool, public_api_id).await? else {
            return Ok(None);
        };
        let group = group_with_counts(pool, group).await?;
        let members = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT u.public_api_id, u.username FROM native_user_library_groups m
             JOIN native_users u ON u.id = m.user_id
             WHERE m.group_id=$1 ORDER BY lower(u.username), u.public_api_id LIMIT 10000",
        )
        .bind(group.id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(public_api_id, username)| AccessGroupMember {
            public_api_id,
            username,
        })
        .collect();
        let library_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT library_id FROM native_library_group_libraries
             WHERE group_id=$1 ORDER BY library_id LIMIT 10000",
        )
        .bind(group.id)
        .fetch_all(pool)
        .await?;
        Ok(Some(AccessGroupDetail {
            group,
            members,
            library_ids,
        }))
    }

    /// Renames one group addressed by public id. Fails closed when the new
    /// exact name is already held by another group.
    pub async fn rename_group(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
        name: &str,
        now: OffsetDateTime,
    ) -> Result<StoredAccessGroup, GroupAdminError> {
        let name = valid_group_name(name)?;
        let group = resolve_group_by_public_id(pool, public_api_id)
            .await?
            .ok_or(GroupAdminError::GroupNotFound)?;
        let name_taken = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM native_library_access_groups WHERE name=$1 AND id<>$2)",
        )
        .bind(name)
        .bind(group.id)
        .fetch_one(pool)
        .await?;
        if name_taken {
            return Err(GroupAdminError::GroupExists);
        }
        sqlx::query("UPDATE native_library_access_groups SET name=$2, updated_at=$3 WHERE id=$1")
            .bind(group.id)
            .bind(name)
            .bind(now)
            .execute(pool)
            .await?;
        // Re-read so the returned row carries the new name and updated_at.
        let renamed = resolve_group_by_public_id(pool, public_api_id)
            .await?
            .ok_or(GroupAdminError::GroupNotFound)?;
        group_with_counts(pool, renamed).await
    }

    /// Deletes one group addressed by public id, fail closed while members
    /// or library bindings remain. The row lock serializes the footprint
    /// check with concurrent membership/binding writes.
    pub async fn delete_group_by_public_id(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<(), GroupAdminError> {
        let mut tx = pool.begin().await?;
        let group_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM native_library_access_groups WHERE public_api_id=$1 FOR UPDATE",
        )
        .bind(public_api_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(GroupAdminError::GroupNotFound)?;
        let (members, bindings) = group_footprint_tx(&mut tx, group_id).await?;
        if members > 0 {
            tx.rollback().await?;
            return Err(GroupAdminError::GroupHasMembers);
        }
        if bindings > 0 {
            tx.rollback().await?;
            return Err(GroupAdminError::GroupHasBindings);
        }
        sqlx::query("DELETE FROM native_library_access_groups WHERE id=$1")
            .bind(group_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Adds a user to a group, both addressed by public id. Idempotent:
    /// returns `false` when the membership already existed.
    pub async fn add_member_by_public_ids(
        &self,
        pool: &PgPool,
        group_public_api_id: Uuid,
        user_public_api_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let (group_id, user_id) =
            resolve_public_ids(pool, group_public_api_id, user_public_api_id).await?;
        let inserted = sqlx::query(
            "INSERT INTO native_user_library_groups(user_id,group_id,created_at)
             VALUES($1,$2,$3) ON CONFLICT (user_id,group_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(group_id)
        .bind(now)
        .execute(pool)
        .await?;
        touch_group(pool, group_id, now).await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Removes a membership, both sides addressed by public id. Returns
    /// `false` when none existed.
    pub async fn remove_member_by_public_ids(
        &self,
        pool: &PgPool,
        group_public_api_id: Uuid,
        user_public_api_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let (group_id, user_id) =
            resolve_public_ids(pool, group_public_api_id, user_public_api_id).await?;
        let removed =
            sqlx::query("DELETE FROM native_user_library_groups WHERE user_id=$1 AND group_id=$2")
                .bind(user_id)
                .bind(group_id)
                .execute(pool)
                .await?;
        touch_group(pool, group_id, now).await?;
        Ok(removed.rows_affected() == 1)
    }

    /// Binds a library to a group addressed by public id. Idempotent:
    /// returns `false` when the binding already existed.
    pub async fn bind_library_by_public_id(
        &self,
        pool: &PgPool,
        group_public_api_id: Uuid,
        library_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let group_id = resolve_group_by_public_id(pool, group_public_api_id)
            .await?
            .ok_or(GroupAdminError::GroupNotFound)?
            .id;
        let inserted = sqlx::query(
            "INSERT INTO native_library_group_libraries(group_id,library_id,imported_at)
             VALUES($1,$2,$3) ON CONFLICT (group_id,library_id) DO NOTHING",
        )
        .bind(group_id)
        .bind(library_id)
        .bind(now)
        .execute(pool)
        .await?;
        touch_group(pool, group_id, now).await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Removes a library binding from a group addressed by public id.
    /// Returns `false` when none existed.
    pub async fn unbind_library_by_public_id(
        &self,
        pool: &PgPool,
        group_public_api_id: Uuid,
        library_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<bool, GroupAdminError> {
        let group_id = resolve_group_by_public_id(pool, group_public_api_id)
            .await?
            .ok_or(GroupAdminError::GroupNotFound)?
            .id;
        let removed = sqlx::query(
            "DELETE FROM native_library_group_libraries WHERE group_id=$1 AND library_id=$2",
        )
        .bind(group_id)
        .bind(library_id)
        .execute(pool)
        .await?;
        touch_group(pool, group_id, now).await?;
        Ok(removed.rows_affected() == 1)
    }
}

fn valid_group_name(name: &str) -> Result<&str, GroupAdminError> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_GROUP_NAME_LENGTH {
        Err(GroupAdminError::InvalidGroupName)
    } else {
        Ok(name)
    }
}

async fn resolve_group(pool: &PgPool, name: &str) -> Result<GroupRow, GroupAdminError> {
    let rows = sqlx::query_as::<_, GroupRow>(
        "SELECT id,public_api_id,name,created_at,updated_at
         FROM native_library_access_groups WHERE name=$1 ORDER BY id LIMIT 2",
    )
    .bind(name)
    .fetch_all(pool)
    .await?;
    match rows.as_slice() {
        [group] => Ok(GroupRow {
            id: group.id,
            public_api_id: group.public_api_id,
            name: group.name.clone(),
            created_at: group.created_at,
            updated_at: group.updated_at,
        }),
        [] => Err(GroupAdminError::GroupNotFound),
        _ => Err(GroupAdminError::AmbiguousGroupName),
    }
}

async fn resolve_group_by_public_id(
    pool: &PgPool,
    public_api_id: Uuid,
) -> Result<Option<GroupRow>, GroupAdminError> {
    sqlx::query_as::<_, GroupRow>(
        "SELECT id,public_api_id,name,created_at,updated_at
         FROM native_library_access_groups WHERE public_api_id=$1",
    )
    .bind(public_api_id)
    .fetch_optional(pool)
    .await
    .map_err(GroupAdminError::Database)
}

/// Resolves (group_id, user_id) from public ids.
async fn resolve_public_ids(
    pool: &PgPool,
    group_public_api_id: Uuid,
    user_public_api_id: Uuid,
) -> Result<(Uuid, Uuid), GroupAdminError> {
    let group_id = resolve_group_by_public_id(pool, group_public_api_id)
        .await?
        .ok_or(GroupAdminError::GroupNotFound)?
        .id;
    let user_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM native_users WHERE public_api_id=$1")
            .bind(user_public_api_id)
            .fetch_optional(pool)
            .await?
            .ok_or(GroupAdminError::UserNotFound)?;
    Ok((group_id, user_id))
}

async fn resolve_user(pool: &PgPool, username: &str) -> Result<Uuid, GroupAdminError> {
    let normalized = normalize_login(username);
    if normalized.is_empty() {
        return Err(GroupAdminError::UserNotFound);
    }
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM native_users WHERE normalized_username=$1")
        .bind(normalized)
        .fetch_optional(pool)
        .await?
        .ok_or(GroupAdminError::UserNotFound)
}

async fn group_footprint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group_id: Uuid,
) -> Result<(i64, i64), GroupAdminError> {
    Ok(sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM native_user_library_groups WHERE group_id=$1),
                (SELECT count(*) FROM native_library_group_libraries WHERE group_id=$1)",
    )
    .bind(group_id)
    .fetch_one(&mut **tx)
    .await?)
}

async fn group_with_counts(
    pool: &PgPool,
    group: GroupRow,
) -> Result<StoredAccessGroup, GroupAdminError> {
    let (member_count, binding_count) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM native_user_library_groups WHERE group_id=$1),
                (SELECT count(*) FROM native_library_group_libraries WHERE group_id=$1)",
    )
    .bind(group.id)
    .fetch_one(pool)
    .await?;
    Ok(StoredAccessGroup {
        id: group.id,
        public_api_id: group.public_api_id,
        name: group.name,
        member_count,
        binding_count,
        created_at: group.created_at,
        updated_at: group.updated_at,
    })
}

async fn touch_group(
    pool: &PgPool,
    group_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), GroupAdminError> {
    sqlx::query("UPDATE native_library_access_groups SET updated_at=$2 WHERE id=$1")
        .bind(group_id)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}
