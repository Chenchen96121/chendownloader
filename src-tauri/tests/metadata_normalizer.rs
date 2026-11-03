use local_media_downloader_lib::{
    domain::media::{AnalysisErrorCode, Platform, StreamKind},
    infrastructure::ytdlp::{normalize_metadata, preview_fixture},
};
use serde_json::{json, Value};

fn base() -> Value {
    json!({"id":"sample","title":"Sample video","formats":[
        {"format_id":"v1","ext":"mp4","vcodec":"avc1","acodec":"none"}
    ]})
}

fn normalize(
    value: &Value,
) -> Result<local_media_downloader_lib::domain::media::MediaInfo, AnalysisErrorCode> {
    normalize_metadata(Platform::Youtube, &value.to_string())
}

#[test]
fn youtube_projects_literal_stream_fields_and_preserves_order() {
    let media = normalize_metadata(
        Platform::Youtube,
        include_str!("../../tests/fixtures/ytdlp/youtube.json"),
    )
    .unwrap();
    assert_eq!(media.platform, Platform::Youtube);
    assert_eq!(media.media_id, "sample-youtube");
    assert_eq!(media.title, "YouTube sample video");
    assert_eq!(media.creator.as_deref(), Some("Sample creator"));
    assert_eq!(media.duration_seconds, Some(125.0));
    assert_eq!(media.formats.len(), 3);
    assert_eq!(
        media
            .formats
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["v4k", "a1", "c720"]
    );
    let video = &media.formats[0];
    assert_eq!(video.kind, StreamKind::VideoOnly);
    assert_eq!(video.container, "mp4");
    assert_eq!(video.width, Some(3840));
    assert_eq!(video.height, Some(2160));
    assert_eq!(video.fps, Some(60.0));
    assert_eq!(video.video_bitrate_kbps, Some(18_000.5));
    assert_eq!(video.audio_bitrate_kbps, None);
    assert_eq!(video.audio_sample_rate_hz, None);
    assert_eq!(video.audio_channels, None);
    assert_eq!(video.video_codec.as_deref(), Some("av01.0.12M.08"));
    assert_eq!(video.audio_codec, None);
    assert_eq!(video.dynamic_range.as_deref(), Some("HDR10"));
    assert_eq!(video.file_size.as_ref().unwrap().bytes, 41943040);
    assert!(!video.file_size.as_ref().unwrap().is_approximate);
    assert_eq!(media.formats[1].kind, StreamKind::AudioOnly);
    assert_eq!(media.formats[1].container, "m4a");
    assert_eq!(media.formats[1].audio_codec.as_deref(), Some("mp4a.40.2"));
    assert_eq!(media.formats[1].video_bitrate_kbps, None);
    assert_eq!(media.formats[1].audio_bitrate_kbps, Some(192.0));
    assert_eq!(media.formats[1].audio_sample_rate_hz, Some(48_000));
    assert_eq!(media.formats[1].audio_channels, Some(2));
    assert_eq!(media.formats[1].file_size.as_ref().unwrap().bytes, 2097152);
    assert!(media.formats[1].file_size.as_ref().unwrap().is_approximate);
    assert_eq!(media.formats[2].kind, StreamKind::Combined);
    assert_eq!(media.formats[2].width, Some(1280));
    assert_eq!(media.formats[2].height, Some(720));
    assert_eq!(media.formats[2].fps, Some(30.0));
    assert_eq!(media.formats[2].video_bitrate_kbps, Some(5_000.0));
    assert_eq!(media.formats[2].audio_bitrate_kbps, Some(128.0));
    assert_eq!(media.formats[2].audio_sample_rate_hz, Some(44_100));
    assert_eq!(media.formats[2].audio_channels, Some(2));
    assert_eq!(media.formats[2].file_size, None);
}

#[test]
fn bilibili_projects_literal_hevc_and_aac_streams() {
    let media = normalize_metadata(
        Platform::Bilibili,
        include_str!("../../tests/fixtures/ytdlp/bilibili.json"),
    )
    .unwrap();
    assert_eq!(media.platform, Platform::Bilibili);
    assert_eq!(media.media_id, "sample-bilibili");
    assert_eq!(media.title, "Bilibili sample video");
    assert_eq!(media.creator.as_deref(), Some("Sample creator"));
    assert_eq!(media.duration_seconds, Some(90.0));
    assert_eq!(media.formats.len(), 2);
    assert_eq!(media.formats[0].id, "v1080");
    assert_eq!(media.formats[0].kind, StreamKind::VideoOnly);
    assert_eq!(media.formats[0].width, Some(1920));
    assert_eq!(media.formats[0].height, Some(1080));
    assert_eq!(media.formats[0].fps, Some(60.0));
    assert_eq!(media.formats[0].video_bitrate_kbps, Some(8_000.25));
    assert_eq!(
        media.formats[0].video_codec.as_deref(),
        Some("hev1.1.6.L120")
    );
    assert_eq!(media.formats[0].dynamic_range.as_deref(), Some("SDR"));
    assert_eq!(media.formats[1].id, "a1");
    assert_eq!(media.formats[1].kind, StreamKind::AudioOnly);
    assert_eq!(media.formats[1].audio_codec.as_deref(), Some("mp4a.40.2"));
    assert_eq!(media.formats[1].audio_bitrate_kbps, Some(160.0));
    assert_eq!(media.formats[1].audio_sample_rate_hz, Some(48_000));
    assert_eq!(media.formats[1].audio_channels, Some(2));
}

#[test]
fn x_omits_creator_duration_and_unknown_size() {
    let media = normalize_metadata(
        Platform::X,
        include_str!("../../tests/fixtures/ytdlp/x.json"),
    )
    .unwrap();
    assert_eq!(media.platform, Platform::X);
    assert_eq!(media.media_id, "sample-x");
    assert_eq!(media.title, "X sample video");
    assert_eq!(media.creator, None);
    assert_eq!(media.duration_seconds, None);
    assert_eq!(media.formats.len(), 1);
    assert_eq!(media.formats[0].kind, StreamKind::Combined);
    assert_eq!(media.formats[0].container, "mp4");
    assert_eq!(media.formats[0].video_codec.as_deref(), Some("avc1.64001f"));
    assert_eq!(media.formats[0].audio_codec.as_deref(), Some("mp4a.40.2"));
    assert_eq!(media.formats[0].width, Some(1280));
    assert_eq!(media.formats[0].height, Some(720));
    assert_eq!(media.formats[0].video_bitrate_kbps, Some(4_500.0));
    assert_eq!(media.formats[0].audio_bitrate_kbps, Some(128.0));
    assert_eq!(media.formats[0].audio_sample_rate_hz, Some(44_100));
    assert_eq!(media.formats[0].audio_channels, Some(2));
    let output = serde_json::to_value(media).unwrap();
    assert!(output.get("creator").is_none());
    assert!(output.get("durationSeconds").is_none());
    assert!(output["formats"][0].get("fileSize").is_none());
}

#[test]
fn instagram_projects_literal_portrait_combined_stream() {
    let media = normalize_metadata(
        Platform::Instagram,
        include_str!("../../tests/fixtures/ytdlp/instagram.json"),
    )
    .unwrap();
    assert_eq!(media.platform, Platform::Instagram);
    assert_eq!(media.media_id, "sample-instagram");
    assert_eq!(media.title, "Instagram sample video");
    assert_eq!(media.creator.as_deref(), Some("Sample creator"));
    assert_eq!(media.duration_seconds, Some(30.0));
    assert_eq!(media.formats.len(), 1);
    assert_eq!(media.formats[0].kind, StreamKind::Combined);
    assert_eq!(media.formats[0].container, "mp4");
    assert_eq!(media.formats[0].video_codec.as_deref(), Some("avc1.64001f"));
    assert_eq!(media.formats[0].audio_codec.as_deref(), Some("mp4a.40.2"));
    assert_eq!(media.formats[0].width, Some(1080));
    assert_eq!(media.formats[0].height, Some(1920));
    assert_eq!(media.formats[0].fps, Some(30.0));
    assert_eq!(media.formats[0].video_bitrate_kbps, Some(6_000.0));
    assert_eq!(media.formats[0].audio_bitrate_kbps, Some(128.0));
    assert_eq!(media.formats[0].audio_sample_rate_hz, Some(48_000));
    assert_eq!(media.formats[0].audio_channels, Some(2));
    assert_eq!(media.formats[0].file_size.unwrap().bytes, 4194304);
    assert!(media.formats[0].file_size.unwrap().is_approximate);
}

#[test]
fn preview_selects_all_four_static_samples_without_tools() {
    for (platform, id, title, count) in [
        (
            Platform::Youtube,
            "sample-youtube",
            "YouTube sample video",
            3,
        ),
        (
            Platform::Bilibili,
            "sample-bilibili",
            "Bilibili sample video",
            2,
        ),
        (Platform::X, "sample-x", "X sample video", 1),
        (
            Platform::Instagram,
            "sample-instagram",
            "Instagram sample video",
            1,
        ),
    ] {
        let media = preview_fixture(platform).unwrap();
        assert_eq!(media.platform, platform);
        assert_eq!(media.media_id, id);
        assert_eq!(media.title, title);
        assert_eq!(media.formats.len(), count);
        assert_eq!(media.validate(), Ok(()));
    }
}

#[test]
fn trims_display_text_without_inventing_missing_values() {
    let mut value = base();
    value["title"] = json!("  Sample video  ");
    value["uploader"] = json!("  Sample creator  ");
    value["duration"] = json!(0);
    let media = normalize(&value).unwrap();
    assert_eq!(media.title, "Sample video");
    assert_eq!(media.creator.as_deref(), Some("Sample creator"));
    assert_eq!(media.duration_seconds, Some(0.0));
}

#[test]
fn exact_size_precedes_estimate_and_invalid_exact_falls_back() {
    for (exact, approximate, bytes, estimated) in [
        (json!(100), json!(200), Some(100), false),
        (json!(0), json!(200), Some(200), true),
        (json!(9007199254740992u64), json!(200), Some(200), true),
        (json!(1.5), json!(200), Some(200), true),
        (json!("100"), json!(200), Some(200), true),
        (json!(100), json!(null), Some(100), false),
        (json!(-1), json!(0), None, false),
        (
            json!(9007199254740991u64),
            json!(200),
            Some(9007199254740991),
            false,
        ),
    ] {
        let mut value = base();
        value["formats"][0]["filesize"] = exact;
        value["formats"][0]["filesize_approx"] = approximate;
        let size = normalize(&value).unwrap().formats[0].file_size;
        assert_eq!(size.map(|size| size.bytes), bytes);
        if let Some(size) = size {
            assert_eq!(size.is_approximate, estimated);
        }
    }
}

#[test]
fn invalid_optional_values_are_omitted() {
    for (field, invalid) in [
        ("uploader", json!(" ")),
        ("uploader", json!("bad\ncreator")),
        ("uploader", json!("x".repeat(257))),
        ("uploader", json!(42)),
        ("duration", json!(-1)),
        ("duration", json!(31536001)),
        ("duration", json!("12")),
    ] {
        let mut value = base();
        value[field] = invalid;
        let media = normalize(&value).unwrap();
        assert_eq!(media.creator, None);
        assert_eq!(media.duration_seconds, None);
    }
    for (field, invalid, output_field) in [
        ("width", json!(0), "width"),
        ("width", json!(16385), "width"),
        ("width", json!(1.5), "width"),
        ("height", json!(-1), "height"),
        ("height", json!("720"), "height"),
        ("fps", json!(0), "fps"),
        ("fps", json!(240.1), "fps"),
        ("fps", json!("30"), "fps"),
        ("dynamic_range", json!("unknown"), "dynamicRange"),
        ("dynamic_range", json!(42), "dynamicRange"),
        ("acodec", json!("unsafe/token"), "audioCodec"),
        ("acodec", json!("x".repeat(65)), "audioCodec"),
        ("acodec", json!(42), "audioCodec"),
    ] {
        let mut value = base();
        value["formats"][0][field] = invalid;
        let output = serde_json::to_value(normalize(&value).unwrap()).unwrap();
        assert!(output["formats"][0].get(output_field).is_none(), "{field}");
    }

    for invalid in [json!(0), json!(-1), json!(1_000_000.1), json!("18000.5")] {
        let mut value = base();
        value["formats"][0]["vbr"] = invalid;
        let media = normalize(&value).unwrap();
        assert_eq!(media.formats.len(), 1);
        assert_eq!(media.formats[0].id, "v1");
        assert_eq!(media.formats[0].video_bitrate_kbps, None);
    }
    for invalid in [json!(0), json!(-1), json!(100_000.1), json!("192")] {
        let mut value = base();
        value["formats"][0]["acodec"] = json!("aac");
        value["formats"][0]["abr"] = invalid;
        let media = normalize(&value).unwrap();
        assert_eq!(media.formats.len(), 1);
        assert_eq!(media.formats[0].id, "v1");
        assert_eq!(media.formats[0].audio_bitrate_kbps, None);
    }
    for (field, output_field, invalid_values) in [
        (
            "asr",
            "audioSampleRateHz",
            vec![
                json!(0),
                json!(-1),
                json!(768_001),
                json!(48_000.5),
                json!("48000"),
            ],
        ),
        (
            "audio_channels",
            "audioChannels",
            vec![json!(0), json!(-1), json!(65), json!(2.5), json!("2")],
        ),
    ] {
        for invalid in invalid_values {
            let mut value = base();
            value["formats"][0]["acodec"] = json!("aac");
            value["formats"][0][field] = invalid;
            let media = normalize(&value).unwrap();
            assert_eq!(media.formats.len(), 1);
            assert_eq!(media.formats[0].id, "v1");
            let output = serde_json::to_value(media).unwrap();
            assert!(output["formats"][0].get(output_field).is_none(), "{field}");
        }
    }
}

#[test]
fn valid_optional_boundaries_and_safe_codec_alphabet_are_retained() {
    let mut value = base();
    value["uploader"] = json!("x".repeat(256));
    value["duration"] = json!(31536000);
    value["formats"][0]["width"] = json!(1);
    value["formats"][0]["height"] = json!(16384);
    value["formats"][0]["fps"] = json!(240);
    value["formats"][0]["acodec"] = json!("A1._+-");
    for range in ["SDR", "HDR", "HDR10", "HDR10+", "HDR12", "HLG", "DV"] {
        value["formats"][0]["dynamic_range"] = json!(range);
        let media = normalize(&value).unwrap();
        let row = &media.formats[0];
        assert_eq!(media.creator.as_ref().unwrap().len(), 256);
        assert_eq!(media.duration_seconds, Some(31536000.0));
        assert_eq!(row.width, Some(1));
        assert_eq!(row.height, Some(16384));
        assert_eq!(row.fps, Some(240.0));
        assert_eq!(row.dynamic_range.as_deref(), Some(range));
        assert_eq!(row.audio_codec.as_deref(), Some("A1._+-"));
    }
}

#[test]
fn valid_media_metric_boundaries_are_retained_without_inference() {
    for (vbr, abr, asr, channels) in [(1.0, 1.0, 1, 1), (1_000_000.0, 100_000.0, 768_000, 64)] {
        let mut value = base();
        value["formats"][0]["acodec"] = json!("aac");
        value["formats"][0]["vbr"] = json!(vbr);
        value["formats"][0]["abr"] = json!(abr);
        value["formats"][0]["asr"] = json!(asr);
        value["formats"][0]["audio_channels"] = json!(channels);
        let row = normalize(&value).unwrap().formats.remove(0);
        assert_eq!(row.video_bitrate_kbps, Some(vbr));
        assert_eq!(row.audio_bitrate_kbps, Some(abr));
        assert_eq!(row.audio_sample_rate_hz, Some(asr));
        assert_eq!(row.audio_channels, Some(channels));
    }

    let mut value = base();
    value["duration"] = json!(125);
    value["formats"][0]["tbr"] = json!(5500);
    value["formats"][0]["filesize"] = json!(41943040);
    value["formats"].as_array_mut().unwrap().push(json!({
        "format_id":"c2","ext":"mp4","vcodec":"avc1","acodec":"aac",
        "vbr":2500,"abr":128,"asr":44100,"audio_channels":2
    }));
    let media = normalize(&value).unwrap();
    assert_eq!(media.formats[0].id, "v1");
    assert_eq!(media.formats[0].video_bitrate_kbps, None);
    assert_eq!(media.formats[0].audio_bitrate_kbps, None);
    assert_eq!(media.formats[0].audio_sample_rate_hz, None);
    assert_eq!(media.formats[0].audio_channels, None);
    assert_eq!(media.formats[1].video_bitrate_kbps, Some(2_500.0));
}

#[test]
fn audio_only_strips_all_video_attributes() {
    let mut value = base();
    value["formats"][0] = json!({"format_id":"a","ext":"m4a","vcodec":"none","acodec":"aac",
        "width":1920,"height":1080,"fps":60,"dynamic_range":"HDR10","vbr":18000,
        "abr":192,"asr":48000,"audio_channels":2});
    let media = normalize(&value).unwrap();
    let row = &media.formats[0];
    assert_eq!(row.kind, StreamKind::AudioOnly);
    assert_eq!(row.width, None);
    assert_eq!(row.height, None);
    assert_eq!(row.fps, None);
    assert_eq!(row.video_bitrate_kbps, None);
    assert_eq!(row.audio_bitrate_kbps, Some(192.0));
    assert_eq!(row.audio_sample_rate_hz, Some(48_000));
    assert_eq!(row.audio_channels, Some(2));
    assert_eq!(row.dynamic_range, None);
    let output = serde_json::to_value(media).unwrap();
    for field in [
        "width",
        "height",
        "fps",
        "dynamicRange",
        "videoBitrateKbps",
        "videoCodec",
    ] {
        assert!(output["formats"][0].get(field).is_none());
    }
}

#[test]
fn video_only_strips_all_audio_metrics() {
    let mut value = base();
    value["formats"][0]["vbr"] = json!(18000);
    value["formats"][0]["abr"] = json!(192);
    value["formats"][0]["asr"] = json!(48000);
    value["formats"][0]["audio_channels"] = json!(2);
    let row = normalize(&value).unwrap().formats.remove(0);
    assert_eq!(row.kind, StreamKind::VideoOnly);
    assert_eq!(row.video_bitrate_kbps, Some(18_000.0));
    assert_eq!(row.audio_bitrate_kbps, None);
    assert_eq!(row.audio_sample_rate_hz, None);
    assert_eq!(row.audio_channels, None);
}

#[test]
fn skips_invalid_required_rows_storyboards_drm_and_empty_streams_in_order() {
    let invalid_rows = [
        json!(null),
        json!(42),
        json!([]),
        json!({}),
        json!({"format_id":"bad/id","ext":"mp4","vcodec":"avc1"}),
        json!({"format_id":"x".repeat(65),"ext":"mp4","vcodec":"avc1"}),
        json!({"format_id":"bad","ext":"MP4","vcodec":"avc1"}),
        json!({"format_id":"bad","ext":"x".repeat(17),"vcodec":"avc1"}),
        json!({"format_id":"bad","ext":"","vcodec":"avc1"}),
        json!({"format_id":"bad","ext":"mp4","vcodec":"images","acodec":"aac"}),
        json!({"format_id":"bad","ext":"mp4","acodec":"images","vcodec":"avc1"}),
        json!({"format_id":"bad","ext":"mp4","vcodec":"storyboard","acodec":"aac"}),
        json!({"format_id":"bad","ext":"mp4","vcodec":"avc1","has_drm":true}),
        json!({"format_id":"bad","ext":"mp4","vcodec":"none","acodec":"none"}),
        json!({"format_id":"bad","ext":"mp4","vcodec":"bad/token"}),
    ];
    let mut value = base();
    let rows = value["formats"].as_array_mut().unwrap();
    rows.extend(invalid_rows);
    rows.push(json!({"format_id":"a2","ext":"m4a","acodec":"aac"}));
    let media = normalize(&value).unwrap();
    assert_eq!(
        media
            .formats
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["v1", "a2"]
    );
}

#[test]
fn duplicate_usable_ids_reject_but_skipped_ids_do_not() {
    let mut value = base();
    value["formats"]
        .as_array_mut()
        .unwrap()
        .push(json!({"format_id":"v1","ext":"mp4","vcodec":"avc1"}));
    assert_eq!(normalize(&value), Err(AnalysisErrorCode::InvalidMetadata));
    value["formats"][1]["has_drm"] = json!(true);
    assert_eq!(normalize(&value).unwrap().formats.len(), 1);
}

#[test]
fn rejects_malformed_root_and_formats_shapes() {
    for input in [
        "{",
        "{} trailing",
        "{} {}",
        "[]",
        "null",
        "42",
        "\"sample\"",
        "",
    ] {
        assert_eq!(
            normalize_metadata(Platform::Youtube, input),
            Err(AnalysisErrorCode::InvalidMetadata)
        );
    }
    for shape in [json!(null), json!({}), json!("formats"), json!(42)] {
        let mut value = base();
        value["formats"] = shape;
        assert_eq!(normalize(&value), Err(AnalysisErrorCode::InvalidMetadata));
    }
    let mut value = base();
    value.as_object_mut().unwrap().remove("formats");
    assert_eq!(normalize(&value), Err(AnalysisErrorCode::InvalidMetadata));
}

#[test]
fn invalid_required_media_id_and_title_reject() {
    for (field, invalid) in [
        ("id", json!(null)),
        ("id", json!(42)),
        ("id", json!("")),
        ("id", json!("bad/id")),
        ("id", json!(" sample")),
        ("id", json!("é")),
        ("id", json!("x".repeat(129))),
        ("title", json!(null)),
        ("title", json!(42)),
        ("title", json!(" ")),
        ("title", json!("bad\nvideo")),
        ("title", json!("x".repeat(513))),
    ] {
        let mut value = base();
        value[field] = invalid;
        assert_eq!(
            normalize(&value),
            Err(AnalysisErrorCode::InvalidMetadata),
            "{field}"
        );
    }
    for field in ["id", "title"] {
        let mut value = base();
        value.as_object_mut().unwrap().remove(field);
        assert_eq!(normalize(&value), Err(AnalysisErrorCode::InvalidMetadata));
    }
    let mut value = base();
    value["id"] = json!("A1._+-".repeat(21));
    value["title"] = json!("界".repeat(512));
    assert_eq!(normalize(&value).unwrap().title.chars().count(), 512);
}

#[test]
fn rejects_playlists_entries_live_upcoming_postlive_and_top_level_drm() {
    for (field, unsupported) in [
        ("_type", json!("playlist")),
        ("_type", json!("multi_video")),
        ("_type", json!(42)),
        ("entries", json!([])),
        ("entries", json!(null)),
        ("is_live", json!(true)),
        ("live_status", json!("is_live")),
        ("live_status", json!("is_upcoming")),
        ("live_status", json!("post_live")),
        ("has_drm", json!(true)),
    ] {
        let mut value = base();
        value[field] = unsupported;
        assert_eq!(
            normalize(&value),
            Err(AnalysisErrorCode::UnsupportedMedia),
            "{field}"
        );
    }
    for status in ["not_live", "was_live"] {
        let mut value = base();
        value["_type"] = json!("video");
        value["live_status"] = json!(status);
        value["has_drm"] = json!(false);
        assert!(normalize(&value).is_ok());
    }
}

#[test]
fn no_usable_formats_returns_format_unavailable() {
    for rows in [
        json!([]),
        json!([null, {}, {"format_id":"s","ext":"mhtml","vcodec":"images"}]),
    ] {
        let mut value = base();
        value["formats"] = rows;
        assert_eq!(normalize(&value), Err(AnalysisErrorCode::FormatUnavailable));
    }
}

#[test]
fn rejects_oversized_input_before_parsing_and_raw_list_before_filtering() {
    assert_eq!(
        normalize_metadata(Platform::Youtube, &" ".repeat(1048577)),
        Err(AnalysisErrorCode::InvalidMetadata)
    );
    let mut value = base();
    value["discarded"] = json!("界".repeat(349526));
    assert_eq!(normalize(&value), Err(AnalysisErrorCode::InvalidMetadata));
    value = base();
    value["formats"] = json!(vec![Value::Null; 513]);
    assert_eq!(normalize(&value), Err(AnalysisErrorCode::InvalidMetadata));
    value["formats"] = json!((0..512)
        .map(|index| json!({"format_id":format!("v{index}"),"ext":"mp4","vcodec":"avc1"}))
        .collect::<Vec<_>>());
    assert_eq!(normalize(&value).unwrap().formats.len(), 512);
    let input = base().to_string();
    let at_bound = format!("{input}{}", " ".repeat(1048576 - input.len()));
    assert!(normalize_metadata(Platform::Youtube, &at_bound).is_ok());
}

#[test]
fn explicit_projection_excludes_all_discarded_neutral_sentinels() {
    let mut value = base();
    for field in [
        "url",
        "webpage_url",
        "thumbnail",
        "thumbnails",
        "http_headers",
        "filename",
        "filepath",
        "hash",
        "process_args",
        "raw_output",
    ] {
        value[field] = json!("discarded-neutral-sentinel");
        value["formats"][0][field] = json!("discarded-neutral-sentinel");
    }
    let output = serde_json::to_value(normalize(&value).unwrap()).unwrap();
    assert_eq!(
        output,
        json!({"platform":"youtube","mediaId":"sample","title":"Sample video","formats":[
            {"id":"v1","container":"mp4","kind":"video_only","videoCodec":"avc1"}
        ]})
    );
    assert!(!output.to_string().contains("discarded-neutral-sentinel"));
}
