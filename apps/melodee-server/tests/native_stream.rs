#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{fs, time::Duration};

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use http_body_util::BodyExt;
use melodee_server::native_stream::{
    ByteRange, CanonicalAssetRoot, ClientAddressResolver, DirectPlayService, PlayableAsset,
    StreamError, StreamTokenCodec, TrustedProxy, parse_byte_range,
};
use tempfile::TempDir;
use uuid::Uuid;

const CONTENT: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

struct Fixture {
    _temp: TempDir,
    service: DirectPlayService,
    asset: PlayableAsset,
}

impl Fixture {
    fn new(max_concurrent_streams: usize) -> Self {
        let temp = tempfile::tempdir().expect("create stream fixture");
        let nested = temp.path().join("releases/01/media");
        fs::create_dir_all(&nested).expect("create nested canonical path");
        fs::write(nested.join("asset.flac"), CONTENT).expect("write asset");
        let root = CanonicalAssetRoot::open(temp.path()).expect("open canonical asset root");
        let service = DirectPlayService::new(root, max_concurrent_streams)
            .expect("positive stream concurrency");
        Self {
            _temp: temp,
            service,
            asset: PlayableAsset {
                canonical_relative_path: "releases/01/media/asset.flac".to_owned(),
                byte_size: CONTENT.len() as u64,
                mime_type: "audio/flac".to_owned(),
            },
        }
    }
}

#[test]
fn captured_token_is_url_safe_timed_client_bound_and_replayable_for_seeks() {
    let codec = StreamTokenCodec;
    let user = Uuid::parse_str("f78448cc-82ec-5864-877c-6c6163b00065").unwrap();
    let track = Uuid::parse_str("2b8eab4b-235d-5892-99f8-c0f5f54e8ab4").unwrap();
    let issued_at = 1_786_307_710_i64;
    let token = codec
        .issue(
            "legacy-user-public-key",
            user,
            track,
            "127.0.0.1",
            issued_at,
        )
        .expect("issue token");
    assert_eq!(
        token,
        "Zjc4NDQ4Y2MtODJlYy01ODY0LTg3N2MtNmM2MTYzYjAwMDY1OjJiOGVhYjRiLTIzNWQtNTg5Mi05OWY4LWMwZjVmNTRlOGFiNDoxMjcuMC4wLjF8MTc4NjkxMjUxMHxjaWVXUm9YNnZCU0xNMlZwbTIyRnhJc2hQQjI5bEdFTkRHbkZOSjZDdm5VPQ"
    );
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "outer token must match legacy URL-safe unpadded base64"
    );

    for now in [issued_at, issued_at + 60, issued_at + 7 * 24 * 60 * 60] {
        codec
            .validate(
                "legacy-user-public-key",
                &token,
                user,
                track,
                "127.0.0.1",
                now,
            )
            .expect("token replay remains valid through its exact expiry");
    }
}

#[test]
fn token_expiry_tampering_and_every_bound_claim_fail_uniformly() {
    let codec = StreamTokenCodec;
    let user = Uuid::parse_str("f78448cc-82ec-5864-877c-6c6163b00065").unwrap();
    let track = Uuid::parse_str("2b8eab4b-235d-5892-99f8-c0f5f54e8ab4").unwrap();
    let other = Uuid::parse_str("b3297a8f-5e2b-5ade-98ea-f899d38059e0").unwrap();
    let issued_at = 1_786_307_710_i64;
    let token = codec
        .issue(
            "legacy-user-public-key",
            user,
            track,
            "127.0.0.1",
            issued_at,
        )
        .unwrap();
    let mut tampered = token.clone();
    tampered.push('A');

    let failures = [
        codec.validate(
            "legacy-user-public-key",
            &token,
            user,
            track,
            "127.0.0.1",
            issued_at + 7 * 24 * 60 * 60 + 1,
        ),
        codec.validate("different-key", &token, user, track, "127.0.0.1", issued_at),
        codec.validate(
            "legacy-user-public-key",
            &tampered,
            user,
            track,
            "127.0.0.1",
            issued_at,
        ),
        codec.validate(
            "legacy-user-public-key",
            &token,
            other,
            track,
            "127.0.0.1",
            issued_at,
        ),
        codec.validate(
            "legacy-user-public-key",
            &token,
            user,
            other,
            "127.0.0.1",
            issued_at,
        ),
        codec.validate(
            "legacy-user-public-key",
            &token,
            user,
            track,
            "127.0.0.2",
            issued_at,
        ),
    ];
    for failure in failures {
        assert_eq!(
            failure.expect_err("credential must fail").to_string(),
            "invalid stream credential"
        );
    }
}

#[test]
fn malformed_stream_tokens_never_panic_or_reveal_parse_details() {
    let codec = StreamTokenCodec;
    let user = Uuid::nil();
    for token in [
        "",
        "%not-base64%",
        "bm90LXV0Zi04_w",
        "bm8tcGlwZXM",
        "YXxi",
        "YXxifGM",
        &"A".repeat(4097),
    ] {
        let error = codec
            .validate("key", token, user, user, "127.0.0.1", 0)
            .expect_err("malformed token");
        assert_eq!(error.to_string(), "invalid stream credential");
    }
}

#[test]
fn untrusted_peers_cannot_spoof_forwarding_headers() {
    let resolver = ClientAddressResolver::new(vec!["10.0.0.0/8".parse().unwrap()]);
    let peer = "192.0.2.40:4040".parse().unwrap();
    assert_eq!(
        resolver
            .resolve(peer, Some("for=198.51.100.7"), Some("203.0.113.8"))
            .unwrap(),
        "192.0.2.40".parse::<std::net::IpAddr>().unwrap()
    );
}

#[test]
fn trusted_proxy_headers_have_strict_precedence_and_syntax() {
    let resolver = ClientAddressResolver::new(vec![
        "10.0.0.0/8".parse().unwrap(),
        "2001:db8::/32".parse().unwrap(),
    ]);
    let peer = "10.0.0.4:4040".parse().unwrap();
    assert_eq!(
        resolver
            .resolve(
                peer,
                Some("for=198.51.100.7;proto=https, for=10.0.0.4"),
                Some("203.0.113.8, 10.0.0.4")
            )
            .unwrap(),
        "198.51.100.7".parse::<std::net::IpAddr>().unwrap()
    );
    assert_eq!(
        resolver
            .resolve(peer, None, Some("203.0.113.8, 10.0.0.4"))
            .unwrap(),
        "203.0.113.8".parse::<std::net::IpAddr>().unwrap()
    );
    assert!(
        resolver
            .resolve(peer, Some("for=unknown"), Some("203.0.113.8"))
            .is_err(),
        "invalid higher-precedence Forwarded must fail closed"
    );

    let ipv6_peer = "[2001:db8::1]:4040".parse().unwrap();
    assert_eq!(
        resolver
            .resolve(ipv6_peer, Some("for=\"[2001:db9:1::7]:8443\""), None)
            .unwrap(),
        "2001:db9:1::7".parse::<std::net::IpAddr>().unwrap()
    );
}

#[test]
fn trusted_proxy_chain_discards_attacker_supplied_leftmost_values() {
    let resolver = ClientAddressResolver::new(vec!["10.0.0.0/8".parse().unwrap()]);
    let peer = "10.0.0.4:4040".parse().unwrap();
    let expected = "198.51.100.7".parse::<std::net::IpAddr>().unwrap();
    assert_eq!(
        resolver
            .resolve(peer, None, Some("203.0.113.250, 198.51.100.7, 10.0.0.5"))
            .unwrap(),
        expected
    );
    assert_eq!(
        resolver
            .resolve(
                peer,
                Some("for=203.0.113.250, for=198.51.100.7, for=10.0.0.5"),
                None
            )
            .unwrap(),
        expected
    );
    assert!(
        resolver
            .resolve(peer, None, Some("10.0.0.8, 10.0.0.5"))
            .is_err(),
        "a chain containing only trusted infrastructure has no client identity"
    );
    let too_many = std::iter::repeat_n("198.51.100.7", 33)
        .collect::<Vec<_>>()
        .join(",");
    assert!(resolver.resolve(peer, None, Some(&too_many)).is_err());
}

#[test]
fn trusted_proxy_configuration_rejects_hosts_bad_prefixes_and_noncanonical_networks() {
    for value in [
        "proxy.example.test",
        "10.0.0.0/33",
        "2001:db8::/129",
        "10.0.0.1/24",
        "2001:db8::1/32",
        "10.0.0.0/not-a-prefix",
        "",
    ] {
        assert!(value.parse::<TrustedProxy>().is_err(), "{value:?}");
    }
    for value in ["10.0.0.0/8", "10.1.2.3", "2001:db8::/32", "::1"] {
        assert!(value.parse::<TrustedProxy>().is_ok(), "{value:?}");
    }
}

#[test]
fn captured_single_range_matrix_is_exact() {
    let length = 220_544;
    assert_eq!(
        parse_byte_range("bytes=0-1023", length).expect("initial range"),
        ByteRange {
            start: 0,
            end: 1023
        }
    );
    assert_eq!(
        parse_byte_range("bytes=100000-101023", length).expect("middle range"),
        ByteRange {
            start: 100_000,
            end: 101_023
        }
    );
    assert_eq!(
        parse_byte_range("bytes=-2048", length).expect("suffix range"),
        ByteRange {
            start: 218_496,
            end: 220_543
        }
    );
    assert_eq!(
        parse_byte_range("bytes=200000-", length).expect("open range"),
        ByteRange {
            start: 200_000,
            end: 220_543
        }
    );
    assert_eq!(
        parse_byte_range("bytes=220000-999999", length).expect("clamped range"),
        ByteRange {
            start: 220_000,
            end: 220_543
        }
    );
    assert_eq!(
        parse_byte_range("bytes=-999999", length).expect("large suffix clamps to full asset"),
        ByteRange {
            start: 0,
            end: 220_543
        }
    );
}

#[test]
fn malformed_inverted_multipart_and_unsatisfiable_ranges_are_rejected() {
    for value in [
        "",
        "bytes=",
        "bytes=-",
        "bytes=abc-def",
        "bytes=1000-500",
        "bytes=0-99,200-299",
        "bytes=999999999-",
        "bytes=-0",
        "bytes= 0-1",
        "items=0-1",
        "bytes=18446744073709551616-",
    ] {
        assert!(
            matches!(
                parse_byte_range(value, 220_544),
                Err(StreamError::InvalidRange)
            ),
            "{value:?} must preserve captured 400 behavior"
        );
    }
    assert!(matches!(
        parse_byte_range("bytes=0-0", 0),
        Err(StreamError::InvalidRange)
    ));
    assert_eq!(
        parse_byte_range("  Bytes=0-1  ", 10).expect("legacy parser is case insensitive"),
        ByteRange { start: 0, end: 1 }
    );
}

#[tokio::test]
async fn full_get_and_head_have_exact_length_type_and_range_headers() {
    let fixture = Fixture::new(2);
    let response = fixture
        .service
        .response(&fixture.asset, None, false, None)
        .await
        .expect("full response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "audio/flac");
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        CONTENT.len().to_string()
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("read full body")
        .to_bytes();
    assert_eq!(&body[..], CONTENT);

    let response = fixture
        .service
        .response(&fixture.asset, None, true, None)
        .await
        .expect("head response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        CONTENT.len().to_string()
    );
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
}

#[tokio::test]
async fn bounded_asset_reads_share_the_no_follow_stream_boundary() {
    let fixture = Fixture::new(2);
    let bytes = fixture
        .service
        .read_asset_bounded(&fixture.asset, CONTENT.len() as u64, None)
        .await
        .expect("bounded asset read");
    assert_eq!(bytes, CONTENT);
    assert!(
        fixture
            .service
            .read_asset_bounded(&fixture.asset, (CONTENT.len() - 1) as u64, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn initial_middle_suffix_and_open_ranges_return_exact_bytes() {
    let fixture = Fixture::new(2);
    let cases = [
        ("bytes=0-3", "bytes 0-3/36", &CONTENT[0..4]),
        ("bytes=10-15", "bytes 10-15/36", &CONTENT[10..16]),
        ("bytes=-4", "bytes 32-35/36", &CONTENT[32..36]),
        ("bytes=30-", "bytes 30-35/36", &CONTENT[30..36]),
    ];
    for (range, content_range, expected) in cases {
        let response = fixture
            .service
            .response(&fixture.asset, Some(range), false, None)
            .await
            .expect("range response");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], content_range);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            expected.len().to_string()
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], expected);
    }
}

#[tokio::test]
async fn dropping_a_body_releases_bounded_stream_capacity() {
    let fixture = Fixture::new(1);
    let first = fixture
        .service
        .response(&fixture.asset, None, false, None)
        .await
        .expect("first response");
    let service = fixture.service.clone();
    let asset = fixture.asset.clone();
    let mut second = tokio::spawn(async move { service.response(&asset, None, false, None).await });

    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut second)
            .await
            .is_err(),
        "second stream must wait for capacity"
    );
    drop(first);
    let response = tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .expect("capacity released promptly")
        .expect("stream task joined")
        .expect("second response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn request_like_paths_wrong_sizes_and_non_regular_assets_fail_closed() {
    let fixture = Fixture::new(1);
    for path in ["", "/etc/passwd", "../outside", "media/../../outside"] {
        let mut asset = fixture.asset.clone();
        asset.canonical_relative_path = path.to_owned();
        assert!(matches!(
            CanonicalAssetRoot::open(fixture._temp.path())
                .expect("root")
                .open_asset(&asset),
            Err(StreamError::AssetUnavailable)
        ));
    }

    let mut wrong_size = fixture.asset.clone();
    wrong_size.byte_size += 1;
    assert!(matches!(
        CanonicalAssetRoot::open(fixture._temp.path())
            .expect("root")
            .open_asset(&wrong_size),
        Err(StreamError::AssetUnavailable)
    ));

    fs::create_dir_all(fixture._temp.path().join("directory-asset")).unwrap();
    let directory = PlayableAsset {
        canonical_relative_path: "directory-asset".to_owned(),
        byte_size: 0,
        mime_type: "audio/flac".to_owned(),
    };
    assert!(matches!(
        CanonicalAssetRoot::open(fixture._temp.path())
            .expect("root")
            .open_asset(&directory),
        Err(StreamError::AssetUnavailable)
    ));
}

#[cfg(unix)]
#[test]
fn symlinked_root_components_and_asset_leaves_are_never_followed() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("fixture");
    let real = temp.path().join("real");
    fs::create_dir_all(real.join("nested")).unwrap();
    fs::write(real.join("outside.flac"), CONTENT).unwrap();
    fs::write(real.join("nested/inside.flac"), CONTENT).unwrap();
    symlink(&real, temp.path().join("root-link")).unwrap();
    assert!(matches!(
        CanonicalAssetRoot::open(&temp.path().join("root-link")),
        Err(StreamError::AssetUnavailable)
    ));

    symlink(real.join("outside.flac"), real.join("nested/linked.flac")).unwrap();
    let root = CanonicalAssetRoot::open(&real).expect("real root");
    let linked = PlayableAsset {
        canonical_relative_path: "nested/linked.flac".to_owned(),
        byte_size: CONTENT.len() as u64,
        mime_type: "audio/flac".to_owned(),
    };
    assert!(matches!(
        root.open_asset(&linked),
        Err(StreamError::AssetUnavailable)
    ));

    symlink(real.join("nested"), real.join("nested-link")).unwrap();
    let linked_component = PlayableAsset {
        canonical_relative_path: "nested-link/inside.flac".to_owned(),
        byte_size: CONTENT.len() as u64,
        mime_type: "audio/flac".to_owned(),
    };
    assert!(matches!(
        root.open_asset(&linked_component),
        Err(StreamError::AssetUnavailable)
    ));
}

#[test]
fn zero_concurrency_is_rejected() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = CanonicalAssetRoot::open(temp.path()).expect("root");
    assert!(DirectPlayService::new(root, 0).is_none());
}

// Keep the Axum body type part of this slice's compile contract. The native
// route wrapper can return it without buffering a complete media asset.
fn _assert_body_type(_: Body) {}

#[tokio::test]
async fn direct_play_gauge_reports_in_flight_waiters_and_release() {
    let registry = melodee_server::resource_bounds::ResourceBounds::new();
    let gauge = registry.register("streams.direct_play", 1);
    let fixture = Fixture::new(1);
    let service = fixture.service.with_gauge(gauge.clone());

    let first = service
        .response(&fixture.asset, None, false, None)
        .await
        .expect("first stream admitted");
    let snapshot = gauge.snapshot();
    assert_eq!(snapshot.in_flight, 1);
    assert!(snapshot.saturated);

    // A second request queues on the semaphore: the waiter is visible while
    // blocked, then admitted once the first response body drops.
    let queued = {
        let service = service.clone();
        let asset = fixture.asset.clone();
        tokio::spawn(async move { service.response(&asset, None, false, None).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let snapshot = gauge.snapshot();
    assert_eq!(snapshot.waiting, 1);
    assert!(snapshot.oldest_wait_ms > 0);

    drop(first);
    let second = queued
        .await
        .expect("queued task joins")
        .expect("queued stream admitted after release");
    let snapshot = gauge.snapshot();
    assert_eq!(snapshot.in_flight, 1);
    assert_eq!(snapshot.waiting, 0);
    assert_eq!(snapshot.admitted_total, 2);
    drop(second);
    assert_eq!(gauge.snapshot().in_flight, 0);
}

#[tokio::test]
async fn per_user_stream_cap_rejects_only_the_over_limit_user() {
    let fixture = Fixture::new(4);
    let service = fixture.service.clone().with_per_user_limit(1);
    let first_user = Uuid::now_v7();
    let second_user = Uuid::now_v7();

    // First admission for the user holds its slot until the body is dropped.
    let first = service
        .response(&fixture.asset, None, false, Some(first_user))
        .await
        .expect("first admission");
    let second = service
        .response(&fixture.asset, None, false, Some(first_user))
        .await;
    assert!(
        matches!(second, Err(StreamError::TooManyStreams)),
        "second concurrent stream for the same user must be rejected"
    );

    // Another user is unaffected by the first user's cap.
    let other = service
        .response(&fixture.asset, None, false, Some(second_user))
        .await
        .expect("independent user admission");

    // Dropping the bodies releases both slots.
    drop(first);
    drop(other);
    service
        .response(&fixture.asset, None, false, Some(first_user))
        .await
        .expect("slot released after body drop");
}

#[tokio::test]
async fn per_user_cap_zero_disables_the_admission_bound() {
    let fixture = Fixture::new(8);
    let service = fixture.service.clone().with_per_user_limit(0);
    assert_eq!(service.per_user_limit(), None);
    let user = Uuid::now_v7();
    let first = service
        .response(&fixture.asset, None, false, Some(user))
        .await
        .expect("no per-user bound");
    let second = service
        .response(&fixture.asset, None, false, Some(user))
        .await
        .expect("no per-user bound");
    let third = service
        .response(&fixture.asset, None, false, Some(user))
        .await
        .expect("no per-user bound");
    drop((first, second, third));
}
