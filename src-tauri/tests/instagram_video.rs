use local_media_downloader_lib::{
    application::{
        instagram_video::{resolve_instagram_video, InstagramVideoSelector},
        media_url::validate_media_url,
    },
    domain::{
        media::AnalysisErrorCode,
        output_policy::{AudioProcessing, OutputContainer, StreamProcessing},
    },
    infrastructure::ytdlp::normalize_instagram_video,
};

const REEL: &str = include_str!("../../tests/fixtures/ytdlp/instagram-reel.json");
const POST: &str = include_str!("../../tests/fixtures/ytdlp/instagram-post-mixed.json");
const CHANGED: &str = include_str!("../../tests/fixtures/ytdlp/instagram-post-changed.json");

#[test]
fn reel_selects_highest_video_and_audio_without_default_transcoding() {
    let source = validate_media_url("https://www.instagram.com/reel/Reel_01/").unwrap();
    let selector = InstagramVideoSelector::reel();
    let media = normalize_instagram_video(&source, &selector, REEL).unwrap();
    let resolved = resolve_instagram_video(media, selector, false).unwrap();
    assert_eq!(resolved.plan().video_format_id.as_deref(), Some("av1-2160"));
    assert_eq!(
        resolved.plan().audio_format_id.as_deref(),
        Some("opus-audio")
    );
    assert_eq!(
        resolved.plan().output_container.as_deref(),
        Some(OutputContainer::Mp4.as_str())
    );
    assert_eq!(
        resolved.plan().video_processing,
        Some(StreamProcessing::Copy)
    );
    assert_eq!(
        resolved.plan().audio_processing,
        Some(AudioProcessing::Copy)
    );
    assert!(!resolved.plan().compatibility_mode);
    assert!(resolved.plan().clip.is_none());
}

#[test]
fn explicit_compatibility_is_the_only_instagram_video_transcode_path() {
    let source = validate_media_url("https://www.instagram.com/reel/Reel_01/").unwrap();
    let selector = InstagramVideoSelector::reel();
    let media = normalize_instagram_video(&source, &selector, REEL).unwrap();
    let resolved = resolve_instagram_video(media, selector, true).unwrap();
    assert_eq!(resolved.plan().output_container.as_deref(), Some("mp4"));
    assert_eq!(resolved.plan().output_video_codec.as_deref(), Some("h264"));
    assert_eq!(
        resolved.plan().video_processing,
        Some(StreamProcessing::Transcode)
    );
    assert_eq!(
        resolved.plan().audio_processing,
        Some(AudioProcessing::Transcode)
    );
}

#[test]
fn automatic_container_priority_falls_back_without_selecting_a_lower_video() {
    let source = validate_media_url("https://www.instagram.com/reel/Reel_01/").unwrap();
    for (audio_codec, expected_container) in [("opus", "webm"), ("mp4a.40.2", "mkv")] {
        let mut value: serde_json::Value = serde_json::from_str(REEL).unwrap();
        value["formats"][1]["vcodec"] = serde_json::json!("vp8");
        value["formats"][2]["acodec"] = serde_json::json!(audio_codec);
        let json = serde_json::to_string(&value).unwrap();
        let selector = InstagramVideoSelector::reel();
        let media = normalize_instagram_video(&source, &selector, &json).unwrap();
        let resolved = resolve_instagram_video(media, selector, false).unwrap();
        assert_eq!(resolved.plan().video_format_id.as_deref(), Some("av1-2160"));
        assert_eq!(
            resolved.plan().output_container.as_deref(),
            Some(expected_container)
        );
        assert_eq!(
            resolved.plan().video_processing,
            Some(StreamProcessing::Copy)
        );
    }
}

#[test]
fn post_item_requires_exact_parent_ordinal_and_encoded_media_identity() {
    let source = validate_media_url("https://www.instagram.com/p/Post_01/").unwrap();
    let selector = InstagramVideoSelector::post_item(3, "1").unwrap();
    let media = normalize_instagram_video(&source, &selector, POST).unwrap();
    let resolved = resolve_instagram_video(media, selector, false).unwrap();
    assert_eq!(resolved.plan().video_format_id.as_deref(), Some("vp9-1080"));
    assert_eq!(
        resolved.plan().audio_format_id.as_deref(),
        Some("opus-audio")
    );
    assert_eq!(resolved.plan().output_container.as_deref(), Some("mp4"));
    assert_eq!(resolved.selector().playlist_item(), Some(3));

    for (selector, json) in [
        (InstagramVideoSelector::post_item(2, "1").unwrap(), POST),
        (InstagramVideoSelector::post_item(3, "1").unwrap(), CHANGED),
        (InstagramVideoSelector::post_item(3, "2").unwrap(), POST),
    ] {
        assert_eq!(
            normalize_instagram_video(&source, &selector, json).err(),
            Some(AnalysisErrorCode::ContentChanged)
        );
    }
}

#[test]
fn rejects_cross_kind_non_video_and_unprovable_post_metadata() {
    let reel = validate_media_url("https://www.instagram.com/reel/Reel_01/").unwrap();
    let post = validate_media_url("https://www.instagram.com/p/Post_01/").unwrap();
    assert_eq!(
        normalize_instagram_video(
            &reel,
            &InstagramVideoSelector::post_item(3, "1").unwrap(),
            POST,
        )
        .err(),
        Some(AnalysisErrorCode::UnsupportedMedia)
    );
    assert_eq!(
        normalize_instagram_video(&post, &InstagramVideoSelector::reel(), REEL).err(),
        Some(AnalysisErrorCode::UnsupportedMedia)
    );
    let image = r#"{"extractor_key":"Instagram","id":"B","title":"Image","playlist_id":"Post_01","playlist_index":3,"playlist_count":3,"formats":[{"format_id":"image","ext":"jpg","vcodec":"none","acodec":"none"}]}"#;
    assert_eq!(
        normalize_instagram_video(
            &post,
            &InstagramVideoSelector::post_item(3, "1").unwrap(),
            image,
        )
        .err(),
        Some(AnalysisErrorCode::FormatUnavailable)
    );
}
