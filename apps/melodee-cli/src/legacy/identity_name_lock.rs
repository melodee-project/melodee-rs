//! Name-scoped serialization shared with catalog import (ADR-0189).

use sqlx::{Postgres, Transaction};

/// Lock the complete batch before any affected Artist row. The transaction
/// owns these advisory locks until commit/rollback; names remain non-unique.
pub(super) async fn lock_names(
    tx: &mut Transaction<'_, Postgres>,
    names: &[String],
) -> Result<(), String> {
    if names.is_empty() {
        return Ok(());
    }
    let available: bool = sqlx::query_scalar(
        "SELECT to_regprocedure('public.lock_artist_identity_names(text[])') IS NOT NULL",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| format!("cannot inspect Artist name-lock helper: {e}"))?;
    if available {
        sqlx::query("SELECT lock_artist_identity_names($1)")
            .bind(names)
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("cannot lock Artist names: {e}"))?;
    } else {
        // Historical-schema migration tests can run before 0064. Sort the
        // actual database-computed lock keys, not the input strings.
        let keys: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT hashtextextended(name, 0) AS key
             FROM unnest($1::text[]) AS name ORDER BY key",
        )
        .bind(names)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| format!("cannot sort Artist name lock keys: {e}"))?;
        for key in keys {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(key)
                .execute(&mut **tx)
                .await
                .map_err(|e| format!("cannot lock Artist name: {e}"))?;
        }
    }
    Ok(())
}
