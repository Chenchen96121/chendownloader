use local_media_downloader_lib::{
    domain::{instagram::InstagramMediaKind, media::AnalysisErrorCode},
    infrastructure::gallery_dl::normalize_instagram_inventory,
};

const SINGLE_IMAGE: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/single-image.jsonl");
const SINGLE_VIDEO: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/single-video.jsonl");
const MIXED: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/mixed-carousel.jsonl");
const MALFORMED: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/malformed.jsonl");

#[test]
fn normalizes_single_image_video_and_mixed_posts_without_exposing_private_urls() {
    let image = normalize_instagram_inventory("Image_01", SINGLE_IMAGE).unwrap();
    assert_eq!(image.shortcode(), "Image_01");
    assert_eq!(image.title(), "Single image");
    assert_eq!(image.creator(), Some("photographer"));
    assert_eq!(image.items().len(), 1);
    assert_eq!(
        image.items()[0].public_item().kind,
        InstagramMediaKind::Image
    );
    assert_eq!(image.items()[0].public_item().width, Some(1080));
    assert_eq!(image.items()[0].public_item().height, Some(1350));
    assert!(image.items()[0].public_item().preview_available);

    let video = normalize_instagram_inventory("Video_01", SINGLE_VIDEO).unwrap();
    assert_eq!(
        video.items()[0].public_item().kind,
        InstagramMediaKind::Video
    );
    assert!(video.items()[0].has_video_source());
    assert!(video.items()[0].public_item().preview_available);

    let mixed = normalize_instagram_inventory("Mixed_01", MIXED).unwrap();
    assert_eq!(mixed.items().len(), 3);
    assert_eq!(mixed.items()[0].public_item().ordinal, 1);
    assert_eq!(mixed.items()[1].public_item().ordinal, 2);
    assert_eq!(mixed.items()[2].public_item().ordinal, 3);
    assert!(mixed.items()[0].public_item().file_size.is_none());
    assert!(mixed.items()[2].public_item().width.is_none());

    let public_items = mixed
        .items()
        .iter()
        .map(|item| item.public_item())
        .collect::<Vec<_>>();
    let public_json = serde_json::to_string(&public_items).unwrap();
    let lower = public_json.to_ascii_lowercase();
    assert!(!lower.contains("http"));
    assert!(!lower.contains("cookie"));
    assert!(!lower.contains("temp"));
    assert!(!lower.contains("cdninstagram"));
}

#[test]
fn repeated_analysis_uses_new_opaque_item_ids() {
    let first = normalize_instagram_inventory("Image_01", SINGLE_IMAGE).unwrap();
    let second = normalize_instagram_inventory("Image_01", SINGLE_IMAGE).unwrap();
    assert_ne!(
        first.items()[0].public_item().item_id,
        second.items()[0].public_item().item_id
    );
}

#[test]
fn rejects_malformed_duplicate_gap_cross_post_too_many_and_oversized_output() {
    assert_eq!(
        normalize_instagram_inventory("Broken_01", MALFORMED).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );

    let line = |shortcode: &str, ordinal: usize| {
        format!(
            r#"{{"category":"instagram","post_shortcode":"{shortcode}","post_url":"https://www.instagram.com/p/{shortcode}/","type":"post","description":"title","username":"user","num":{ordinal},"media_id":"m{ordinal}","display_url":"https://scontent.cdninstagram.com/{ordinal}.jpg?sig=x","video_url":null,"width":10,"height":20,"extension":"jpg"}}"#
        )
    };
    for payload in [
        format!("{}\n{}\n", line("Post_01", 1), line("Post_01", 1)),
        format!("{}\n{}\n", line("Post_01", 1), line("Post_01", 3)),
    ] {
        assert_eq!(
            normalize_instagram_inventory("Post_01", payload.as_bytes()).err(),
            Some(AnalysisErrorCode::InvalidMetadata)
        );
    }
    assert_eq!(
        normalize_instagram_inventory("Post_01", line("Other_01", 1).as_bytes()).err(),
        Some(AnalysisErrorCode::ContentChanged)
    );
    let thirty_three = (1..=33)
        .map(|ordinal| line("Post_01", ordinal))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        normalize_instagram_inventory("Post_01", thirty_three.as_bytes()).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
    assert_eq!(
        normalize_instagram_inventory("Post_01", &vec![b' '; 2_097_153]).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
}

#[test]
fn rejects_identity_queries_non_instagram_output_bad_media_urls_and_video_without_source() {
    let cases = [
        r#"{"category":"instagram","post_shortcode":"Post_01","post_url":"https://www.instagram.com/p/Post_01/?next=evil","type":"post","description":"../../Windows/System32","username":"user","num":1,"media_id":"1","display_url":"https://scontent.cdninstagram.com/1.jpg?sig=private","video_url":null,"width":10,"height":20,"extension":"jpg"}"#,
        r#"{"category":"other","post_shortcode":"Post_01","post_url":"https://www.instagram.com/p/Post_01/","type":"post","description":"title","username":"user","num":1,"media_id":"1","display_url":"https://scontent.cdninstagram.com/1.jpg","video_url":null,"width":10,"height":20,"extension":"jpg"}"#,
        r#"{"category":"instagram","post_shortcode":"Post_01","post_url":"https://www.instagram.com/p/Post_01/","type":"post","description":"title","username":"user","num":1,"media_id":"1","display_url":"http://127.0.0.1/1.jpg","video_url":null,"width":10,"height":20,"extension":"jpg"}"#,
        r#"{"category":"instagram","post_shortcode":"Post_01","post_url":"https://www.instagram.com/p/Post_01/","type":"post","description":"title","username":"user","num":1,"media_id":"1","display_url":"https://scontent.cdninstagram.com/cover.jpg","video_url":null,"width":10,"height":20,"extension":"mp4"}"#,
    ];
    for payload in cases {
        assert_eq!(
            normalize_instagram_inventory("Post_01", payload.as_bytes()).err(),
            Some(AnalysisErrorCode::InvalidMetadata),
            "{payload}"
        );
    }
}

#[test]
fn post_inventory_rejects_reel_kind_and_reel_identity_url() {
    for payload in [
        r#"{"category":"instagram","post_shortcode":"Post_01","post_url":"https://www.instagram.com/p/Post_01/","type":"reel","description":"title","username":"user","num":1,"media_id":"1","display_url":"https://scontent.cdninstagram.com/1.jpg","video_url":null,"width":10,"height":20,"extension":"jpg"}"#,
        r#"{"category":"instagram","post_shortcode":"Post_01","post_url":"https://www.instagram.com/reel/Post_01/","type":"post","description":"title","username":"user","num":1,"media_id":"1","display_url":"https://scontent.cdninstagram.com/1.jpg","video_url":null,"width":10,"height":20,"extension":"jpg"}"#,
    ] {
        assert_eq!(
            normalize_instagram_inventory("Post_01", payload.as_bytes()).err(),
            Some(AnalysisErrorCode::InvalidMetadata),
            "{payload}"
        );
    }
}

#[test]
fn captions_are_display_only_and_never_become_a_path_or_source() {
    let payload = br#"{"category":"instagram","post_shortcode":"Post_01","post_url":"https://www.instagram.com/p/Post_01/","type":"post","description":"../../Windows/System32","username":"user","num":1,"media_id":"1","display_url":"https://scontent.cdninstagram.com/1.png?sig=private","video_url":null,"width":10,"height":20,"extension":"png"}"#;
    let inventory = normalize_instagram_inventory("Post_01", payload).unwrap();
    assert_eq!(inventory.title(), "../../Windows/System32");
    assert_eq!(inventory.items()[0].public_item().container, "png");
}

#[test]
fn preserves_order_for_image_only_and_video_only_carousels() {
    let line = |shortcode: &str, ordinal: u16, video: bool| {
        let extension = if video { "mp4" } else { "jpg" };
        let video_url = if video {
            format!(r#""https://scontent.cdninstagram.com/{ordinal}.mp4?sig=x""#)
        } else {
            "null".to_owned()
        };
        format!(
            r#"{{"category":"instagram","post_shortcode":"{shortcode}","post_url":"https://www.instagram.com/p/{shortcode}/","type":"post","description":"carousel","username":"user","num":{ordinal},"media_id":"m{ordinal}","display_url":"https://scontent.cdninstagram.com/{ordinal}.jpg?sig=x","video_url":{video_url},"width":1080,"height":1080,"extension":"{extension}"}}"#
        )
    };
    for (shortcode, video, expected_kind) in [
        ("Images_01", false, InstagramMediaKind::Image),
        ("Videos_01", true, InstagramMediaKind::Video),
    ] {
        let jsonl = (1..=3)
            .map(|ordinal| line(shortcode, ordinal, video))
            .collect::<Vec<_>>()
            .join("\n");
        let inventory = normalize_instagram_inventory(shortcode, jsonl.as_bytes()).unwrap();
        assert_eq!(inventory.items().len(), 3);
        for (index, item) in inventory.items().iter().enumerate() {
            assert_eq!(item.public_item().ordinal, (index + 1) as u16);
            assert_eq!(item.public_item().kind, expected_kind);
        }
    }
}
