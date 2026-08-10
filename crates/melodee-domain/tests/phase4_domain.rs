#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use melodee_domain::{
    Artist, ArtistAlias, ArtistAliasId, ArtistAliasType, ArtistId, ArtistMatchDecision,
    ArtistMatchDecisionId, ArtistMatchEvidence, ArtistMatchEvidenceId, ArtistMatchOutcome,
    ArtistMerge, ArtistMergeId, ArtistNegativeAssertion, ArtistNegativeAssertionId, ArtistPublicId,
    ArtistRelationship, ArtistRelationshipId, ArtistRelationshipKind, ArtistSplit, ArtistSplitId,
    ArtistStatus, ArtistTrackAssociation, ArtistType, AudioIdentity, AudioIdentityDecisionId,
    AudioIdentityEvidence, AudioIdentityEvidenceId, AudioIdentityEvidenceKind, AudioIdentityId,
    AudioIdentityMatchDecision, AudioIdentityMatchOutcome, AudioIdentityStatus, CatalogImportId,
    CatalogSemanticOverlay, CatalogSemanticOverlayId, Confidence, ContributionId, ContributionRole,
    DateRange, DecisionAuthority, DisplayArtist, DisplayArtistOwner, DomainInvariantError,
    EvidenceKind, EvidencePolarity, GroupMembership, GroupMembershipId, MediaAsset, MediaAssetId,
    MediaAssetRole, MembershipRole, NewArtist, NewArtistMatchDecision,
    NewAudioIdentityMatchDecision, NewCatalogSemanticOverlay, NewTrack, ObservedArtist,
    ObservedArtistId, OverlayArtistResolution, OverlayAudioIdentityLink, OverlayDisplayArtist,
    OverlayReferences, ParsedArtistCandidate, ParticipationKind, Release, ReleaseArtist,
    ReleaseArtistId, ReleaseId, ReleasePublicId, SplitAssignment, Track, TrackArtist,
    TrackArtistId, TrackId, TrackMediaAsset, validate_release_artist_order,
    validate_release_track_ordinals, validate_track_artist_order,
};
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

fn confidence(value: u16) -> Confidence {
    Confidence::new(value).unwrap()
}

fn artist(id: ArtistId, public_api_id: ArtistPublicId, name: &str) -> Artist {
    Artist::new(NewArtist {
        id,
        public_api_id,
        canonical_name: name.to_owned(),
        normalized_name: name.to_lowercase(),
        sort_name: name.to_owned(),
        artist_type: ArtistType::Person,
        disambiguation: None,
        active_dates: None,
        status: ArtistStatus::Provisional,
        created_at: OffsetDateTime::UNIX_EPOCH,
    })
    .unwrap()
}

#[test]
fn same_names_and_legacy_public_ids_do_not_define_artist_identity() {
    let legacy_public = Uuid::parse_str("813c11e0-61cd-5204-ae0f-ce68386eacec").unwrap();
    let first = artist(
        ArtistId::new(),
        ArtistPublicId::from_uuid(legacy_public),
        "Dune",
    );
    let second = artist(ArtistId::new(), ArtistId::new().into(), "Dune");

    assert_ne!(first.id(), second.id());
    assert_eq!(first.canonical_name(), second.canonical_name());
    assert_eq!(first.normalized_name(), second.normalized_name());
    assert_eq!(first.public_api_id().into_uuid(), legacy_public);
    assert_ne!(first.id().into_uuid(), first.public_api_id().into_uuid());
}

#[test]
fn alias_values_are_not_globally_unique_identity_keys() {
    let value = "The Artist".to_owned();
    let first = ArtistAlias::new(ArtistAlias {
        id: ArtistAliasId::new(),
        artist_id: ArtistId::new(),
        value: value.clone(),
        normalized_value: "the artist".to_owned(),
        alias_type: ArtistAliasType::Search,
        locale: Some("en".to_owned()),
        evidence_source: "manual".to_owned(),
        confidence: confidence(10_000),
        locked: true,
    })
    .unwrap();
    let second = ArtistAlias::new(ArtistAlias {
        id: ArtistAliasId::new(),
        artist_id: ArtistId::new(),
        value,
        normalized_value: "the artist".to_owned(),
        alias_type: ArtistAliasType::Search,
        locale: Some("en".to_owned()),
        evidence_source: "manual".to_owned(),
        confidence: confidence(10_000),
        locked: true,
    })
    .unwrap();

    assert_eq!(first.normalized_value, second.normalized_value);
    assert_ne!(first.artist_id, second.artist_id);
}

#[test]
fn relationships_and_membership_reject_self_links_and_infer_only_known_dates() {
    let artist_id = ArtistId::new();
    let relationship = ArtistRelationship {
        id: ArtistRelationshipId::new(),
        subject_artist_id: artist_id,
        object_artist_id: artist_id,
        kind: ArtistRelationshipKind::new("collaborated_with".to_owned()).unwrap(),
        active_dates: None,
        confidence: confidence(8_000),
        evidence_source: "liner notes".to_owned(),
    };
    assert_eq!(
        ArtistRelationship::new(relationship),
        Err(DomainInvariantError::SelfArtistRelationship)
    );

    let start = Date::from_calendar_date(1980, Month::January, 1).unwrap();
    let end = Date::from_calendar_date(1985, Month::December, 31).unwrap();
    let track_id = TrackId::new();
    let membership = GroupMembership::new(GroupMembership {
        id: GroupMembershipId::new(),
        member_artist_id: ArtistId::new(),
        group_artist_id: ArtistId::new(),
        active_dates: DateRange::new(Some(start), Some(end)).unwrap(),
        role: MembershipRole::new("member".to_owned()).unwrap(),
        confidence: confidence(9_000),
        evidence_source: "manual research".to_owned(),
        excluded_tracks: BTreeSet::new(),
    })
    .unwrap();

    assert!(membership.supports_inference(track_id, Some(start)));
    assert!(membership.supports_inference(track_id, Some(end)));
    assert!(!membership.supports_inference(track_id, None));
    assert!(!membership.supports_inference(
        track_id,
        Some(Date::from_calendar_date(1986, Month::January, 1).unwrap())
    ));
}

#[test]
fn date_range_containment_property_is_inclusive_and_bounded() {
    let start = Date::from_calendar_date(2000, Month::January, 10).unwrap();
    let end = Date::from_calendar_date(2000, Month::January, 20).unwrap();
    let range = DateRange::new(Some(start), Some(end)).unwrap();

    for day in 1..=31 {
        let date = Date::from_calendar_date(2000, Month::January, day).unwrap();
        assert_eq!(range.contains(date), (10..=20).contains(&day));
    }
    assert_eq!(
        DateRange::new(Some(end), Some(start)),
        Err(DomainInvariantError::InvalidDateRange)
    );
}

#[test]
fn unbounded_membership_still_requires_a_known_track_date() {
    let unbounded = DateRange::new(None, None).unwrap();
    let date = Date::from_calendar_date(2000, Month::January, 1).unwrap();
    assert!(unbounded.contains_known(Some(date)));
    assert!(!unbounded.contains_known(None));
}

#[test]
fn display_artist_is_exact_and_structured_billing_is_independent() {
    let display = DisplayArtist::new("Somebody feat. Colin Hay".to_owned()).unwrap();
    let track_id = TrackId::new();
    let primary = TrackArtist::new(TrackArtist {
        id: TrackArtistId::new(),
        track_id,
        artist_id: ArtistId::new(),
        role: melodee_domain::PresentationRole::Primary,
        presentation_order: 1,
        join_phrase: " feat. ".to_owned(),
    })
    .unwrap();
    let featured = TrackArtist::new(TrackArtist {
        id: TrackArtistId::new(),
        track_id,
        artist_id: ArtistId::new(),
        role: melodee_domain::PresentationRole::Featured,
        presentation_order: 2,
        join_phrase: String::new(),
    })
    .unwrap();

    assert_eq!(display.as_str(), "Somebody feat. Colin Hay");
    assert_eq!(primary.join_phrase, " feat. ");
    assert!(validate_track_artist_order(&[primary, featured]).is_ok());
}

#[test]
fn presentation_order_must_be_unique_per_owner() {
    let release_id = ReleaseId::new();
    let duplicate_release_order = [
        ReleaseArtist::new(ReleaseArtist {
            id: ReleaseArtistId::new(),
            release_id,
            artist_id: ArtistId::new(),
            role: melodee_domain::PresentationRole::Primary,
            presentation_order: 1,
            join_phrase: String::new(),
        })
        .unwrap(),
        ReleaseArtist::new(ReleaseArtist {
            id: ReleaseArtistId::new(),
            release_id,
            artist_id: ArtistId::new(),
            role: melodee_domain::PresentationRole::Featured,
            presentation_order: 1,
            join_phrase: String::new(),
        })
        .unwrap(),
    ];
    assert_eq!(
        validate_release_artist_order(&duplicate_release_order),
        Err(DomainInvariantError::InvalidOrdinal)
    );
}

#[test]
fn parser_candidates_cannot_claim_artist_identity() {
    let parsed = ParsedArtistCandidate::new("Colin Hay".to_owned(), String::new()).unwrap();
    assert_eq!(parsed.raw_value, "Colin Hay");
    // ParsedArtistCandidate intentionally exposes no ArtistId field or conversion.
}

#[test]
fn default_participation_includes_only_performance_roles() {
    let performance = [
        ContributionRole::PrimaryArtist,
        ContributionRole::FeaturedArtist,
        ContributionRole::GuestArtist,
        ContributionRole::Performer,
        ContributionRole::BandMember,
        ContributionRole::Vocalist,
        ContributionRole::Instrumentalist,
    ];
    let non_performance = [
        ContributionRole::Composer,
        ContributionRole::Lyricist,
        ContributionRole::Producer,
        ContributionRole::Remixer,
        ContributionRole::Engineer,
        ContributionRole::Other,
    ];

    for role in performance {
        let association = ArtistTrackAssociation {
            artist_id: ArtistId::new(),
            track_id: TrackId::new(),
            role,
            participation: ParticipationKind::Direct,
        };
        assert!(association.included_in_default_appears_on());
    }
    for role in non_performance {
        assert!(!role.is_performance_role());
    }
}

fn evidence(
    observed_artist_id: ObservedArtistId,
    candidate_artist_id: ArtistId,
    kind: EvidenceKind,
    polarity: EvidencePolarity,
) -> ArtistMatchEvidence {
    ArtistMatchEvidence::new(ArtistMatchEvidence {
        id: ArtistMatchEvidenceId::new(),
        observed_artist_id,
        candidate_artist_id,
        kind,
        polarity,
        confidence: confidence(8_000),
        source: "test".to_owned(),
    })
    .unwrap()
}

fn match_input(
    observed_artist_id: ObservedArtistId,
    artist_id: ArtistId,
    authority: DecisionAuthority,
    locked: bool,
) -> NewArtistMatchDecision {
    NewArtistMatchDecision {
        id: ArtistMatchDecisionId::new(),
        observed_artist_id,
        artist_id,
        outcome: ArtistMatchOutcome::Confirmed,
        authority,
        locked,
        reason: "identity evidence".to_owned(),
        decided_at: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn automatic_confirmation_rejects_name_only_and_conflicting_evidence() {
    let observation = ObservedArtistId::new();
    let candidate = ArtistId::new();
    let automatic = DecisionAuthority::Automated {
        resolver_version: "resolver-v1".to_owned(),
    };
    let name = evidence(
        observation,
        candidate,
        EvidenceKind::NameSimilarity,
        EvidencePolarity::Supports,
    );
    assert_eq!(
        ArtistMatchDecision::new(
            match_input(observation, candidate, automatic.clone(), false),
            &[name]
        ),
        Err(DomainInvariantError::AutomaticNameOnlyMatch)
    );

    let external = evidence(
        observation,
        candidate,
        EvidenceKind::EmbeddedExternalId,
        EvidencePolarity::Supports,
    );
    let conflict = evidence(
        observation,
        candidate,
        EvidenceKind::ReleaseContext,
        EvidencePolarity::Conflicts,
    );
    assert_eq!(
        ArtistMatchDecision::new(
            match_input(observation, candidate, automatic, false),
            &[external, conflict]
        ),
        Err(DomainInvariantError::ConflictingMatchEvidence)
    );
}

#[test]
fn locked_manual_decision_and_negative_assertion_outrank_refresh() {
    let observation = ObservedArtistId::new();
    let candidate = ArtistId::new();
    let admin = DecisionAuthority::Administrator {
        actor: "curator".to_owned(),
    };
    let decision = ArtistMatchDecision::new(
        match_input(observation, candidate, admin.clone(), true),
        &[],
    )
    .unwrap();
    assert!(
        !decision.can_be_superseded_by(&DecisionAuthority::Automated {
            resolver_version: "resolver-v2".to_owned(),
        })
    );
    assert!(decision.can_be_superseded_by(&admin));

    let negative = ArtistNegativeAssertion::new(ArtistNegativeAssertion {
        id: ArtistNegativeAssertionId::new(),
        observed_artist_id: observation,
        rejected_artist_id: candidate,
        authority: DecisionAuthority::Administrator {
            actor: "curator".to_owned(),
        },
        locked: true,
        reason: "wrong same-name Artist".to_owned(),
        decided_at: OffsetDateTime::UNIX_EPOCH,
    })
    .unwrap();
    assert!(negative.prevents_suggestion(observation, candidate));
}

fn track(
    id: TrackId,
    release_id: ReleaseId,
    audio_identity_id: AudioIdentityId,
    media_asset_id: MediaAssetId,
    ordinal: u32,
) -> Track {
    Track::new(NewTrack {
        id,
        public_api_id: id.into(),
        release_id,
        audio_identity_id,
        ordinal,
        title: format!("Track {ordinal}"),
        display_artist: DisplayArtist::new("Artist".to_owned()).unwrap(),
        observed_disc_number: Some(1),
        observed_track_number: Some(ordinal),
        media_assets: vec![
            TrackMediaAsset::new(TrackMediaAsset {
                track_id: id,
                media_asset_id,
                role: MediaAssetRole::Playable,
                ordinal: 1,
            })
            .unwrap(),
        ],
    })
    .unwrap()
}

#[test]
fn track_media_and_audio_identity_are_distinct_and_shareable() {
    let release_id = ReleaseId::new();
    let audio_identity_id = AudioIdentityId::new();
    let media_asset_id = MediaAssetId::new();
    let hash = "a".repeat(64);
    let first_asset = MediaAsset::new(
        media_asset_id,
        format!("media/{hash}.mp3"),
        hash.clone(),
        42,
    )
    .unwrap();
    let second_asset = MediaAsset::new(
        MediaAssetId::new(),
        format!("media/copy/{hash}.mp3"),
        hash,
        42,
    )
    .unwrap();
    let first = track(
        TrackId::new(),
        release_id,
        audio_identity_id,
        media_asset_id,
        1,
    );
    let second = track(
        TrackId::new(),
        release_id,
        audio_identity_id,
        media_asset_id,
        2,
    );
    let audio = AudioIdentity::new(audio_identity_id, AudioIdentityStatus::Provisional).unwrap();

    assert_ne!(first.id(), second.id());
    assert_eq!(first.audio_identity_id(), second.audio_identity_id());
    assert_eq!(
        first.media_assets()[0].media_asset_id,
        second.media_assets()[0].media_asset_id
    );
    assert_ne!(first_asset.id(), second_asset.id());
    assert_eq!(first_asset.byte_hash(), second_asset.byte_hash());
    assert_eq!(audio.id(), audio_identity_id);
}

#[test]
fn an_individual_fingerprint_signal_cannot_merge_audio_identities() {
    let source = AudioIdentityId::new();
    let candidate = AudioIdentityId::new();
    let fingerprint = AudioIdentityEvidence::new(AudioIdentityEvidence {
        id: AudioIdentityEvidenceId::new(),
        source_audio_identity_id: source,
        candidate_audio_identity_id: candidate,
        kind: AudioIdentityEvidenceKind::AcousticFingerprint,
        polarity: EvidencePolarity::Supports,
        confidence: confidence(9_500),
        source: "fpcalc".to_owned(),
    })
    .unwrap();
    let decision = NewAudioIdentityMatchDecision {
        id: AudioIdentityDecisionId::new(),
        source_audio_identity_id: source,
        candidate_audio_identity_id: candidate,
        outcome: AudioIdentityMatchOutcome::Confirmed,
        authority: DecisionAuthority::Automated {
            resolver_version: "audio-resolver-v1".to_owned(),
        },
        locked: false,
        reason: "candidate evidence".to_owned(),
        decided_at: OffsetDateTime::UNIX_EPOCH,
    };
    assert_eq!(
        AudioIdentityMatchDecision::new(decision.clone(), std::slice::from_ref(&fingerprint)),
        Err(DomainInvariantError::InsufficientAudioIdentityEvidence)
    );

    let duration = AudioIdentityEvidence::new(AudioIdentityEvidence {
        id: AudioIdentityEvidenceId::new(),
        source_audio_identity_id: source,
        candidate_audio_identity_id: candidate,
        kind: AudioIdentityEvidenceKind::Duration,
        polarity: EvidencePolarity::Supports,
        confidence: confidence(9_000),
        source: "ffprobe".to_owned(),
    })
    .unwrap();
    assert!(AudioIdentityMatchDecision::new(decision, &[fingerprint, duration]).is_ok());
}

#[test]
fn invariant_constructors_reject_version_four_internal_ids() {
    let v4 = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let invalid_artist = ArtistId::from_uuid(v4);
    assert_eq!(
        Artist::new(NewArtist {
            id: invalid_artist,
            public_api_id: ArtistPublicId::from_uuid(v4),
            canonical_name: "Name".to_owned(),
            normalized_name: "name".to_owned(),
            sort_name: "Name".to_owned(),
            artist_type: ArtistType::Person,
            disambiguation: None,
            active_dates: None,
            status: ArtistStatus::Provisional,
            created_at: OffsetDateTime::UNIX_EPOCH,
        }),
        Err(DomainInvariantError::InvalidInternalId("ArtistId"))
    );

    assert_eq!(
        ObservedArtist::new(ObservedArtist {
            id: ObservedArtistId::from_uuid(v4),
            release_id: ReleaseId::new(),
            track_id: None,
            raw_value: "Name".to_owned(),
            normalized_value: "name".to_owned(),
            source_file: None,
            source_field: "artist".to_owned(),
        }),
        Err(DomainInvariantError::InvalidInternalId("ObservedArtistId"))
    );

    let relationship = ArtistRelationship {
        id: ArtistRelationshipId::new(),
        subject_artist_id: invalid_artist,
        object_artist_id: ArtistId::new(),
        kind: ArtistRelationshipKind::new("collaborated_with".to_owned()).unwrap(),
        active_dates: None,
        confidence: confidence(8_000),
        evidence_source: "test".to_owned(),
    };
    assert_eq!(
        ArtistRelationship::new(relationship),
        Err(DomainInvariantError::InvalidInternalId("ArtistId"))
    );

    let invalid_link = TrackMediaAsset {
        track_id: TrackId::from_uuid(v4),
        media_asset_id: MediaAssetId::new(),
        role: MediaAssetRole::Playable,
        ordinal: 1,
    };
    assert_eq!(
        TrackMediaAsset::new(invalid_link),
        Err(DomainInvariantError::InvalidInternalId("TrackId"))
    );

    let observation = ObservedArtistId::new();
    assert_eq!(
        ArtistMatchEvidence::new(ArtistMatchEvidence {
            id: ArtistMatchEvidenceId::new(),
            observed_artist_id: observation,
            candidate_artist_id: invalid_artist,
            kind: EvidenceKind::Alias,
            polarity: EvidencePolarity::Supports,
            confidence: confidence(9_000),
            source: "test".to_owned(),
        }),
        Err(DomainInvariantError::InvalidInternalId("ArtistId"))
    );
}

#[test]
fn release_adopts_reserved_manifest_identity_and_track_ordinals_are_unique() {
    let reserved = ReleaseId::reserve_for_publication();
    let release = Release::from_published_manifest(
        reserved,
        ReleasePublicId::from(reserved),
        "Release".to_owned(),
        DisplayArtist::new("Artist".to_owned()).unwrap(),
        None,
        OffsetDateTime::UNIX_EPOCH,
    )
    .unwrap();
    assert_eq!(release.id(), reserved);

    let audio = AudioIdentityId::new();
    let asset = MediaAssetId::new();
    let duplicate = [
        track(TrackId::new(), reserved, audio, asset, 1),
        track(TrackId::new(), reserved, audio, asset, 1),
    ];
    assert_eq!(
        validate_release_track_ordinals(&duplicate),
        Err(DomainInvariantError::InvalidOrdinal)
    );
}

#[test]
fn merge_tombstones_and_split_moves_only_explicit_relationships() {
    let source = ArtistId::new();
    let target = ArtistId::new();
    let merge = ArtistMerge::new(ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: source,
        source_public_api_id: source.into(),
        target_artist_id: target,
        actor: "curator".to_owned(),
        reason: "same identity".to_owned(),
        occurred_at: OffsetDateTime::UNIX_EPOCH,
    })
    .unwrap();
    assert_eq!(merge.tombstone_status(), ArtistStatus::MergedInto(target));

    let assignment = SplitAssignment::Contribution(ContributionId::new());
    let split = ArtistSplit::new(ArtistSplit {
        id: ArtistSplitId::new(),
        source_artist_id: target,
        new_artist_id: ArtistId::new(),
        assignments: vec![assignment],
        actor: "curator".to_owned(),
        reason: "wrong contribution".to_owned(),
        occurred_at: OffsetDateTime::UNIX_EPOCH,
    })
    .unwrap();
    assert_eq!(split.assignments, vec![assignment]);

    let duplicate = ArtistSplit {
        id: ArtistSplitId::new(),
        source_artist_id: target,
        new_artist_id: ArtistId::new(),
        assignments: vec![assignment, assignment],
        actor: "curator".to_owned(),
        reason: "wrong contribution".to_owned(),
        occurred_at: OffsetDateTime::UNIX_EPOCH,
    };
    assert_eq!(
        ArtistSplit::new(duplicate),
        Err(DomainInvariantError::DuplicateSplitAssignment)
    );
}

fn overlay(id: CatalogSemanticOverlayId, actor: &str) -> CatalogSemanticOverlay {
    let release_id =
        ReleaseId::from_uuid(Uuid::parse_str("01989f00-7000-7000-8000-000000000001").unwrap());
    let track_id =
        TrackId::from_uuid(Uuid::parse_str("01989f00-7000-7000-8000-000000000002").unwrap());
    CatalogSemanticOverlay::new(NewCatalogSemanticOverlay {
        id,
        release_id,
        source_manifest_id: Uuid::parse_str("01989f00-7000-7000-8000-000000000003").unwrap(),
        source_semantic_manifest_hash: "a".repeat(64),
        publication_plan_hash: "b".repeat(64),
        catalog_import_id: CatalogImportId::from_uuid(
            Uuid::parse_str("01989f00-7000-7000-8000-000000000004").unwrap(),
        ),
        resolver_version: "resolver-v1".to_owned(),
        predecessor_id: None,
        references: OverlayReferences {
            artist_resolutions: vec![OverlayArtistResolution {
                observed_artist_id: ObservedArtistId::from_uuid(
                    Uuid::parse_str("01989f00-7000-7000-8000-000000000005").unwrap(),
                ),
                artist_id: ArtistId::from_uuid(
                    Uuid::parse_str("01989f00-7000-7000-8000-000000000006").unwrap(),
                ),
                decision_id: None,
            }],
            audio_identities: vec![OverlayAudioIdentityLink {
                track_id,
                audio_identity_id: AudioIdentityId::from_uuid(
                    Uuid::parse_str("01989f00-7000-7000-8000-000000000007").unwrap(),
                ),
            }],
            display_artists: vec![OverlayDisplayArtist {
                owner: DisplayArtistOwner::Track(track_id),
                value: DisplayArtist::new("Somebody feat. Colin Hay".to_owned()).unwrap(),
            }],
            ..OverlayReferences::default()
        },
        created_at: OffsetDateTime::UNIX_EPOCH,
        created_by: actor.to_owned(),
    })
    .unwrap()
}

#[test]
fn semantic_overlay_hash_is_deterministic_and_separate_from_audit_provenance() {
    let first = overlay(CatalogSemanticOverlayId::new(), "worker-a");
    let second = overlay(CatalogSemanticOverlayId::new(), "worker-b");
    assert_eq!(
        first.overlay_hash().unwrap(),
        second.overlay_hash().unwrap()
    );
    assert_eq!(second.source_manifest_id(), first.source_manifest_id());
}

#[test]
fn overlay_rejects_duplicate_semantic_references() {
    let id = ReleaseArtistId::new();
    let release_id = ReleaseId::new();
    let result = CatalogSemanticOverlay::new(NewCatalogSemanticOverlay {
        id: CatalogSemanticOverlayId::new(),
        release_id,
        source_manifest_id: Uuid::now_v7(),
        source_semantic_manifest_hash: "a".repeat(64),
        publication_plan_hash: "b".repeat(64),
        catalog_import_id: CatalogImportId::new(),
        resolver_version: "resolver-v1".to_owned(),
        predecessor_id: None,
        references: OverlayReferences {
            release_artists: vec![id, id],
            ..OverlayReferences::default()
        },
        created_at: OffsetDateTime::UNIX_EPOCH,
        created_by: "worker".to_owned(),
    });
    assert_eq!(result, Err(DomainInvariantError::DuplicateOverlayReference));
}
