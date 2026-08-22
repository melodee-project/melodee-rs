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
        redirect_ids: Vec::new(),
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
        primary_artist_ids: artist_ids.iter().map(|s| s.to_string()).collect(),
        featured_artist_ids: Vec::new(),
        performer_artist_ids: Vec::new(),
        other_artist_ids: Vec::new(),
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
fn release_artist_roles_do_not_bleed_into_one_another() {
    let directory = TempDir::new().unwrap();
    let mut store = SearchStore::create(directory.path()).unwrap();
    for id in ["primary", "featured", "performer", "other"] {
        store
            .upsert_artist(&artist(id, &format!("public-{id}"), id, &[], 1))
            .unwrap();
    }
    let release = ReleaseDoc {
        internal_id: "role-release".to_string(),
        public_id: "public-role-release".to_string(),
        title: "Role Exactness".to_string(),
        sort_title: "Role Exactness".to_string(),
        artist_ids: vec![
            "primary".into(),
            "featured".into(),
            "performer".into(),
            "other".into(),
        ],
        primary_artist_ids: vec!["primary".into()],
        featured_artist_ids: vec!["featured".into()],
        performer_artist_ids: vec!["performer".into()],
        other_artist_ids: vec!["other".into()],
        artist_display: String::new(),
        genre_tags: Vec::new(),
        release_year: None,
        release_date_days: None,
        release_type: "studio".to_string(),
        status: "active".to_string(),
        revision: 1,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    store.upsert_release(&release).unwrap();
    store
        .upsert_track(&track(
            "role-track",
            "role-release",
            "Track Role Exactness",
            &["primary"],
            &["featured"],
            &["performer"],
            &["other"],
            1,
        ))
        .unwrap();
    store.commit().unwrap();

    for (scope, matching_id) in [
        ("primaryArtist", "primary"),
        ("featuredArtist", "featured"),
        ("performer", "performer"),
        ("other", "other"),
    ] {
        for candidate in ["primary", "featured", "performer", "other"] {
            let hits = store
                .search(
                    &parse_and_plan(&format!("{scope}:{candidate}")).unwrap(),
                    10,
                    0,
                )
                .unwrap();
            for expected_entity in ["role-release", "role-track"] {
                assert_eq!(
                    hits.iter().any(|hit| hit.internal_id == expected_entity),
                    candidate == matching_id,
                    "{scope}:{candidate} returned {hits:?}"
                );
            }
        }
    }
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
fn date_queries_union_whole_day_updated_at_with_release_dates() {
    let directory = TempDir::new().unwrap();
    let mut store = SearchStore::create(directory.path()).unwrap();
    let same_day = time::OffsetDateTime::from_unix_timestamp(951_827_696).unwrap();
    let next_day = time::OffsetDateTime::from_unix_timestamp(951_912_000).unwrap();

    let mut same_day_artist = artist("updated-same", "pub-updated-same", "Same Day", &[], 1);
    same_day_artist.updated_at = same_day;
    store.upsert_artist(&same_day_artist).unwrap();
    let mut next_day_artist = artist("updated-next", "pub-updated-next", "Next Day", &[], 1);
    next_day_artist.updated_at = next_day;
    store.upsert_artist(&next_day_artist).unwrap();

    let mut dated_release = release("dated-release", "Dated Release", &[], 1);
    dated_release.release_date_days = Some(11_016);
    store.upsert_release(&dated_release).unwrap();
    let mut dated_track = track(
        "dated-track",
        "dated-release",
        "Dated Track",
        &[],
        &[],
        &[],
        &[],
        1,
    );
    dated_track.release_date_days = Some(11_016);
    store.upsert_track(&dated_track).unwrap();
    store.commit().unwrap();

    let exact = hit_ids(
        &store
            .search(&parse_and_plan("date:2000-02-29").unwrap(), 20, 0)
            .unwrap(),
    );
    assert!(exact.contains(&"updated-same".to_string()));
    assert!(exact.contains(&"dated-release".to_string()));
    assert!(exact.contains(&"dated-track".to_string()));
    assert!(!exact.contains(&"updated-next".to_string()));

    let inclusive = hit_ids(
        &store
            .search(
                &parse_and_plan("date:2000-02-29..2000-03-01").unwrap(),
                20,
                0,
            )
            .unwrap(),
    );
    assert!(inclusive.contains(&"updated-same".to_string()));
    assert!(inclusive.contains(&"updated-next".to_string()));
    assert!(inclusive.contains(&"dated-release".to_string()));
    assert!(inclusive.contains(&"dated-track".to_string()));

    let open_upper = hit_ids(
        &store
            .search(&parse_and_plan("date:..2000-02-29").unwrap(), 20, 0)
            .unwrap(),
    );
    assert!(open_upper.contains(&"updated-same".to_string()));
    assert!(open_upper.contains(&"dated-release".to_string()));
    assert!(open_upper.contains(&"dated-track".to_string()));
    assert!(!open_upper.contains(&"updated-next".to_string()));

    let open_lower = hit_ids(
        &store
            .search(&parse_and_plan("date:2000-03-01..").unwrap(), 20, 0)
            .unwrap(),
    );
    assert!(open_lower.contains(&"updated-next".to_string()));
    assert!(!open_lower.contains(&"updated-same".to_string()));
    assert!(!open_lower.contains(&"dated-release".to_string()));
    assert!(!open_lower.contains(&"dated-track".to_string()));

    let through_day = hit_ids(
        &store
            .search(&parse_and_plan("date<=2000-02-29").unwrap(), 20, 0)
            .unwrap(),
    );
    assert!(through_day.contains(&"updated-same".to_string()));
    assert!(!through_day.contains(&"updated-next".to_string()));
    let after_day = hit_ids(
        &store
            .search(&parse_and_plan("date>2000-02-29").unwrap(), 20, 0)
            .unwrap(),
    );
    assert!(after_day.contains(&"updated-next".to_string()));
    assert!(!after_day.contains(&"updated-same".to_string()));
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

        let direct_symlink_id = uuid::Uuid::now_v7();
        let direct_symlink_path = root
            .create_generation(
                direct_symlink_id,
                time::OffsetDateTime::UNIX_EPOCH,
                melodee_search::SCHEMA_VERSION,
                0,
            )
            .unwrap();
        std::fs::remove_file(direct_symlink_path.join("generation.json")).unwrap();
        std::fs::remove_dir(&direct_symlink_path).unwrap();
        let outside_directory = parent.path().join("must-survive-directory");
        std::fs::create_dir(&outside_directory).unwrap();
        std::fs::write(outside_directory.join("authoritative"), b"catalog truth").unwrap();
        std::os::unix::fs::symlink(&outside_directory, &direct_symlink_path).unwrap();
        assert!(root.remove_generation(direct_symlink_id).is_err());
        assert_eq!(
            std::fs::read(outside_directory.join("authoritative")).unwrap(),
            b"catalog truth"
        );
    }
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[test]
fn generation_removal_rejects_a_leaf_replaced_by_a_symlink_after_root_open() {
    let parent = TempDir::new().unwrap();
    let root = SearchRoot::open(&parent.path().join("search")).unwrap();
    let generation_id = uuid::Uuid::now_v7();
    let generation = root
        .create_generation(
            generation_id,
            time::OffsetDateTime::UNIX_EPOCH,
            melodee_search::SCHEMA_VERSION,
            0,
        )
        .unwrap();
    let outside = parent.path().join("outside-authoritative-data");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("must-survive"), b"authoritative").unwrap();

    let result = root.remove_generation_with_test_hook(generation_id, || {
        std::fs::remove_file(generation.join("generation.json")).unwrap();
        std::fs::remove_dir(&generation).unwrap();
        std::os::unix::fs::symlink(&outside, &generation).unwrap();
    });

    assert!(result.is_err());
    assert_eq!(
        std::fs::read(outside.join("must-survive")).unwrap(),
        b"authoritative"
    );
    assert!(!generation.exists());
    assert!(
        root.path()
            .join(format!(".retiring_gen_{generation_id}"))
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
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

#[cfg(unix)]
#[test]
fn manifest_and_pending_symlinks_are_rejected_without_external_write() {
    let parent = TempDir::new().unwrap();
    let root = SearchRoot::open(&parent.path().join("search")).unwrap();
    let generation_id = uuid::Uuid::now_v7();
    let generation = root
        .create_generation(
            generation_id,
            time::OffsetDateTime::UNIX_EPOCH,
            melodee_search::SCHEMA_VERSION,
            0,
        )
        .unwrap();
    let outside = parent.path().join("outside-authoritative");
    std::fs::write(&outside, b"authoritative").unwrap();

    std::fs::remove_file(generation.join("generation.json")).unwrap();
    std::os::unix::fs::symlink(&outside, generation.join("generation.json")).unwrap();
    assert!(root.read_manifest(generation_id).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"authoritative");
}

#[cfg(unix)]
#[test]
fn manifest_parent_symlink_is_rejected_without_external_read() {
    let parent = TempDir::new().unwrap();
    let root = SearchRoot::open(&parent.path().join("search")).unwrap();
    let generation_id = uuid::Uuid::now_v7();
    let generation = root
        .create_generation(
            generation_id,
            time::OffsetDateTime::UNIX_EPOCH,
            melodee_search::SCHEMA_VERSION,
            0,
        )
        .unwrap();
    let outside = parent.path().join("outside-generation");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("generation.json"), b"authoritative").unwrap();
    std::fs::remove_dir_all(&generation).unwrap();
    std::os::unix::fs::symlink(&outside, &generation).unwrap();

    assert!(root.read_manifest(generation_id).is_err());
    assert_eq!(
        std::fs::read(outside.join("generation.json")).unwrap(),
        b"authoritative"
    );
}

#[cfg(unix)]
#[test]
fn sealing_rejects_a_pending_symlink_without_writing_its_target() {
    let parent = TempDir::new().unwrap();
    let root = SearchRoot::open(&parent.path().join("search")).unwrap();
    let generation_id = uuid::Uuid::now_v7();
    let generation = root
        .create_generation(
            generation_id,
            time::OffsetDateTime::UNIX_EPOCH,
            melodee_search::SCHEMA_VERSION,
            0,
        )
        .unwrap();
    let outside = parent.path().join("outside-authoritative");
    std::fs::write(&outside, b"authoritative").unwrap();
    let pending = generation.join("generation.json.pending");
    std::os::unix::fs::symlink(&outside, &pending).unwrap();

    let manifest = root.read_manifest(generation_id).unwrap();
    assert!(
        root.seal_generation(
            manifest,
            melodee_persistence::SearchDocumentCounts {
                artists: 1,
                releases: 2,
                tracks: 3,
            },
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"authoritative");
    assert!(pending.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn sealing_replaces_a_stale_regular_pending_file_and_updates_counts() {
    let directory = TempDir::new().unwrap();
    let root = SearchRoot::open(directory.path()).unwrap();
    let generation_id = uuid::Uuid::now_v7();
    let generation = root
        .create_generation(
            generation_id,
            time::OffsetDateTime::UNIX_EPOCH,
            melodee_search::SCHEMA_VERSION,
            0,
        )
        .unwrap();
    std::fs::write(generation.join("generation.json.pending"), b"stale").unwrap();

    let manifest = root.read_manifest(generation_id).unwrap();
    let hash = root
        .seal_generation(
            manifest,
            melodee_persistence::SearchDocumentCounts {
                artists: 1,
                releases: 2,
                tracks: 3,
            },
        )
        .unwrap();
    let sealed = root.read_manifest(generation_id).unwrap();
    assert_eq!(sealed.document_counts.artists, 1);
    assert_eq!(sealed.document_counts.releases, 2);
    assert_eq!(sealed.document_counts.tracks, 3);
    assert!(!generation.join("generation.json.pending").exists());
    assert_eq!(
        hash,
        blake3::hash(&serde_json::to_vec(&sealed).unwrap())
            .to_hex()
            .to_string()
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
