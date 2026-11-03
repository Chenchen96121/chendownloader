use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        x_video::{match_x_video, resolve_x_video},
    },
    domain::{format_selection::SelectionMode, media::AnalysisErrorCode},
    infrastructure::x::inventory::parse_x_inventory,
};
use serde_json::{json, Value};
use std::sync::Arc;
fn inventory() -> local_media_downloader_lib::application::x_source::XInventory {
    parse_x_inventory(
        include_bytes!("../../fixtures/x/mixed-post.json"),
        &validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap(),
    )
    .unwrap()
}
fn metadata() -> Value {
    serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json")).unwrap()
}
#[test]
fn x_video_matches_stable_entities_not_all_attachment_ordinals() {
    let i = inventory();
    let v = resolve_x_video(
        match_x_video(&i, &i.items[1], &metadata()).unwrap(),
        Arc::new(NetworkProfile::current()),
        false,
    )
    .unwrap();
    assert_eq!(v.media().media_id, "10000000000000000002");
    assert_eq!(v.plan().video_format_id.as_deref(), Some("high"));
    assert_eq!(v.plan().audio_format_id.as_deref(), Some("high"));
    assert!(v.plan().clip.is_none());
    assert!(!v.plan().compatibility_mode);
    assert_eq!(
        v.plan().video_processing,
        Some(local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy)
    );
    assert_eq!(
        v.plan().audio_processing,
        Some(local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy)
    );
    assert_eq!(v.plan().output_container.as_deref(), Some("mp4"));
    assert_eq!(v.media().formats[1].video_codec.as_deref(), Some("vp9"));
    assert_eq!(v.media().formats[1].audio_codec.as_deref(), Some("opus"));
    let silent = resolve_x_video(
        match_x_video(&i, &i.items[3], &metadata()).unwrap(),
        Arc::new(NetworkProfile::current()),
        false,
    )
    .unwrap();
    assert_eq!(silent.plan().mode, SelectionMode::VideoOnly);
    assert!(silent.plan().audio_format_id.is_none());
    let compatible = resolve_x_video(
        match_x_video(&i, &i.items[1], &metadata()).unwrap(),
        Arc::new(NetworkProfile::current()),
        true,
    )
    .unwrap();
    assert!(compatible.plan().compatibility_mode);
    assert_eq!(
        compatible.plan().source_format_ids,
        v.plan().source_format_ids
    );
    assert_eq!(compatible.plan().output_container.as_deref(), Some("mp4"));
    assert_eq!(
        compatible.plan().video_processing,
        Some(local_media_downloader_lib::domain::output_policy::StreamProcessing::Transcode)
    );
}
#[test]
fn x_video_refuses_ambiguous_same_thumbnail_wrong_parent_and_omitted_high_stream() {
    let i = inventory();
    let mut wrong = metadata();
    wrong["entries"][1]["display_id"] = json!("9");
    assert!(match_x_video(&i, &i.items[1], &wrong).is_err());
    let mut wrong = metadata();
    let duplicate = wrong["entries"][1].clone();
    wrong["entries"].as_array_mut().unwrap().push(duplicate);
    assert!(match_x_video(&i, &i.items[1], &wrong).is_err());
    let mut wrong = metadata();
    wrong["entries"][1]["formats"].as_array_mut().unwrap().pop();
    assert!(match_x_video(&i, &i.items[1], &wrong).is_err());
    let mut wrong = metadata();
    wrong["entries"][1]["id"] = json!("8");
    wrong["entries"][1]["formats"][0]["url"] = json!("https://video.twimg.com/other.mp4");
    wrong["entries"][1]["formats"][1]["url"] = json!("https://video.twimg.com/other2.mp4");
    assert!(match_x_video(&i, &i.items[1], &wrong).is_err());
    let mut ambiguous = inventory();
    ambiguous.items[3].video_candidates = ambiguous.items[1].video_candidates.clone();
    assert!(match_x_video(&ambiguous, &ambiguous.items[1], &metadata()).is_err());
}
#[test]
fn x_unknown_high_codec_cannot_be_ignored_in_favor_of_known_low_quality() {
    let i = inventory();
    let mut raw = metadata();
    raw["entries"][1]["formats"][1]["vcodec"] = Value::Null;
    let matched = match_x_video(&i, &i.items[1], &raw).unwrap();
    assert!(matches!(
        resolve_x_video(matched, Arc::new(NetworkProfile::current()), false),
        Err(AnalysisErrorCode::FormatUnavailable)
    ));
}

#[test]
fn x_missing_tie_breaking_quality_facts_cannot_silently_rank_as_lower_quality() {
    let inventory = inventory();
    for missing in ["dynamic_range", "vbr", "audio_channels", "abr", "asr"] {
        let mut raw = metadata();
        let rows = raw["entries"][1]["formats"].as_array_mut().unwrap();
        for row in rows.iter_mut() {
            row["width"] = json!(1280);
            row["height"] = json!(720);
            row["fps"] = json!(30);
            row["vcodec"] = json!("h264");
            row["acodec"] = json!("aac");
            row["dynamic_range"] = json!("SDR");
            row["audio_channels"] = json!(2);
            row["asr"] = json!(48000);
            if missing == "asr" {
                row["abr"] = json!(128);
            }
        }
        rows[1][missing] = Value::Null;
        if missing == "vbr" {
            rows[1]["tbr"] = json!(9000);
        }
        assert!(
            matches!(
                resolve_x_video(
                    match_x_video(&inventory, &inventory.items[1], &raw).unwrap(),
                    Arc::new(NetworkProfile::current()),
                    false
                ),
                Err(AnalysisErrorCode::FormatUnavailable)
            ),
            "unproven {missing} was silently ranked below known facts"
        );
    }
}
#[test]
fn x_generated_download_json_excludes_reextraction_and_raw_headers() {
    let i = inventory();
    let mut raw = metadata();
    raw["entries"][1]["webpage_url"] = json!("https://x.com/other/status/1");
    raw["entries"][1]["http_headers"] = json!({"Cookie":"secret"});
    let video = resolve_x_video(
        match_x_video(&i, &i.items[1], &raw).unwrap(),
        Arc::new(NetworkProfile::current()),
        false,
    )
    .unwrap();
    let value: Value =
        serde_json::from_slice(&video.source().info_json(video.plan()).unwrap()).unwrap();
    assert!(value.get("webpage_url").is_none());
    assert!(value.get("extractor").is_none());
    assert!(value.get("http_headers").is_none());
    assert_eq!(
        value["formats"][0]["url"],
        "https://video.twimg.com/ext_tw_video/fixtureB/high.mp4"
    );
}
