//! Supply v3 handoffs to historical schema tests without starting today's
//! ingestion workers against an old database. Keep the historical constraints
//! active while loading the five immutable publication records.
use crate::catalog_common::{
    CatalogFixturePresentation, PublishedCatalogFixture, seed_published_release,
};
use crate::common::TestDb;
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, PolicyClaim, WorkItemStore, WorkerId,
};
use time::OffsetDateTime;

pub async fn import(
    source: &TestDb,
    historical: &TestDb,
    ordinal: u32,
    title: &str,
) -> PublishedCatalogFixture {
    let fixture = seed_published_release(
        source,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            ..Default::default()
        },
    )
    .await;
    let manifest_row_id: i64 =
        sqlx::query_scalar("SELECT id FROM release_work_item_manifests WHERE work_item_id=$1")
            .bind(fixture.work_item_id)
            .fetch_one(&source.pool)
            .await
            .expect("read synthetic manifest row");
    let manifest_payload =
        melodee_persistence::manifest_evidence::load_manifest(&source.pool, manifest_row_id)
            .await
            .expect("read synthetic manifest evidence");
    let mut handoff: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_array(to_jsonb(w),to_jsonb(m),to_jsonb(a),to_jsonb(p),to_jsonb(r))
           FROM release_work_items w
           JOIN release_work_item_manifests m ON m.work_item_id=w.id
           JOIN release_publication_authorizations a ON a.work_item_id=w.id
           JOIN release_publication_plans p ON p.work_item_id=w.id
           JOIN release_publication_receipts r ON r.work_item_id=w.id WHERE w.id=$1",
    )
    .bind(fixture.work_item_id)
    .fetch_one(&source.pool)
    .await
    .expect("read synthetic v3 publication");
    // ADR-0213: current manifests keep their payload as evidence, while the
    // historical snapshot still requires an inline column. The fixture
    // restores the payload for the copy only.
    if let Some(manifest) = handoff.get_mut(1) {
        manifest["manifest_payload"] = manifest_payload;
    }
    for (index, insert) in [
        "INSERT INTO release_work_items SELECT * FROM jsonb_populate_record(NULL::release_work_items,$1)",
        "INSERT INTO release_work_item_manifests OVERRIDING SYSTEM VALUE SELECT * FROM jsonb_populate_record(NULL::release_work_item_manifests,$1)",
        "INSERT INTO release_publication_authorizations SELECT * FROM jsonb_populate_record(NULL::release_publication_authorizations,$1)",
        "INSERT INTO release_publication_plans SELECT * FROM jsonb_populate_record(NULL::release_publication_plans,$1)",
        "INSERT INTO release_publication_receipts SELECT * FROM jsonb_populate_record(NULL::release_publication_receipts,$1)",
    ].iter().enumerate() {
        sqlx::query(*insert).bind(&handoff[index]).execute(&historical.pool).await.expect("load into historical schema");
    }
    let worker = WorkerId::new();
    sqlx::query(
        "UPDATE release_work_items SET state='cataloging',lease_owner=$2,lease_acquired_at=now(),
        lease_expires_at=now()+interval '5 minutes' WHERE id=$1 AND state='published'",
    )
    .bind(fixture.work_item_id)
    .bind(worker.as_str())
    .execute(&historical.pool)
    .await
    .expect("historical catalog lease");
    CatalogStore
        .import_manifest_v3(
            &historical.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("historical-fixture-v1", "migration-test"),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("import historical handoff");
    fixture
}

#[allow(dead_code)]
pub async fn policy_claim(
    db: &TestDb,
    fixture: &PublishedCatalogFixture,
    title: &str,
    worker: &WorkerId,
) -> PolicyClaim {
    sqlx::query("UPDATE release_work_items SET lease_owner=$2,lease_acquired_at=now(),lease_expires_at=now()+interval '5 minutes'
        WHERE id=$1 AND state='cataloged'").bind(fixture.work_item_id).bind(worker.as_str()).execute(&db.pool).await.expect("historical policy lease");
    PolicyClaim {
        item: WorkItemStore
            .get(&db.pool, fixture.work_item_id)
            .await
            .expect("historical cataloged work item"),
        release_id: fixture.release_id,
        title: title.to_owned(),
        source_fingerprint: fixture.sealed_manifest_hash.clone(),
        semantic_manifest_hash: fixture.semantic_manifest_hash.clone(),
    }
}
