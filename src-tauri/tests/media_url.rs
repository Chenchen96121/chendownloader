use local_media_downloader_lib::{
    application::media_url::validate_media_url,
    domain::instagram::InstagramUrlKind,
    domain::media::{AnalysisErrorCode, Platform},
};

#[test]
fn canonicalizes_x_post_aliases_without_losing_large_ids() {
    for input in [
        " https://twitter.com/example/status/12345678901234567890?utm_source=share#part ",
        "https://www.x.com/i/status/12345678901234567890",
        "https://x.com/i/web/status/12345678901234567890/",
        "https://www.twitter.com/example/status/12345678901234567890/photo/2",
        "https://x.com/example/status/12345678901234567890/video/1?x=1",
    ] {
        let source = validate_media_url(input).unwrap();
        assert_eq!(source.media_id(), "12345678901234567890");
        assert_eq!(
            source.url(),
            "https://x.com/i/web/status/12345678901234567890"
        );
    }
}

#[test]
fn rejects_x_ambiguous_authority_and_non_attachment_suffixes() {
    for input in [
        "https://x.com:443/example/status/1",
        "https://user@x.com/example/status/1",
        "https://x.com/example/status/1/photo/0",
        "https://x.com/example/status/1/photo/%31",
        "https://x.com/example/status/1/video/33",
        "https://x.com/i/web/status/%31",
        "https://x.com/example/status/1/other/1",
        "https://t.co/example",
    ] {
        assert!(validate_media_url(input).is_err(), "{input}");
    }
}

#[test]
fn classifies_only_literal_resource_routes_on_exact_hosts() {
    for (input, platform, id) in [
        (
            " https://www.youtube.com/watch?v=demoYT00001&t=5 ",
            Platform::Youtube,
            "demoYT00001",
        ),
        (
            "https://youtube.com/watch/?v=demoYT00001#part",
            Platform::Youtube,
            "demoYT00001",
        ),
        (
            "https://m.youtube.com/shorts/demoYT00001/",
            Platform::Youtube,
            "demoYT00001",
        ),
        (
            "https://youtu.be/demoYT00001",
            Platform::Youtube,
            "demoYT00001",
        ),
        (
            "https://www.bilibili.com/video/BVdemo000001/",
            Platform::Bilibili,
            "BVdemo000001",
        ),
        (
            "https://bilibili.com/video/av12345678901234567890",
            Platform::Bilibili,
            "av12345678901234567890",
        ),
        (
            "https://x.com/sample_user/status/12345678901234567890/",
            Platform::X,
            "12345678901234567890",
        ),
        ("https://www.twitter.com/a/status/1", Platform::X, "1"),
        ("https://twitter.com/a/status/2", Platform::X, "2"),
        ("https://www.x.com/a/status/3", Platform::X, "3"),
        (
            "https://instagram.com/p/sample_01-",
            Platform::Instagram,
            "sample_01-",
        ),
        (
            "https://www.instagram.com/reel/sample_02/",
            Platform::Instagram,
            "sample_02",
        ),
        (
            "https://YOUTUBE.COM:443/watch?v=demoYT00001",
            Platform::Youtube,
            "demoYT00001",
        ),
    ] {
        let source = validate_media_url(input).unwrap();
        assert_eq!(source.platform(), platform, "{input}");
        assert_eq!(source.media_id(), id, "{input}");
    }
}

#[test]
fn keeps_canonical_url_and_native_only_query_and_fragment() {
    let source =
        validate_media_url(" https://YOUTUBE.COM:443/watch?v=demoYT00001&t=5#part ").unwrap();
    assert_eq!(
        source.url(),
        "https://youtube.com/watch?v=demoYT00001&t=5#part"
    );
}

#[test]
fn canonicalizes_instagram_posts_and_reels_without_tracking_data() {
    for (input, kind, canonical) in [
        (
            " https://instagram.com/p/Abc_123-/?utm_source=share#fragment ",
            InstagramUrlKind::Post,
            "https://www.instagram.com/p/Abc_123-/",
        ),
        (
            "https://www.instagram.com/reel/Reel_456-?igsh=private-token",
            InstagramUrlKind::Reel,
            "https://www.instagram.com/reel/Reel_456-/",
        ),
    ] {
        let source = validate_media_url(input).unwrap();
        assert_eq!(source.platform(), Platform::Instagram);
        assert_eq!(source.instagram_kind(), Some(kind));
        assert_eq!(source.url(), canonical);
    }
    assert_eq!(
        validate_media_url("https://youtu.be/demoYT00001")
            .unwrap()
            .instagram_kind(),
        None
    );
}

#[test]
fn plural_instagram_reel_alias_preserves_identity_and_canonical_source() {
    for input in [
        "https://www.instagram.com/reels/Det5ABjSXVU/",
        "https://instagram.com/reels/Det5ABjSXVU?utm_source=share#clip",
        "https://www.instagram.com/reel/Det5ABjSXVU/?utm_source=ig_web_copy_link&dlrf=SYNTHETIC",
    ] {
        let source = validate_media_url(input).unwrap_or_else(|error| panic!("{input}: {error:?}"));
        assert_eq!(source.platform(), Platform::Instagram);
        assert_eq!(source.instagram_kind(), Some(InstagramUrlKind::Reel));
        assert_eq!(source.media_id(), "Det5ABjSXVU");
        assert_eq!(source.url(), "https://www.instagram.com/reel/Det5ABjSXVU/");
    }
}

#[test]
fn plural_instagram_reel_alias_does_not_accept_feeds_or_widen_url_authority() {
    for input in [
        "https://www.instagram.com/reels/",
        "https://www.instagram.com/reels/Det5ABjSXVU/embed/",
        "https://www.instagram.com/reels//Det5ABjSXVU/",
        "https://www.instagram.com/%72eels/Det5ABjSXVU/",
        "https://www.instagram.com/reels/%44et5ABjSXVU/",
        "https://instagram.com.evil.invalid/reels/Det5ABjSXVU/",
        "https://user@www.instagram.com/reels/Det5ABjSXVU/",
        "https://www.instagram.com:444/reels/Det5ABjSXVU/",
        "http://www.instagram.com/reels/Det5ABjSXVU/",
    ] {
        assert!(validate_media_url(input).is_err(), "{input}");
    }
}

#[test]
fn accepts_exact_byte_limit_and_literal_id_with_encoded_parameter_key() {
    let base = "https://youtu.be/demoYT00001?padding=";
    let input = format!("{base}{}", "a".repeat(2048 - base.len()));
    assert_eq!(
        validate_media_url(&input).unwrap().media_id(),
        "demoYT00001"
    );
    assert_eq!(
        validate_media_url("https://youtube.com/watch?%76=demoYT00001")
            .unwrap()
            .media_id(),
        "demoYT00001"
    );
}

#[test]
fn does_not_let_parser_normalization_hide_malformed_authority() {
    assert_eq!(
        validate_media_url("https:///youtube.com/watch?v=demoYT00001").err(),
        Some(AnalysisErrorCode::InvalidUrl)
    );
}

#[test]
fn rejects_encoded_identifier_even_when_parameter_key_is_encoded() {
    assert_eq!(
        validate_media_url("https://youtube.com/watch?%76=%64emoYT00001").err(),
        Some(AnalysisErrorCode::UnsupportedMedia)
    );
}

#[test]
fn rejects_non_resource_paths_hidden_by_dot_segment_normalization() {
    for input in [
        "https://youtu.be/other/../demoYT00001",
        "https://instagram.com/p/%2e/sample_01",
        "https://youtube.com/channel/../watch?v=demoYT00001",
    ] {
        assert_eq!(
            validate_media_url(input).err(),
            Some(AnalysisErrorCode::UnsupportedMedia),
            "{input}"
        );
    }
}

#[test]
fn rejects_malformed_or_unsafe_urls_with_stable_invalid_url_code() {
    for input in [
        "",
        "not a uri",
        "https://",
        "http://youtube.com/watch?v=demoYT00001",
        "file:///watch?v=demoYT00001",
        "https://user:pass@youtube.com/watch?v=demoYT00001",
        "https://@youtube.com/watch?v=demoYT00001",
        "https://youtube.com:444/watch?v=demoYT00001",
        "https://127.0.0.1/watch?v=demoYT00001",
        "https://[::1]/watch?v=demoYT00001",
        "https://youtube.com/watch?v=demoYT\n00001",
        "\nhttps://youtu.be/demoYT00001",
        "https://youtube.com\\watch?v=demoYT00001",
        "https:youtube.com/watch?v=demoYT00001",
    ] {
        assert_eq!(
            validate_media_url(input).err(),
            Some(AnalysisErrorCode::InvalidUrl),
            "{input:?}"
        );
    }
    let oversized = format!("https://youtu.be/demoYT00001?{}", "x".repeat(2048));
    assert_eq!(
        validate_media_url(&oversized).err(),
        Some(AnalysisErrorCode::InvalidUrl)
    );
    let unicode_oversized = format!("https://youtu.be/demoYT00001?{}", "界".repeat(700));
    assert_eq!(
        validate_media_url(&unicode_oversized).err(),
        Some(AnalysisErrorCode::InvalidUrl)
    );
}

#[test]
fn rejects_unknown_and_misleading_hosts_as_unsupported_platform() {
    for input in [
        "https://youtube.com.evil.invalid/watch?v=demoYT00001",
        "https://evil-youtube.com/watch?v=demoYT00001",
        "https://youtube.com./watch?v=demoYT00001",
        "https://music.youtube.com/watch?v=demoYT00001",
        "https://example.invalid/p/sample",
    ] {
        assert_eq!(
            validate_media_url(input).err(),
            Some(AnalysisErrorCode::UnsupportedPlatform),
            "{input}"
        );
    }
}

#[test]
fn rejects_non_resource_routes_and_invalid_or_encoded_identifiers() {
    for input in [
        "https://youtube.com/playlist?list=sample",
        "https://youtube.com/channel/sample",
        "https://youtube.com/live/demoYT00001",
        "https://youtube.com/watch",
        "https://youtube.com/watch?v=demoYT00001&v=demoYT00002",
        "https://youtube.com/watch?v=demoYT00001&%76=demoYT00002",
        "https://youtube.com/watch?v=%64emoYT00001",
        "https://youtube.com/watch?v=short",
        "https://youtu.be/demoYT00001//",
        "https://youtu.be/demoYT00001/extra",
        "https://youtu.be/%64emoYT00001",
        "https://youtube.com/shorts/demoYT0000!",
        "https://bilibili.com/video/BVshort",
        "https://bilibili.com/video/BVdemo00000!",
        "https://bilibili.com/video/av",
        "https://bilibili.com/video/av123456789012345678901",
        "https://bilibili.com/video/av1x",
        "https://bilibili.com/space/123",
        "https://x.com/sample_user",
        "https://x.com/user_too_long_16/status/1",
        "https://x.com/user-name/status/1",
        "https://twitter.com/a/status/1x",
        "https://x.com/a/status/123456789012345678901",
        "https://x.com/a/status/%31",
        "https://instagram.com/sample",
        "https://instagram.com/p/",
        "https://instagram.com/p/bad+id",
        "https://instagram.com/reel/%73ample",
        "https://instagram.com/p/sample/gallery",
    ] {
        assert_eq!(
            validate_media_url(input).err(),
            Some(AnalysisErrorCode::UnsupportedMedia),
            "{input}"
        );
    }
    let long_instagram = format!("https://instagram.com/p/{}", "a".repeat(65));
    assert_eq!(
        validate_media_url(&long_instagram).err(),
        Some(AnalysisErrorCode::UnsupportedMedia)
    );
}
