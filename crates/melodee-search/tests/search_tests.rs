//! In-memory Tantivy search correctness tests (acceptance A1-A3, A6, A7, A20-A23).
//!
//! These build a generation in a temp dir, index catalog-shaped documents, and
//! verify MQL-driven search behavior without a database.

#![allow(clippy::too_many_arguments, clippy::unwrap_used)]

use melodee_mql::parse_and_plan;
use melodee_search::{
    ArtistDoc, ReleaseDoc, SearchRoot, SearchStore, TrackDoc, reject_root_overlap,
};
use tempfile::TempDir;

fn artist(id: &str, public: &str, name: &str, aliases: &[&str], rev: i64) -> ArtistDoc {
    ArtistDoc {
        internal_id: id.to_string(),
        public_id: public.to_string(),
        display_name: name.to_string(),
        sort_name: name.to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        disambiguation: None,
        artist_type: "person".to_string(),
        status: "verified".to_string(),
        begin_year: None,
        end_year: None,
        revision: rev,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

fn track(
    id: &str,
    release_id: &str,
    title: &str,
    primary: &[&str],
    featured: &[&str],
    performer: &[&str],
    other: &[&str],
    rev: i64,
) -> TrackDoc {
    TrackDoc {
        internal_id: id.to_string(),
        public_id: format!("pub-{id}"),
        release_id: release_id.to_string(),
        title: title.to_string(),
        sort_title: title.to_string(),
        primary_artist_ids: primary.iter().map(|s| s.to_string()).collect(),
        featured_artist_ids: featured.iter().map(|s| s.to_string()).collect(),
        performer_artist_ids: performer.iter().map(|s| s.to_string()).collect(),
        other_artist_ids: other.iter().map(|s| s.to_string()).collect(),
        participant_artist_ids: primary
            .iter()
            .chain(featured)
            .chain(performer)
            .chain(other)
            .map(|s| s.to_string())
            .collect(),
        display_credits: String::new(),
        genre_tags: vec![],
        track_number: Some(1),
        duration_ms: Some(180000),
        release_year: Some(2000),
        release_date_days: Some(11016),
        revision: rev,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

fn release(id: &str, title: &str, artist_ids: &[&str], rev: i64) -> ReleaseDoc {
    ReleaseDoc {
        internal_id: id.to_string(),
        public_id: format!("pub-{id}"),
        title: title.to_string(),
        sort_title: title.to_string(),
        artist_ids: artist_ids.iter().map(|s| s.to_string()).collect(),
        artist_display: String::new(),
        genre_tags: vec![],
        release_year: Some(2000),
        release_date_days: Some(11016),
        release_type: "album".to_string(),
        status: "active".to_string(),
        revision: rev,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

fn populated() -> (TempDir, SearchStore) {
    let dir = TempDir::new().unwrap();
    let mut store = SearchStore::create(dir.path()).unwrap();
    // Two same-named distinct artists.
    store
        .upsert_artist(&artist("a1", "pa1", "The Beatles", &["Beatles"], 1))
        .unwrap();
    store
        .upsert_artist(&artist("a2", "pa2", "The Beatles", &[], 1))
        .unwrap();
    // Distinct artist with diacritics.
    store
        .upsert_artist(&artist("a3", "pa3", "Björk", &["Bjork"], 1))
        .unwrap();
    // Tracks with structured participation.
    store
        .upsert_track(&track(
            "t1",
            "r1",
            "Hey Jude",
            &["a1"],
            &["a2"],
            &["a3"],
            &[],
            1,
        ))
        .unwrap();
    store
        .upsert_track(&track(
            "t2",
            "r1",
            "Let It Be",
            &["a2"],
            &[],
            &["a3"],
            &[],
            1,
        ))
        .unwrap();
    store
        .upsert_release(&release("r1", "Let It Be", &["a1", "a2"], 1))
        .unwrap();
    store.commit().unwrap();
    (dir, store)
}

fn hit_ids(hits: &[melodee_search::SearchHit]) -> Vec<String> {
    hits.iter().map(|h| h.internal_id.clone()).collect()
}

#[test]
fn same_name_artists_remain_distinct() {
    let (_d, store) = populated();
    let plan = parse_and_plan("artist:\"The Beatles\"").unwrap();
    let hits = store.search(&plan, 50, 0).unwrap();
    let ids = hit_ids(&hits);
    assert!(ids.contains(&"a1".to_string()), "a1 missing: {ids:?}");
    assert!(ids.contains(&"a2".to_string()), "a2 missing: {ids:?}");
    // artist: is a cross-participation query: it also returns the release and
    // tracks the artist participates in, but the two same-named artists must
    // both be present and not collapsed.
    assert!(ids.len() >= 2, "expected >=2 hits across kinds: {ids:?}");
}

#[test]
fn aliases_are_searchable() {
    let (_d, store) = populated();
    let plan = parse_and_plan("alias:Beatles").unwrap();
    let ids = hit_ids(&store.search(&plan, 50, 0).unwrap());
    assert!(ids.contains(&"a1".to_string()));
}

#[test]
fn primary_featured_performer_participation() {
    let (_d, store) = populated();
    // primaryArtist:a1 -> t1 only.
    let plan = parse_and_plan("primaryArtist:a1").unwrap();
    let ids = hit_ids(&store.search(&plan, 50, 0).unwrap());
    assert!(
        ids.contains(&"t1".to_string()),
        "primary a1 missing: {ids:?}"
    );
    assert!(
        !ids.contains(&"t2".to_string()),
        "t2 should not match primary a1"
    );

    // featuredArtist:a2 -> t1 only.
    let plan = parse_and_plan("featuredArtist:a2").unwrap();
    let ids = hit_ids(&store.search(&plan, 50, 0).unwrap());
    assert!(
        ids.contains(&"t1".to_string()),
        "featured a2 missing: {ids:?}"
    );

    // performer:a3 -> both t1 and t2.
    let plan = parse_and_plan("performer:a3").unwrap();
    let ids = hit_ids(&store.search(&plan, 50, 0).unwrap());
    assert!(ids.contains(&"t1".to_string()));
    assert!(ids.contains(&"t2".to_string()));

    // Cross artist:a1 -> t1 (via participant ids).
    let plan = parse_and_plan("artist:a1").unwrap();
    let ids = hit_ids(&store.search(&plan, 50, 0).unwrap());
    assert!(
        ids.contains(&"t1".to_string()),
        "cross artist a1 missing: {ids:?}"
    );
}

#[test]
fn diacritics_fold_to_ascii() {
    let (_d, store) = populated();
    // ASCII-folded query "bjork" should match "Björk".
    let plan = parse_and_plan("artist:bjork").unwrap();
    let ids = hit_ids(&store.search(&plan, 50, 0).unwrap());
    assert!(
        ids.contains(&"a3".to_string()),
        "diacritic fold failed: {ids:?}"
    );
}

#[test]
fn multilingual_unicode_tokens_remain_searchable() {
    let directory = TempDir::new().unwrap();
    let mut store = SearchStore::create(directory.path()).unwrap();
    store
        .upsert_artist(&artist("a-jp", "pa-jp", "東京事変", &[], 1))
        .unwrap();
    store
        .upsert_artist(&artist("a-is", "pa-is", "Sigur Rós", &[], 1))
        .unwrap();
    store.commit().unwrap();
    assert_eq!(
        store
            .search(&parse_and_plan("artist:東京事変").unwrap(), 10, 0)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .search(&parse_and_plan("artist:ros").unwrap(), 10, 0)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn deterministic_pagination() {
    let dir = TempDir::new().unwrap();
    let mut store = SearchStore::create(dir.path()).unwrap();
    for i in 0..100 {
        let name = format!("Artist {i:03}");
        store
            .upsert_artist(&artist(&format!("x{i}"), &format!("px{i}"), &name, &[], 1))
            .unwrap();
    }
    store.commit().unwrap();
    let plan = parse_and_plan("artist:Artist").unwrap();
    let first = store.search(&plan, 10, 0).unwrap();
    let second = store.search(&plan, 10, 10).unwrap();
    assert_eq!(first.len(), 10);
    assert_eq!(second.len(), 10);
    // No overlap between pages.
    let overlap = first.iter().filter(|h| second.contains(h)).count();
    assert_eq!(overlap, 0, "pagination overlapped");
}

#[test]
fn date_fields_use_tantivy_dates_and_preserve_range_semantics() {
    let (_directory, store) = populated();
    let exact = store
        .search(&parse_and_plan("date:2000-02-29").unwrap(), 20, 0)
        .unwrap();
    assert_eq!(exact.len(), 3);
    let range = store
        .search(
            &parse_and_plan("date:1999-01-01..2001-01-01").unwrap(),
            20,
            0,
        )
        .unwrap();
    assert_eq!(range.len(), 3);
}

#[test]
fn adr_declared_sort_identity_numeric_and_date_fields_are_fast() {
    let schema = melodee_search::build_schema();
    for field in [
        schema.doc_id,
        schema.revision,
        schema.updated_at,
        schema.a_internal_id,
        schema.a_public_id,
        schema.a_sort_name,
        schema.a_begin_year,
        schema.a_end_year,
        schema.r_internal_id,
        schema.r_public_id,
        schema.r_sort_title,
        schema.r_release_year,
        schema.r_release_date,
        schema.t_internal_id,
        schema.t_public_id,
        schema.t_sort_title,
        schema.t_track_number,
        schema.t_duration_ms,
        schema.t_release_year,
        schema.t_release_date,
    ] {
        assert!(
            schema.schema.get_field_entry(field).is_fast(),
            "{} must be a fast field",
            schema.schema.get_field_entry(field).name()
        );
    }
    assert!(matches!(
        schema
            .schema
            .get_field_entry(schema.r_release_date)
            .field_type(),
        tantivy::schema::FieldType::Date(_)
    ));
    assert!(matches!(
        schema
            .schema
            .get_field_entry(schema.t_release_date)
            .field_type(),
        tantivy::schema::FieldType::Date(_)
    ));
}

#[test]
fn revision_fencing_skips_stale() {
    let dir = TempDir::new().unwrap();
    let mut store = SearchStore::create(dir.path()).unwrap();
    store
        .upsert_artist(&artist("a1", "pa1", "Name v2", &[], 5))
        .unwrap();
    store.commit().unwrap();
    // A stale revision (<= existing) must be ignored.
    store
        .upsert_artist(&artist("a1", "pa1", "Name v1", &[], 1))
        .unwrap();
    store.commit().unwrap();
    let plan = parse_and_plan("artist:\"Name v2\"").unwrap();
    assert_eq!(store.search(&plan, 10, 0).unwrap().len(), 1);
    let plan2 = parse_and_plan("artist:\"Name v1\"").unwrap();
    assert_eq!(store.search(&plan2, 10, 0).unwrap().len(), 0);
}

#[test]
fn delete_removes_only_projection() {
    let dir = TempDir::new().unwrap();
    let mut store = SearchStore::create(dir.path()).unwrap();
    store
        .upsert_artist(&artist("a1", "pa1", "Doomed", &[], 1))
        .unwrap();
    store.commit().unwrap();
    store.delete("artist", "a1", 2).unwrap();
    store.commit().unwrap();
    let plan = parse_and_plan("artist:Doomed").unwrap();
    assert_eq!(store.search(&plan, 10, 0).unwrap().len(), 0);
}

#[test]
fn mql_boolean_and_numeric_range() {
    let (_d, store) = populated();
    let plan = parse_and_plan("artist:\"The Beatles\" AND year>=2000").unwrap();
    let hits = store.search(&plan, 50, 0).unwrap();
    assert!(!hits.is_empty());
}

#[test]
fn negation_and_not_equal_match_the_complement() {
    let (_directory, store) = populated();
    let not_old = parse_and_plan("year!=1999").unwrap();
    assert_eq!(store.search(&not_old, 50, 0).unwrap().len(), 3);

    let not_bjork = parse_and_plan("NOT artist:Björk").unwrap();
    let ids = hit_ids(&store.search(&not_bjork, 50, 0).unwrap());
    assert!(!ids.contains(&"a3".to_string()));
    assert!(ids.contains(&"a1".to_string()));
}

#[test]
fn stale_upsert_cannot_resurrect_a_tombstone() {
    let directory = TempDir::new().unwrap();
    let mut store = SearchStore::create(directory.path()).unwrap();
    store
        .upsert_artist(&artist("a1", "pa1", "Deleted", &[], 1))
        .unwrap();
    store.commit().unwrap();
    store.delete("artist", "a1", 3).unwrap();
    store.commit().unwrap();
    store
        .upsert_artist(&artist("a1", "pa1", "Resurrected", &[], 2))
        .unwrap();
    store.commit().unwrap();
    assert!(
        store
            .search(&parse_and_plan("artist:Resurrected").unwrap(), 10, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pending_revisions_are_fenced_before_commit() {
    let directory = TempDir::new().unwrap();
    let mut store = SearchStore::create(directory.path()).unwrap();
    store
        .upsert_artist(&artist("a1", "pa1", "Newest", &[], 5))
        .unwrap();
    store
        .upsert_artist(&artist("a1", "pa1", "Stale", &[], 4))
        .unwrap();
    store.commit().unwrap();
    assert_eq!(
        store
            .search(&parse_and_plan("artist:Newest").unwrap(), 10, 0)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .search(&parse_and_plan("artist:Stale").unwrap(), 10, 0)
            .unwrap()
            .is_empty()
    );
}

#[cfg(feature = "test-support")]
#[test]
fn failed_commit_retains_pending_revision_fencing_until_retry() {
    let directory = TempDir::new().unwrap();
    let mut store = SearchStore::create(directory.path()).unwrap();
    let document = artist(
        "commit-retry",
        "public-commit-retry",
        "Commit Retry",
        &[],
        1,
    );
    assert!(store.upsert_artist(&document).unwrap());
    melodee_search::store::set_commit_failure_for_test(true);
    assert!(store.commit().is_err());
    assert!(
        !store.upsert_artist(&document).unwrap(),
        "the buffered revision remains fenced after a failed commit"
    );
    melodee_search::store::set_commit_failure_for_test(false);
    store.commit().unwrap();
    assert_eq!(
        store
            .search(&parse_and_plan("artist:\"Commit Retry\"").unwrap(), 10, 0)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn autocomplete_is_prefix_bounded_and_deterministic() {
    let (_directory, store) = populated();
    assert_eq!(
        store.autocomplete("artist", "Bjo", 10).unwrap(),
        vec!["Björk".to_string()]
    );
    assert!(store.autocomplete("artist", "", 10).is_err());
    assert!(store.autocomplete("artist", "B", 201).is_err());
}

#[test]
fn owned_generation_removal_rejects_symlinks_and_root_overlap() {
    let parent = TempDir::new().unwrap();
    let search_path = parent.path().join("search");
    let storage_path = parent.path().join("storage");
    std::fs::create_dir(&storage_path).unwrap();
    let root = SearchRoot::open(&search_path).unwrap();
    assert!(reject_root_overlap(root.path(), std::slice::from_ref(&storage_path)).is_ok());
    assert!(
        reject_root_overlap(
            root.path(),
            std::slice::from_ref(&parent.path().to_path_buf())
        )
        .is_err()
    );

    let generation_id = uuid::Uuid::now_v7();
    let generation = root
        .create_generation(
            generation_id,
            time::OffsetDateTime::UNIX_EPOCH,
            melodee_search::SCHEMA_VERSION,
            0,
        )
        .unwrap();
    let outside = parent.path().join("must-survive");
    std::fs::write(&outside, b"authoritative").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, generation.join("escape")).unwrap();
        assert!(root.remove_generation(generation_id).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"authoritative");
    }
}

#[test]
fn nonempty_unowned_root_is_rejected() {
    let directory = TempDir::new().unwrap();
    std::fs::write(directory.path().join("foreign"), b"not melodee").unwrap();
    assert!(SearchRoot::open(directory.path()).is_err());
    assert!(directory.path().join("foreign").exists());
    assert!(SearchRoot::open(&directory.path().join("owned").join("..").join("escape")).is_err());
}

#[test]
fn generation_identity_and_manifest_facts_are_fail_closed() {
    let directory = TempDir::new().unwrap();
    let root = SearchRoot::open(directory.path()).unwrap();
    assert!(
        root.create_generation(
            uuid::Uuid::new_v4(),
            time::OffsetDateTime::UNIX_EPOCH,
            melodee_search::SCHEMA_VERSION,
            0,
        )
        .is_err()
    );
    let generation_id = uuid::Uuid::now_v7();
    root.create_generation(
        generation_id,
        time::OffsetDateTime::UNIX_EPOCH,
        melodee_search::SCHEMA_VERSION,
        0,
    )
    .unwrap();
    let mut forged = root.read_manifest(generation_id).unwrap();
    forged.source_high_water_mark = 1;
    assert!(
        root.seal_generation(
            forged,
            melodee_persistence::SearchDocumentCounts {
                artists: 0,
                releases: 0,
                tracks: 0,
            },
        )
        .is_err()
    );
}

#[test]
fn invalid_regex_is_reported_instead_of_becoming_an_empty_query() {
    let (_directory, store) = populated();
    let plan = parse_and_plan("title:/[/").unwrap();
    assert!(store.search(&plan, 10, 0).is_err());
}

#[test]
fn readonly_queries_do_not_contend_with_the_projection_writer() {
    let (directory, writer) = populated();
    assert!(
        SearchStore::open(directory.path()).is_err(),
        "Tantivy must fence a second process writer for the generation"
    );
    let reader = SearchStore::open_readonly(directory.path()).unwrap();
    let plan = parse_and_plan("artist:Björk").unwrap();
    assert_eq!(reader.search(&plan, 10, 0).unwrap().len(), 3);
    drop(reader);

    // The original writer remains usable after the independent query reader.
    assert_eq!(writer.stats().unwrap().artists, 3);
}
