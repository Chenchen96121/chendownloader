use local_media_downloader_lib::domain::media::{
    AnalysisErrorCode, FileSize, MediaFormat, MediaInfo, Platform, StreamKind,
};

fn sample() -> MediaInfo {
    MediaInfo {
        platform: Platform::Youtube,
        media_id: "sample-youtube".into(),
        title: "Sample video".into(),
        creator: None,
        duration_seconds: Some(125.0),
        formats: vec![MediaFormat {
            id: "v1".into(),
            container: "mp4".into(),
            kind: StreamKind::VideoOnly,
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_bitrate_kbps: None,
            audio_bitrate_kbps: None,
            audio_sample_rate_hz: None,
            audio_channels: None,
            video_codec: Some("avc1.640028".into()),
            audio_codec: None,
            dynamic_range: Some("SDR".into()),
            file_size: None,
        }],
    }
}

fn invalid(media: &MediaInfo) {
    assert_eq!(media.validate(), Err(AnalysisErrorCode::InvalidMetadata));
}

#[test]
fn serializes_only_contract_fields_with_camel_case_and_omitted_optionals() {
    let media = sample();
    assert!(media.validate().is_ok());
    assert_eq!(
        serde_json::to_value(&media).unwrap(),
        serde_json::json!({
            "platform": "youtube", "mediaId": "sample-youtube", "title": "Sample video",
            "durationSeconds": 125.0, "formats": [{
                "id": "v1", "container": "mp4", "kind": "video_only",
                "width": 1920, "height": 1080, "fps": 30.0,
                "videoCodec": "avc1.640028", "dynamicRange": "SDR"
            }]
        })
    );
}

#[test]
fn serializes_media_metrics_with_exact_camel_case_names() {
    let mut media = sample();
    let row = &mut media.formats[0];
    row.kind = StreamKind::Combined;
    row.audio_codec = Some("mp4a.40.2".into());
    row.video_bitrate_kbps = Some(5_500.5);
    row.audio_bitrate_kbps = Some(192.0);
    row.audio_sample_rate_hz = Some(48_000);
    row.audio_channels = Some(2);

    assert!(media.validate().is_ok());
    let json = serde_json::to_value(media).unwrap();
    assert_eq!(json["formats"][0]["videoBitrateKbps"], 5_500.5);
    assert_eq!(json["formats"][0]["audioBitrateKbps"], 192.0);
    assert_eq!(json["formats"][0]["audioSampleRateHz"], 48_000);
    assert_eq!(json["formats"][0]["audioChannels"], 2);
}

#[test]
fn serializes_all_closed_platform_kind_and_error_values() {
    for (platform, literal) in [
        (Platform::Youtube, "youtube"),
        (Platform::Bilibili, "bilibili"),
        (Platform::X, "x"),
        (Platform::Instagram, "instagram"),
    ] {
        assert_eq!(serde_json::to_value(platform).unwrap(), literal);
    }
    for (kind, literal) in [
        (StreamKind::VideoOnly, "video_only"),
        (StreamKind::AudioOnly, "audio_only"),
        (StreamKind::Combined, "combined"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), literal);
    }
    for (code, literal) in [
        (AnalysisErrorCode::InvalidUrl, "invalid_url"),
        (
            AnalysisErrorCode::UnsupportedPlatform,
            "unsupported_platform",
        ),
        (AnalysisErrorCode::UnsupportedMedia, "unsupported_media"),
        (AnalysisErrorCode::InvalidMetadata, "invalid_metadata"),
        (AnalysisErrorCode::FormatUnavailable, "format_unavailable"),
        (
            AnalysisErrorCode::AnalysisUnavailable,
            "analysis_unavailable",
        ),
        (AnalysisErrorCode::ToolMissing, "tool_missing"),
        (
            AnalysisErrorCode::ToolIntegrityFailed,
            "tool_integrity_failed",
        ),
        (
            AnalysisErrorCode::AuthenticationRequired,
            "authentication_required",
        ),
        (AnalysisErrorCode::AccessDenied, "access_denied"),
        (AnalysisErrorCode::ContentUnavailable, "content_unavailable"),
        (AnalysisErrorCode::RateLimited, "rate_limited"),
        (AnalysisErrorCode::ExtractorOutdated, "extractor_outdated"),
        (AnalysisErrorCode::NetworkFailed, "network_failed"),
        (AnalysisErrorCode::AnalysisTimeout, "analysis_timeout"),
        (AnalysisErrorCode::ProcessFailed, "process_failed"),
        (AnalysisErrorCode::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(code).unwrap(), literal);
    }
}

#[test]
fn accepts_unicode_text_bounds_and_optional_unknown_values() {
    let mut media = sample();
    media.media_id = "A0._+-".repeat(21);
    media.title = "界".repeat(512);
    media.creator = Some("人".repeat(256));
    media.duration_seconds = Some(31_536_000.0);
    media.formats[0].id = "A0._+-".repeat(10);
    media.formats[0].container = "a1".repeat(8);
    media.formats[0].video_codec = Some("a".repeat(64));
    media.formats[0].width = Some(16384);
    media.formats[0].height = Some(1);
    media.formats[0].fps = Some(240.0);
    media.formats[0].file_size = Some(FileSize {
        bytes: 9_007_199_254_740_991,
        is_approximate: true,
    });
    assert!(media.validate().is_ok());
    assert_eq!(
        serde_json::to_value(&media).unwrap()["formats"][0]["fileSize"],
        serde_json::json!({"bytes":9007199254740991_u64,"isApproximate":true})
    );
    media.creator = None;
    media.duration_seconds = None;
    media.formats[0].width = None;
    media.formats[0].height = None;
    media.formats[0].fps = None;
    media.formats[0].dynamic_range = None;
    assert!(media.validate().is_ok());
}

#[test]
fn rejects_invalid_required_identifiers_and_text() {
    for value in ["".to_owned(), "bad/id".into(), "界".into(), "a".repeat(129)] {
        let mut media = sample();
        media.media_id = value;
        invalid(&media);
    }
    for value in ["".to_owned(), "bad id".into(), "a".repeat(65)] {
        let mut media = sample();
        media.formats[0].id = value;
        invalid(&media);
    }
    for value in [
        "".to_owned(),
        " ".into(),
        " untrimmed".into(),
        "untrimmed ".into(),
        "bad\ntext".into(),
        "界".repeat(513),
    ] {
        let mut media = sample();
        media.title = value;
        invalid(&media);
    }
    for value in [
        "".to_owned(),
        " name".into(),
        "name ".into(),
        "bad\u{7f}name".into(),
        "人".repeat(257),
    ] {
        let mut media = sample();
        media.creator = Some(value);
        invalid(&media);
    }
}

#[test]
fn requires_one_to_512_uniquely_identified_formats() {
    let mut media = sample();
    media.formats.clear();
    invalid(&media);
    let mut media = sample();
    media.formats.push(media.formats[0].clone());
    invalid(&media);
    let mut media = sample();
    media.formats = (0..512)
        .map(|id| {
            let mut row = sample().formats.remove(0);
            row.id = id.to_string();
            row
        })
        .collect();
    assert!(media.validate().is_ok());
    let mut row = sample().formats.remove(0);
    row.id = "extra".into();
    media.formats.push(row);
    invalid(&media);
}

#[test]
fn requires_kind_to_exactly_match_codec_presence() {
    for (kind, video, audio, valid) in [
        (StreamKind::VideoOnly, true, false, true),
        (StreamKind::AudioOnly, false, true, true),
        (StreamKind::Combined, true, true, true),
        (StreamKind::VideoOnly, false, false, false),
        (StreamKind::VideoOnly, true, true, false),
        (StreamKind::AudioOnly, true, false, false),
        (StreamKind::AudioOnly, true, true, false),
        (StreamKind::Combined, true, false, false),
        (StreamKind::Combined, false, true, false),
        (StreamKind::Combined, false, false, false),
    ] {
        let mut media = sample();
        let row = &mut media.formats[0];
        row.kind = kind;
        row.video_codec = video.then(|| "avc1".into());
        row.audio_codec = audio.then(|| "mp4a".into());
        row.width = None;
        row.height = None;
        row.fps = None;
        row.dynamic_range = None;
        assert_eq!(media.validate().is_ok(), valid, "{kind:?} {video} {audio}");
    }
}

#[test]
fn audio_only_rejects_each_video_attribute() {
    for attribute in 0..5 {
        let mut media = sample();
        let row = &mut media.formats[0];
        row.kind = StreamKind::AudioOnly;
        row.video_codec = None;
        row.audio_codec = Some("mp4a".into());
        row.width = None;
        row.height = None;
        row.fps = None;
        row.dynamic_range = None;
        row.video_bitrate_kbps = None;
        match attribute {
            0 => row.width = Some(1),
            1 => row.height = Some(1),
            2 => row.fps = Some(1.0),
            3 => row.dynamic_range = Some("SDR".into()),
            _ => row.video_bitrate_kbps = Some(1.0),
        }
        invalid(&media);
    }
}

#[test]
fn video_only_rejects_each_audio_metric_and_combined_accepts_both_groups() {
    for attribute in 0..3 {
        let mut media = sample();
        let row = &mut media.formats[0];
        match attribute {
            0 => row.audio_bitrate_kbps = Some(1.0),
            1 => row.audio_sample_rate_hz = Some(1),
            _ => row.audio_channels = Some(1),
        }
        invalid(&media);
    }

    let mut media = sample();
    let row = &mut media.formats[0];
    row.kind = StreamKind::Combined;
    row.audio_codec = Some("mp4a.40.2".into());
    row.video_bitrate_kbps = Some(1_000_000.0);
    row.audio_bitrate_kbps = Some(100_000.0);
    row.audio_sample_rate_hz = Some(768_000);
    row.audio_channels = Some(64);
    assert!(media.validate().is_ok());
}

#[test]
fn accepts_media_metric_boundaries_and_rejects_values_immediately_outside() {
    for value in [
        0.0,
        -1.0,
        1_000_000.1,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let mut media = sample();
        media.formats[0].video_bitrate_kbps = Some(value);
        invalid(&media);
    }
    for value in [
        0.0,
        -1.0,
        100_000.1,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let mut media = sample();
        media.formats[0].kind = StreamKind::Combined;
        media.formats[0].audio_codec = Some("mp4a.40.2".into());
        media.formats[0].audio_bitrate_kbps = Some(value);
        invalid(&media);
    }
    for value in [0, 768_001, u32::MAX] {
        let mut media = sample();
        media.formats[0].kind = StreamKind::Combined;
        media.formats[0].audio_codec = Some("mp4a.40.2".into());
        media.formats[0].audio_sample_rate_hz = Some(value);
        invalid(&media);
    }
    for value in [0, 65, u8::MAX] {
        let mut media = sample();
        media.formats[0].kind = StreamKind::Combined;
        media.formats[0].audio_codec = Some("mp4a.40.2".into());
        media.formats[0].audio_channels = Some(value);
        invalid(&media);
    }

    for (video_bitrate, audio_bitrate, sample_rate, channels) in
        [(1.0, 1.0, 1, 1), (1_000_000.0, 100_000.0, 768_000, 64)]
    {
        let mut media = sample();
        let row = &mut media.formats[0];
        row.kind = StreamKind::Combined;
        row.audio_codec = Some("mp4a.40.2".into());
        row.video_bitrate_kbps = Some(video_bitrate);
        row.audio_bitrate_kbps = Some(audio_bitrate);
        row.audio_sample_rate_hz = Some(sample_rate);
        row.audio_channels = Some(channels);
        assert!(media.validate().is_ok());
    }
}

#[test]
fn rejects_invalid_container_codec_and_dynamic_range_tokens() {
    for token in [
        "".to_owned(),
        "MP4".into(),
        "mp-4".into(),
        "a".repeat(17),
        "界".into(),
    ] {
        let mut media = sample();
        media.formats[0].container = token;
        invalid(&media);
    }
    for token in [
        "".to_owned(),
        "none".into(),
        "bad codec".into(),
        "bad/codec".into(),
        "bad\ncodec".into(),
        "界".into(),
        "a".repeat(65),
    ] {
        let mut media = sample();
        media.formats[0].video_codec = Some(token.clone());
        invalid(&media);
        let mut media = sample();
        media.formats[0].kind = StreamKind::Combined;
        media.formats[0].audio_codec = Some(token);
        invalid(&media);
    }
    for token in ["SDR", "HDR", "HDR10", "HDR10+", "HDR12", "HLG", "DV"] {
        let mut media = sample();
        media.formats[0].dynamic_range = Some(token.into());
        assert!(media.validate().is_ok());
    }
    for token in ["", "hdr", "HDR11", "HDR10 "] {
        let mut media = sample();
        media.formats[0].dynamic_range = Some(token.into());
        invalid(&media);
    }
}

#[test]
fn rejects_zero_and_oversized_dimensions() {
    for value in [0, 16385, u32::MAX] {
        let mut media = sample();
        media.formats[0].width = Some(value);
        invalid(&media);
        let mut media = sample();
        media.formats[0].height = Some(value);
        invalid(&media);
    }
}

#[test]
fn rejects_nonfinite_and_out_of_bounds_fps_and_duration() {
    for value in [0.0, -1.0, 240.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut media = sample();
        media.formats[0].fps = Some(value);
        invalid(&media);
    }
    for value in [
        -1.0,
        31_536_001.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let mut media = sample();
        media.duration_seconds = Some(value);
        invalid(&media);
    }
    let mut media = sample();
    media.duration_seconds = Some(0.0);
    media.formats[0].fps = Some(0.01);
    assert!(media.validate().is_ok());
}

#[test]
fn rejects_zero_and_javascript_unsafe_file_sizes() {
    for value in [0, 9_007_199_254_740_992, u64::MAX] {
        let mut media = sample();
        media.formats[0].file_size = Some(FileSize {
            bytes: value,
            is_approximate: false,
        });
        invalid(&media);
    }
    let mut media = sample();
    media.formats[0].file_size = Some(FileSize {
        bytes: 1,
        is_approximate: false,
    });
    assert!(media.validate().is_ok());
}
