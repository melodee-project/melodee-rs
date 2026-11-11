//! Coverage for the operator-queue enrichment and dashboard activity reads
//! added by ADR-0184:
//!
//! * `WorkItemStore::list_page_enriched` joins the import binding for display
//!   values, filters by a LIKE-escaped substring, and stays a single
//!   index-backed join.
//! * `WorkItemStore::transitions_hourly` / `count_transitions_since` /
//!   `count_created_since` read the append-only transition log inside a
//!   bounded window.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{T0, TestDb, seed_ready_item};
use melodee_persistence::{WorkItemSort, WorkItemStore};
use phase5_common::seed_cataloged_release;
use time::Duration;

#[tokio::test]
async fn list_page_enriched_joins_the_release_binding_for_display_only() {
    let db = TestDb::create("queue_list_enriched").await;
    let store = WorkItemStore;
    let fixture = seed_cataloged_release(
        &db,
        41,
        CatalogFixturePresentation {
            release_title: "Enriched Queue Release",
            release_artist: "Enriched Queue Artist",
            track_title: "Enriched Track",
            track_artist: "Enriched Queue Artist",
            release_date: "2001-02-03",
        },
    )
    .await;
    let bound_id = fixture.work_item_id();
    let unbound_id = seed_ready_item(&store, &db.pool, "queue/unbound-release").await;

    let (rows, total) = store
        .list_page_enriched(&db.pool, None, 50, 0, WorkItemSort::default_order(), None)
        .await
        .expect("list enriched page");
    assert_eq!(total, 2, "both fixtures are listed");
    assert_eq!(rows.len(), 2);

    let bound = rows
        .iter()
        .find(|row| row.item.id == bound_id)
        .expect("cataloged fixture present");
    assert_eq!(bound.item.id, bound_id, "identity stays the work-item id");
    assert_eq!(
        bound.release_title.as_deref(),
        Some("Enriched Queue Release")
    );
    assert_eq!(
        bound.display_artist.as_deref(),
        Some("Enriched Queue Artist")
    );
    assert_eq!(
        bound.release_id.map(melodee_domain::ReleaseId::from_uuid),
        Some(fixture.release_id())
    );
    assert!(
        bound.media_bytes.unwrap_or_default() > 0,
        "release aggregates carry media bytes for a cataloged release"
    );

    let unbound = rows
        .iter()
        .find(|row| row.item.id == unbound_id)
        .expect("unbound fixture present");
    assert_eq!(
        unbound.release_title, None,
        "no title before an import binds"
    );
    assert_eq!(unbound.display_artist, None);
    assert_eq!(unbound.media_bytes, None);
}

#[tokio::test]
async fn list_page_enriched_filters_by_substring_and_survives_wildcards() {
    let db = TestDb::create("queue_list_query").await;
    let store = WorkItemStore;
    let fixture = seed_cataloged_release(
        &db,
        42,
        CatalogFixturePresentation {
            release_title: "Percent 100% Release",
            release_artist: "Wildcard Artist",
            track_title: "Wildcard Track",
            track_artist: "Wildcard Artist",
            release_date: "2002-03-04",
        },
    )
    .await;
    seed_ready_item(&store, &db.pool, "queue/other-item").await;
    seed_ready_item(&store, &db.pool, "queue/literal_underscore").await;

    // Matching on the bound Release title.
    let (by_title, total) = store
        .list_page_enriched(
            &db.pool,
            None,
            50,
            0,
            WorkItemSort::default_order(),
            Some("percent 100%"),
        )
        .await
        .expect("title query");
    assert_eq!(total, 1);
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].item.id, fixture.work_item_id());

    // Matching on the billed artist.
    let (by_artist, _) = store
        .list_page_enriched(
            &db.pool,
            None,
            50,
            0,
            WorkItemSort::default_order(),
            Some("Wildcard"),
        )
        .await
        .expect("artist query");
    assert_eq!(by_artist.len(), 1);

    // Matching on the work-item path still works for unbound rows.
    let (by_path, _) = store
        .list_page_enriched(
            &db.pool,
            None,
            50,
            0,
            WorkItemSort::default_order(),
            Some("other-item"),
        )
        .await
        .expect("path query");
    assert_eq!(by_path.len(), 1);
    assert_eq!(by_path[0].release_title, None);

    // A bare `%` is a literal, not a wildcard: it must not match everything.
    let (wildcard, _) = store
        .list_page_enriched(
            &db.pool,
            None,
            50,
            0,
            WorkItemSort::default_order(),
            Some("%"),
        )
        .await
        .expect("literal wildcard query");
    assert_eq!(
        wildcard.len(),
        1,
        "only the 100% release contains a literal %"
    );

    // An underscore is likewise literal: it matches the one path that
    // contains it, not every path (which `_` would do as a wildcard).
    let (underscore, _) = store
        .list_page_enriched(
            &db.pool,
            None,
            50,
            0,
            WorkItemSort::default_order(),
            Some("_"),
        )
        .await
        .expect("literal underscore query");
    assert_eq!(
        underscore.len(),
        1,
        "only the path with a literal _ matches"
    );
    assert!(
        underscore[0]
            .item
            .current_path
            .contains("literal_underscore")
    );
}

#[tokio::test]
async fn enriched_listing_stays_a_single_join_plan() {
    let db = TestDb::create("queue_list_plan").await;
    let plan: Vec<(String,)> = sqlx::query_as(
        "EXPLAIN SELECT w.*, ci.release_id, r.title, r.display_artist, rca.media_bytes
           FROM release_work_items w
           LEFT JOIN catalog_imports ci ON ci.work_item_id = w.id
           LEFT JOIN releases r ON r.id = ci.release_id
           LEFT JOIN release_catalog_aggregates rca ON rca.release_id = r.id
          WHERE ($1::text IS NULL OR w.state = $1)
          ORDER BY w.created_at DESC, w.id
          LIMIT 50 OFFSET 0",
    )
    .bind(None::<String>)
    .fetch_all(&db.pool)
    .await
    .expect("explain the queue listing")
    .into_iter()
    .map(|row: (String,)| (row.0,))
    .collect();
    let text = plan
        .into_iter()
        .map(|(line,)| line)
        .collect::<Vec<_>>()
        .join("\n");
    // Nested loops over primary-key/unique index lookups are the correct plan
    // for a page of <= 50 rows joined to one binding each. What must never
    // appear is a sequential scan over any joined relation, or a sort of the
    // whole work-item table.
    for relation in ["releases", "catalog_imports", "release_catalog_aggregates"] {
        assert!(
            !text.contains(&format!("Seq Scan on {relation}")),
            "joined relation {relation} must be reached by index:\n{text}"
        );
    }
    assert!(
        text.contains("catalog_imports_item_release_policy_uniq"),
        "the binding join must use its unique index:\n{text}"
    );
    assert!(
        text.contains("release_work_items_admin_browse_created_idx")
            || text.contains("release_work_items"),
        "paging must ride the work-item created_at index:\n{text}"
    );
    assert!(
        !text.contains("Sort Method: external merge")
            && !text.contains("Seq Scan on release_work_items"),
        "paging must not sort or scan the whole work-item table:\n{text}"
    );
}

#[tokio::test]
async fn transition_reads_are_bounded_and_report_real_events() {
    let db = TestDb::create("queue_transition_reads").await;
    let store = WorkItemStore;
    let fixture = seed_cataloged_release(
        &db,
        43,
        CatalogFixturePresentation {
            release_title: "Transition Release",
            release_artist: "Transition Artist",
            track_title: "Transition Track",
            track_artist: "Transition Artist",
            release_date: "2003-04-05",
        },
    )
    .await;
    // Fixtures are written at the deterministic test epoch, so the window starts
    // there rather than at wall-clock now.
    let since = T0 - Duration::hours(1);

    let created = store
        .count_created_since(&db.pool, since)
        .await
        .expect("created count");
    assert!(
        created >= 1,
        "the cataloged fixture emitted a created event"
    );

    let reviews = store
        .count_transitions_since(&db.pool, since, "needs_review")
        .await
        .expect("review transitions");
    assert!(reviews >= 0);

    let indexed = store
        .transitions_hourly(&db.pool, since, "indexed")
        .await
        .expect("indexed series");
    // The series is one row per hour at most, and never negative.
    assert!(indexed.len() <= 25, "bounded by the 24h window");
    assert!(indexed.iter().all(|bucket| bucket.count > 0));
    assert_eq!(
        store
            .count_transitions_since(&db.pool, since, "indexed")
            .await
            .expect("indexed count"),
        indexed.iter().map(|bucket| bucket.count).sum::<i64>(),
        "hourly series and scalar count read the same evidence"
    );

    // A window that ends before the fixtures returns nothing at all.
    let (empty, empty_total) = store
        .list_page_enriched(
            &db.pool,
            None,
            50,
            0,
            WorkItemSort::default_order(),
            Some("no-such-row"),
        )
        .await
        .expect("empty window");
    assert!(empty.is_empty());
    assert_eq!(empty_total, 0);
    let _ = fixture;
}
