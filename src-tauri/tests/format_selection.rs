use local_media_downloader_lib::domain::{
    format_selection::{
        ranked_audio_format_ids, ranked_video_format_ids, select_formats, ClipRange,
        SelectionIntent, SelectionMode, SelectionOperation, SelectionPlan,
        SelectionUnavailableCode, SelectionWarning,
    },
    media::{FileSize, MediaFormat, MediaInfo, Platform, StreamKind},
};

fn format(id: &str, kind: StreamKind, codec: &str) -> MediaFormat {
    let video = kind != StreamKind::AudioOnly;
    let audio = kind != StreamKind::VideoOnly;
    MediaFormat {
        id: id.into(),
        container: "mp4".into(),
        kind,
        width: video.then_some(1920),
        height: video.then_some(1080),
        fps: video.then_some(30.0),
        video_bitrate_kbps: video.then_some(1000.0),
        audio_bitrate_kbps: audio.then_some(128.0),
        audio_sample_rate_hz: audio.then_some(48000),
        audio_channels: audio.then_some(2),
        video_codec: video.then(|| codec.into()),
        audio_codec: audio.then(|| if video { "aac".into() } else { codec.into() }),
        dynamic_range: video.then(|| "SDR".into()),
        file_size: Some(FileSize {
            bytes: 100,
            is_approximate: false,
        }),
    }
}

fn intent(mode: SelectionMode) -> SelectionIntent {
    SelectionIntent {
        compatibility_mode: false,
        mode,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}

#[test]
fn best_av1_with_opus_copies_both_to_mp4() {
    let mut high = format("high", StreamKind::VideoOnly, "av1");
    high.width = Some(3840);
    high.height = Some(2160);
    let rows = media(vec![
        high,
        format("low", StreamKind::Combined, "h264"),
        format("sound", StreamKind::AudioOnly, "opus"),
    ]);
    let plan = select(&rows, &intent(SelectionMode::BestAvailable)).unwrap();
    assert_eq!(plan.video_format_id.as_deref(), Some("high"));
    assert_eq!(plan.output_container.as_deref(), Some("mp4"));
    assert_eq!(plan.audio_format_id.as_deref(), Some("sound"));
    let json = serde_json::to_value(plan).unwrap();
    assert_eq!(json["outputAudioCodec"], "opus");
    assert_eq!(json["videoProcessing"], "copy");
    assert_eq!(json["audioProcessing"], "copy");
    assert_eq!(json["compatibilityMode"], false);
}

#[test]
fn manual_accepts_webm_without_audio_override() {
    let request: SelectionIntent = serde_json::from_value(
        serde_json::json!({"mode":"manual","videoFormatId":"video","outputContainer":"webm"}),
    )
    .unwrap();
    let rows = media(vec![
        format("video", StreamKind::VideoOnly, "vp9"),
        format("sound", StreamKind::AudioOnly, "opus"),
    ]);
    let json = serde_json::to_value(select(&rows, &request).unwrap()).unwrap();
    assert_eq!(json["outputContainer"], "webm");
    assert_eq!(json["outputAudioCodec"], "opus");
    assert_eq!(json["audioProcessing"], "copy");
    assert!(serde_json::from_value::<SelectionIntent>(
        serde_json::json!({"mode":"manual","videoFormatId":"video","audioFormatId":"sound"})
    )
    .is_err());
}

#[test]
fn h264_webm_is_unavailable() {
    let request: SelectionIntent = serde_json::from_value(
        serde_json::json!({"mode":"video_only","videoFormatId":"video","outputContainer":"webm"}),
    )
    .unwrap();
    assert_eq!(
        select(
            &media(vec![format("video", StreamKind::VideoOnly, "h264")]),
            &request
        ),
        Err(SelectionUnavailableCode::InvalidSelection)
    );
}

#[test]
fn audio_only_keeps_native_opus_in_webm() {
    let mut audio = format("sound", StreamKind::AudioOnly, "opus");
    audio.container = "webm".into();
    let json = serde_json::to_value(
        select(&media(vec![audio]), &intent(SelectionMode::AudioOnly)).unwrap(),
    )
    .unwrap();
    assert_eq!(json["outputContainer"], "webm");
    assert_eq!(json["outputAudioCodec"], "opus");
    assert_eq!(json["audioProcessing"], "copy");
}

#[test]
fn automatic_audio_uses_native_rank_without_container_preference() {
    let rows = media(vec![
        format("video", StreamKind::VideoOnly, "av1"),
        format("aac", StreamKind::AudioOnly, "aac"),
        format("opus", StreamKind::AudioOnly, "opus"),
    ]);
    let plan = select(&rows, &intent(SelectionMode::BestAvailable)).unwrap();
    assert_eq!(plan.audio_format_id.as_deref(), Some("opus"));
    assert_eq!(
        serde_json::to_value(plan).unwrap()["audioProcessing"],
        "copy"
    );
}

#[test]
fn unsupported_highest_video_does_not_silently_choose_lower_video() {
    let mut high = format("unsupported", StreamKind::Combined, "unrecognized");
    high.width = Some(3840);
    high.height = Some(2160);
    assert_eq!(
        select(
            &media(vec![high, format("low", StreamKind::Combined, "h264")]),
            &intent(SelectionMode::BestAvailable)
        ),
        Err(SelectionUnavailableCode::FormatUnavailable)
    );
}

#[test]
fn combined_source_keeps_its_original_audio() {
    let mut combined = format("combined", StreamKind::Combined, "av1");
    combined.audio_codec = Some("opus".into());
    let json = serde_json::to_value(
        select(
            &media(vec![combined]),
            &intent(SelectionMode::BestAvailable),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(json["videoFormatId"], "combined");
    assert_eq!(json["outputContainer"], "mp4");
    assert_eq!(json["audioProcessing"], "copy");
    assert_eq!(json["outputAudioCodec"], "opus");
}

#[test]
fn best_combined_does_not_hide_better_combined_audio() {
    let mut high = format("highest-video", StreamKind::Combined, "h264");
    high.width = Some(3840);
    high.height = Some(2160);
    let mut other = format("highest-audio", StreamKind::Combined, "h264");
    other.audio_codec = Some("flac".into());
    let rows = media(vec![high, other]);
    let plan = select(&rows, &intent(SelectionMode::BestAvailable)).unwrap();
    assert_eq!(plan.source_format_ids, ["highest-video", "highest-audio"]);
    assert_eq!(plan.video_format_id.as_deref(), Some("highest-video"));
    assert_eq!(plan.audio_format_id.as_deref(), Some("highest-audio"));
    assert_eq!(
        serde_json::to_value(plan).unwrap()["outputAudioCodec"],
        "flac"
    );
}

#[test]
fn vp8_container_fallback_never_changes_audio_source() {
    for (codec, want) in [("opus", "webm"), ("aac", "mkv")] {
        let rows = media(vec![
            format("v", StreamKind::VideoOnly, "vp8"),
            format("a", StreamKind::AudioOnly, codec),
        ]);
        let plan = select(&rows, &intent(SelectionMode::BestAvailable)).unwrap();
        assert_eq!(plan.output_container.as_deref(), Some(want));
        assert_eq!(plan.source_format_ids, ["v", "a"]);
        assert_eq!(
            serde_json::to_value(plan).unwrap()["audioProcessing"],
            "copy"
        );
    }
}

#[test]
fn explicit_webm_cannot_transcode_aac_to_force_container() {
    let rows = media(vec![
        format("v", StreamKind::VideoOnly, "vp9"),
        format("a", StreamKind::AudioOnly, "aac"),
    ]);
    let request: SelectionIntent = serde_json::from_value(
        serde_json::json!({"mode":"manual","videoFormatId":"v","outputContainer":"webm"}),
    )
    .unwrap();
    assert_eq!(
        select(&rows, &request),
        Err(SelectionUnavailableCode::InvalidSelection)
    );
}

#[test]
fn compatibility_opt_in_keeps_sources_but_changes_targets() {
    let rows = media(vec![
        format("v", StreamKind::VideoOnly, "av1"),
        format("a", StreamKind::AudioOnly, "opus"),
    ]);
    let request: SelectionIntent = serde_json::from_value(
        serde_json::json!({"mode":"best_available","compatibilityMode":true}),
    )
    .unwrap();
    let plan = select(&rows, &request).unwrap();
    let json = serde_json::to_value(plan).unwrap();
    assert_eq!(json["sourceFormatIds"], serde_json::json!(["v", "a"]));
    assert_eq!(json["outputVideoCodec"], "h264");
    assert_eq!(json["outputAudioCodec"], "aac");
    assert_eq!(json["videoProcessing"], "transcode");
    assert_eq!(json["compatibilityMode"], true);
}

#[test]
fn selectable_modes_honor_lower_resolution_without_silent_fallback() {
    let mut low = format("low", StreamKind::VideoOnly, "h264");
    low.width = Some(1280);
    low.height = Some(720);
    let mut high = format("high", StreamKind::VideoOnly, "av1");
    high.width = Some(3840);
    high.height = Some(2160);
    let rows = media(vec![
        high,
        low,
        format("sound", StreamKind::AudioOnly, "aac"),
    ]);
    for mode in ["manual", "video_only"] {
        let request: SelectionIntent = serde_json::from_value(serde_json::json!({
            "mode": mode, "videoFormatId": "low"
        }))
        .unwrap();
        let plan = select(&rows, &request).unwrap();
        assert_eq!(plan.video_format_id.as_deref(), Some("low"));
        assert_eq!(
            plan.audio_format_id.as_deref(),
            (mode == "manual").then_some("sound")
        );
        assert_eq!(
            plan.source_format_ids,
            if mode == "manual" {
                vec!["low", "sound"]
            } else {
                vec!["low"]
            }
        );
        let stale: SelectionIntent = serde_json::from_value(serde_json::json!({
            "mode": mode, "videoFormatId": "missing"
        }))
        .unwrap();
        assert_eq!(
            select(&rows, &stale),
            Err(SelectionUnavailableCode::InvalidSelection)
        );
    }
}

#[test]
fn video_only_combined_source_requires_extraction_and_unknown_output_size() {
    let rows = media(vec![format("combined", StreamKind::Combined, "h264")]);
    let request: SelectionIntent =
        serde_json::from_value(serde_json::json!({"mode":"video_only"})).unwrap();
    let plan = select(&rows, &request).unwrap();
    assert_eq!(plan.audio_format_id, None);
    assert_eq!(plan.source_format_ids, ["combined"]);
    assert_eq!(
        serde_json::to_value(plan.operation).unwrap(),
        "extract_video"
    );
    assert_eq!(plan.output_container.as_deref(), Some("mp4"));
    assert_eq!(plan.estimated_size_bytes, None);
    assert!(
        serde_json::from_value::<SelectionIntent>(serde_json::json!({
            "mode":"video_only", "audioFormatId":"combined"
        }))
        .is_err()
    );
}

#[test]
fn combined_keep_plan_uses_one_source_for_both_stream_ids() {
    let rows = media(vec![format("source", StreamKind::Combined, "h264")]);
    for mode in [SelectionMode::BestAvailable, SelectionMode::Manual] {
        let mut request = intent(mode);
        if mode == SelectionMode::Manual {
            request.video_format_id = Some("source".into());
        }
        let plan = select(&rows, &request).unwrap();
        assert_eq!(
            plan,
            SelectionPlan {
                compatibility_mode: false,
                container_selection:
                    local_media_downloader_lib::domain::output_policy::ContainerSelection::Automatic,
                video_processing: Some(
                    local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy
                ),
                output_video_codec: Some("h264".into()),
                mode,
                source_format_ids: vec!["source".into()],
                video_format_id: Some("source".into()),
                audio_format_id: Some("source".into()),
                operation: SelectionOperation::Keep,
                output_container: Some("mp4".into()),
                output_audio_codec: Some(
                    local_media_downloader_lib::domain::output_policy::TargetAudioCodec::Aac
                ),
                audio_processing: Some(
                    local_media_downloader_lib::domain::output_policy::AudioProcessing::Copy
                ),
                clip: None,
                estimated_size_bytes: Some(100),
                warnings: vec![SelectionWarning::ContainerUnverified]
            }
        );
    }
}

#[test]
fn merge_plan_has_distinct_ids_compatible_container_and_summed_size() {
    let mut video = format("video", StreamKind::VideoOnly, "h264");
    video.file_size.as_mut().unwrap().bytes = 200;
    let mut audio = format("audio", StreamKind::AudioOnly, "aac");
    audio.file_size.as_mut().unwrap().bytes = 150;
    let plan = select(
        &media(vec![video, audio]),
        &intent(SelectionMode::BestAvailable),
    )
    .unwrap();
    assert_eq!(
        plan,
        SelectionPlan {
            compatibility_mode: false,
            container_selection:
                local_media_downloader_lib::domain::output_policy::ContainerSelection::Automatic,
            video_processing: Some(
                local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy
            ),
            output_video_codec: Some("h264".into()),
            mode: SelectionMode::BestAvailable,
            source_format_ids: vec!["video".into(), "audio".into()],
            video_format_id: Some("video".into()),
            audio_format_id: Some("audio".into()),
            operation: SelectionOperation::Merge,
            output_container: Some("mp4".into()),
            output_audio_codec: Some(
                local_media_downloader_lib::domain::output_policy::TargetAudioCodec::Aac
            ),
            audio_processing: Some(
                local_media_downloader_lib::domain::output_policy::AudioProcessing::Copy
            ),
            clip: None,
            estimated_size_bytes: Some(350),
            warnings: vec![SelectionWarning::ContainerUnverified]
        }
    );
}

#[test]
fn merge_container_is_mp4_for_supported_video_regardless_of_source_container_or_hdr() {
    for (video_codec, audio_codec, container, range, valid) in [
        ("av1", "opus", "mp4", "SDR", true),
        ("h264", "aac", "webm", "SDR", true),
        ("h264", "aac", "mp4", "HDR", true),
        ("avc1evil", "aac", "mp4", "SDR", false),
        ("h264", "mp4a.40.3", "mp4", "SDR", false),
    ] {
        let mut video = format("video", StreamKind::VideoOnly, video_codec);
        video.container = container.into();
        video.dynamic_range = Some(range.into());
        let rows = media(vec![
            video,
            format("audio", StreamKind::AudioOnly, audio_codec),
        ]);
        let result = select(&rows, &intent(SelectionMode::BestAvailable));
        if valid {
            assert_eq!(result.unwrap().output_container.as_deref(), Some("mp4"));
        } else {
            assert_eq!(result, Err(SelectionUnavailableCode::FormatUnavailable));
        }
    }
}

#[test]
fn audio_keep_preserves_original_codec_and_omits_video_in_json() {
    let mut audio = format("audio", StreamKind::AudioOnly, "opus");
    audio.container = "webm".into();
    let plan = select(&media(vec![audio]), &intent(SelectionMode::AudioOnly)).unwrap();
    assert_eq!(
        serde_json::to_value(plan).unwrap(),
        serde_json::json!({"mode":"audio_only","compatibilityMode":false,"containerSelection":"automatic","sourceFormatIds":["audio"],"audioFormatId":"audio","operation":"keep","outputContainer":"webm","outputAudioCodec":"opus","audioProcessing":"copy","estimatedSizeBytes":100,"warnings":["container_unverified"]})
    );
}

#[test]
fn extraction_keeps_native_audio_in_its_lossless_container_and_rejects_unknown() {
    for (codec, container) in [
        ("aac", "m4a"),
        ("MP4A.40.2", "m4a"),
        ("alac", "m4a"),
        ("Opus", "webm"),
        ("vorbis", "webm"),
        ("MP3", "mp3"),
        ("flac", "flac"),
        ("mp4a.40.3", ""),
    ] {
        let mut source = format("combined", StreamKind::Combined, "h264");
        source.audio_codec = Some(codec.into());
        let result = select(&media(vec![source]), &intent(SelectionMode::AudioOnly));
        if codec == "mp4a.40.3" {
            assert_eq!(result, Err(SelectionUnavailableCode::FormatUnavailable));
            continue;
        }
        let plan = result.unwrap();
        assert_eq!(plan.source_format_ids, ["combined"]);
        assert_eq!(plan.video_format_id, None);
        assert_eq!(plan.audio_format_id.as_deref(), Some("combined"));
        assert_eq!(plan.operation, SelectionOperation::ExtractAudio);
        assert_eq!(plan.output_container.as_deref(), Some(container));
        assert_eq!(
            plan.audio_processing,
            Some(local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy)
        );
        assert_eq!(plan.estimated_size_bytes, None);
    }
}

#[test]
fn estimates_require_all_sizes_and_a_js_safe_sum() {
    for (video_bytes, audio_bytes, expected) in [
        (Some(9007199254740891), Some(100), Some(9007199254740991)),
        (Some(9007199254740892), Some(100), None),
        (Some(u64::MAX), Some(100), None),
        (None, Some(100), None),
        (Some(100), None, None),
    ] {
        let mut video = format("video", StreamKind::VideoOnly, "h264");
        video.file_size = video_bytes.map(|bytes| FileSize {
            bytes,
            is_approximate: false,
        });
        let mut audio = format("audio", StreamKind::AudioOnly, "aac");
        audio.file_size = audio_bytes.map(|bytes| FileSize {
            bytes,
            is_approximate: true,
        });
        let plan = select(
            &media(vec![video, audio]),
            &intent(SelectionMode::BestAvailable),
        )
        .unwrap();
        assert_eq!(plan.estimated_size_bytes, expected);
        assert_eq!(
            plan.warnings,
            if expected.is_some() {
                vec![SelectionWarning::ContainerUnverified]
            } else {
                vec![
                    SelectionWarning::ContainerUnverified,
                    SelectionWarning::OutputSizeUnknown,
                ]
            }
        );
    }
    let mut combined = format("source", StreamKind::Combined, "h264");
    combined.file_size = None;
    assert_eq!(
        select(
            &media(vec![combined]),
            &intent(SelectionMode::BestAvailable)
        )
        .unwrap()
        .estimated_size_bytes,
        None
    );
}

#[test]
fn clips_omit_size_and_order_deduplicated_warnings_in_serialized_plan() {
    let mut source = format("source", StreamKind::Combined, "h264");
    source.width = None;
    source.audio_channels = None;
    let mut request = intent(SelectionMode::BestAvailable);
    request.clip = Some(ClipRange {
        start_ms: 1000,
        end_ms: 2000,
    });
    let plan = select(&media(vec![source]), &request).unwrap();
    assert_eq!(
        serde_json::to_value(plan).unwrap(),
        serde_json::json!({"mode":"best_available","compatibilityMode":false,"containerSelection":"automatic","videoProcessing":"copy","outputVideoCodec":"h264","sourceFormatIds":["source"],"videoFormatId":"source","audioFormatId":"source","operation":"keep","outputContainer":"mp4","outputAudioCodec":"aac","audioProcessing":"copy","clip":{"startMs":1000,"endMs":2000},"warnings":["metadata_incomplete","container_unverified","output_size_unknown","clip_boundary_may_vary","clip_transfer_may_exceed_range"]})
    );
}

#[test]
fn each_missing_selected_metric_or_unknown_codec_adds_metadata_warning_once() {
    for field in [
        "width",
        "height",
        "fps",
        "range",
        "channels",
        "abr",
        "asr",
        "video_codec",
        "audio_codec",
    ] {
        let mut source = format("source", StreamKind::Combined, "h264");
        match field {
            "width" => source.width = None,
            "height" => source.height = None,
            "fps" => source.fps = None,
            "range" => source.dynamic_range = None,
            "channels" => source.audio_channels = None,
            "abr" => source.audio_bitrate_kbps = None,
            "asr" => source.audio_sample_rate_hz = None,
            "video_codec" => source.video_codec = Some("avc1evil".into()),
            "audio_codec" => source.audio_codec = Some("mp4a.40.3".into()),
            _ => unreachable!(),
        }
        if field == "video_codec" || field == "audio_codec" {
            assert_eq!(
                select(&media(vec![source]), &intent(SelectionMode::BestAvailable)),
                Err(SelectionUnavailableCode::FormatUnavailable)
            );
            continue;
        }
        assert_eq!(
            select(&media(vec![source]), &intent(SelectionMode::BestAvailable))
                .unwrap()
                .warnings,
            [
                SelectionWarning::MetadataIncomplete,
                SelectionWarning::ContainerUnverified
            ],
            "{field}"
        );
    }
}

#[test]
fn only_selected_stream_metadata_affects_warnings() {
    let mut source = format("source", StreamKind::Combined, "unknown");
    source.width = None;
    source.height = None;
    source.fps = None;
    source.dynamic_range = None;
    assert_eq!(
        select(&media(vec![source]), &intent(SelectionMode::AudioOnly))
            .unwrap()
            .warnings,
        [
            SelectionWarning::ContainerUnverified,
            SelectionWarning::OutputSizeUnknown
        ]
    );
    let mut source = format("source", StreamKind::Combined, "h264");
    source.video_bitrate_kbps = None;
    source.dynamic_range = Some("DV".into());
    let mut unused = format("unused", StreamKind::VideoOnly, "unknown");
    unused.width = None;
    assert_eq!(
        select(
            &media(vec![source, unused]),
            &intent(SelectionMode::BestAvailable)
        )
        .unwrap()
        .warnings,
        [SelectionWarning::ContainerUnverified]
    );
}

#[test]
fn unknown_extracted_codec_is_unavailable_instead_of_unverified_output() {
    let mut source = format("source", StreamKind::Combined, "h264");
    source.audio_codec = Some("unknown".into());
    assert_eq!(
        select(&media(vec![source]), &intent(SelectionMode::AudioOnly)),
        Err(SelectionUnavailableCode::FormatUnavailable)
    );
}

#[test]
fn unavailable_codes_have_closed_snake_case_serialization() {
    for (code, literal) in [
        (
            SelectionUnavailableCode::FormatUnavailable,
            "format_unavailable",
        ),
        (
            SelectionUnavailableCode::InvalidSelection,
            "invalid_selection",
        ),
        (SelectionUnavailableCode::InvalidClip, "invalid_clip"),
        (
            SelectionUnavailableCode::CompatibilityUnavailable,
            "compatibility_unavailable",
        ),
        (
            SelectionUnavailableCode::ClipDurationUnknown,
            "clip_duration_unknown",
        ),
    ] {
        assert_eq!(
            serde_json::to_value(code).unwrap(),
            serde_json::json!(literal)
        );
    }
}

fn select(
    media: &MediaInfo,
    intent: &SelectionIntent,
) -> Result<SelectionPlan, SelectionUnavailableCode> {
    let before = media.clone();
    let result = select_formats(media, intent);
    if media.duration_seconds.is_some_and(f64::is_nan) {
        // NaN is deliberately supplied for the clip-duration guard; PartialEq cannot compare it.
        assert_eq!(format!("{media:?}"), format!("{before:?}"));
        assert_eq!(
            media.duration_seconds.map(f64::to_bits),
            before.duration_seconds.map(f64::to_bits)
        );
    } else {
        assert_eq!(media, &before);
    }
    result
}

#[test]
fn clips_preserve_valid_integer_endpoints_including_maximum() {
    for (duration, start_ms, end_ms) in [
        (60.0, 0, 60000),
        (1.2349, 1000, 1234),
        (31536000.0, 31535999999, 31536000000),
    ] {
        let mut rows = media(vec![format("source", StreamKind::Combined, "h264")]);
        rows.duration_seconds = Some(duration);
        let mut request = intent(SelectionMode::BestAvailable);
        request.clip = Some(ClipRange { start_ms, end_ms });
        assert_eq!(
            select(&rows, &request).unwrap().clip,
            Some(ClipRange { start_ms, end_ms })
        );
    }
}

#[test]
fn clips_reject_equal_reversed_duration_overflow_and_maximum_overflow() {
    for (duration, start_ms, end_ms) in [
        (60.0, 0, 0),
        (60.0, 1000, 999),
        (60.0, 0, 60001),
        (1.2349, 0, 1235),
        (31536001.0, 0, 31536000001),
        (31536002.0, 31536000001, 31536000002),
    ] {
        let mut rows = media(vec![format("source", StreamKind::Combined, "h264")]);
        rows.duration_seconds = Some(duration);
        let mut request = intent(SelectionMode::BestAvailable);
        request.clip = Some(ClipRange { start_ms, end_ms });
        assert_eq!(
            select(&rows, &request),
            Err(SelectionUnavailableCode::InvalidClip)
        );
    }
}

#[test]
fn clips_require_finite_positive_known_duration() {
    for duration in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::NAN),
        Some(f64::INFINITY),
        Some(f64::NEG_INFINITY),
    ] {
        let mut rows = media(vec![format("source", StreamKind::Combined, "h264")]);
        rows.duration_seconds = duration;
        let mut request = intent(SelectionMode::BestAvailable);
        request.clip = Some(ClipRange {
            start_ms: 0,
            end_ms: 1,
        });
        assert_eq!(
            select(&rows, &request),
            Err(SelectionUnavailableCode::ClipDurationUnknown)
        );
    }
}

#[test]
fn audio_only_rejects_clips_without_falling_back() {
    let rows = media(vec![format("source", StreamKind::AudioOnly, "aac")]);
    let mut request = intent(SelectionMode::AudioOnly);
    request.clip = Some(ClipRange {
        start_ms: 0,
        end_ms: 1000,
    });
    assert_eq!(
        select(&rows, &request),
        Err(SelectionUnavailableCode::InvalidSelection)
    );
}

#[test]
fn intent_json_rejects_unknown_fields_and_non_integer_clip_endpoints() {
    for value in [
        serde_json::json!({"mode":"best_available","url":"https://invalid.example"}),
        serde_json::json!({"mode":"best_available","clip":{"startMs":0,"endMs":1,"extra":true}}),
        serde_json::json!({"mode":"best_available","clip":{"startMs":-1,"endMs":1}}),
        serde_json::json!({"mode":"best_available","clip":{"startMs":0,"endMs":1.5}}),
        serde_json::json!({"mode":"best_available","clip":{"startMs":0,"endMs":"1000"}}),
        serde_json::json!({"mode":"best_available","clip":{"startMs":0}}),
        serde_json::json!({"mode":"unrecognized"}),
    ] {
        assert!(serde_json::from_value::<SelectionIntent>(value).is_err());
    }
    assert_eq!(serde_json::from_value::<SelectionIntent>(serde_json::json!({"mode":"manual","videoFormatId":"source","clip":{"startMs":0,"endMs":1}})).unwrap(), SelectionIntent { compatibility_mode: false, mode: SelectionMode::Manual, video_format_id: Some("source".into()), output_container: None, clip: Some(ClipRange { start_ms: 0, end_ms: 1 }) });
}

#[test]
fn best_available_pairs_best_video_with_best_native_audio() {
    let rows = media(vec![
        format("combined", StreamKind::Combined, "h264"),
        format("video", StreamKind::VideoOnly, "av1"),
        format("aac", StreamKind::AudioOnly, "aac"),
        format("opus", StreamKind::AudioOnly, "opus"),
    ]);
    let plan = select(&rows, &intent(SelectionMode::BestAvailable)).unwrap();
    assert_eq!(plan.source_format_ids, ["video", "opus"]);
    assert_eq!(plan.operation, SelectionOperation::Merge);
}

#[test]
fn best_available_maps_best_roles_even_when_audio_is_from_a_combined_source() {
    let mut rows = media(vec![
        format("silent", StreamKind::VideoOnly, "av1"),
        format("combined", StreamKind::Combined, "h264"),
    ]);
    assert_eq!(
        select(&rows, &intent(SelectionMode::BestAvailable))
            .unwrap()
            .source_format_ids,
        ["silent", "combined"]
    );
    rows.formats.remove(0);
    rows.formats
        .push(format("audio", StreamKind::AudioOnly, "flac"));
    assert_eq!(
        select(&rows, &intent(SelectionMode::BestAvailable))
            .unwrap()
            .source_format_ids,
        ["combined", "audio"]
    );
}

#[test]
fn unavailable_modes_do_not_produce_silent_video_or_fallback() {
    let rows = media(vec![format("silent", StreamKind::VideoOnly, "av1")]);
    for mode in [SelectionMode::BestAvailable, SelectionMode::AudioOnly] {
        assert_eq!(
            select(&rows, &intent(mode)),
            Err(SelectionUnavailableCode::FormatUnavailable)
        );
    }
}

#[test]
fn audio_only_compares_combined_sources_and_keeps_or_extracts() {
    let rows = media(vec![
        format("combined", StreamKind::Combined, "h264"),
        format("audio", StreamKind::AudioOnly, "mp3"),
    ]);
    let plan = select(&rows, &intent(SelectionMode::AudioOnly)).unwrap();
    assert_eq!(plan.source_format_ids, ["combined"]);
    assert_eq!(plan.operation, SelectionOperation::ExtractAudio);
    let rows = media(vec![format("audio", StreamKind::AudioOnly, "aac")]);
    assert_eq!(
        select(&rows, &intent(SelectionMode::AudioOnly))
            .unwrap()
            .operation,
        SelectionOperation::Keep
    );
}

#[test]
fn manual_accepts_combined_alone_or_video_with_automatic_audio() {
    let rows = media(vec![
        format("combined", StreamKind::Combined, "h264"),
        format("video", StreamKind::VideoOnly, "h264"),
        format("audio", StreamKind::AudioOnly, "aac"),
    ]);
    for (video, sources, operation) in [
        (
            "combined",
            vec!["combined", "audio"],
            SelectionOperation::Merge,
        ),
        ("video", vec!["video", "audio"], SelectionOperation::Merge),
    ] {
        let mut request = intent(SelectionMode::Manual);
        request.video_format_id = Some(video.into());
        let plan = select(&rows, &request).unwrap();
        assert_eq!(plan.source_format_ids, sources);
        assert_eq!(plan.operation, operation);
    }
}

#[test]
fn manual_rejects_missing_unknown_cross_role_ids_and_audio_overrides() {
    let rows = media(vec![
        format("combined", StreamKind::Combined, "h264"),
        format("video", StreamKind::VideoOnly, "h264"),
        format("audio", StreamKind::AudioOnly, "aac"),
    ]);
    for video in [None, Some("missing"), Some("audio")] {
        let mut request = intent(SelectionMode::Manual);
        request.video_format_id = video.map(str::to_owned);
        assert_eq!(
            select(&rows, &request),
            Err(SelectionUnavailableCode::InvalidSelection)
        );
    }
    for audio in ["missing", "video", "audio", "combined"] {
        assert!(serde_json::from_value::<SelectionIntent>(
            serde_json::json!({"mode":"manual","videoFormatId":"video","audioFormatId":audio})
        )
        .is_err());
    }
}

#[test]
fn automatic_modes_reject_manual_ids_and_requested_containers() {
    let rows = media(vec![format("combined", StreamKind::Combined, "h264")]);
    for mode in [SelectionMode::BestAvailable, SelectionMode::AudioOnly] {
        let mut request = intent(mode);
        request.video_format_id = Some("combined".into());
        assert_eq!(
            select(&rows, &request),
            Err(SelectionUnavailableCode::InvalidSelection)
        );
        request.video_format_id = None;
        request.output_container =
            Some(local_media_downloader_lib::domain::output_policy::VideoContainer::Mp4);
        assert_eq!(
            select(&rows, &request),
            Err(SelectionUnavailableCode::InvalidSelection)
        );
    }
}

#[test]
fn manual_ids_apply_safe_token_policy_before_lookup() {
    for id in [
        "".to_owned(),
        "x".repeat(65),
        "space id".into(),
        "a/b".into(),
        "a\n".into(),
        "视频".into(),
    ] {
        let rows = media(vec![format(&id, StreamKind::Combined, "h264")]);
        let mut request = intent(SelectionMode::Manual);
        request.video_format_id = Some(id);
        assert_eq!(
            select(&rows, &request),
            Err(SelectionUnavailableCode::InvalidSelection)
        );
    }
    let id = "x".repeat(64);
    let rows = media(vec![format(&id, StreamKind::Combined, "h264")]);
    let mut request = intent(SelectionMode::Manual);
    request.video_format_id = Some(id.clone());
    assert_eq!(select(&rows, &request).unwrap().source_format_ids, [id]);
}

fn media(formats: Vec<MediaFormat>) -> MediaInfo {
    MediaInfo {
        platform: Platform::Youtube,
        media_id: "sample".into(),
        title: "Sample".into(),
        creator: None,
        duration_seconds: Some(60.0),
        formats,
    }
}

fn video_order(media: &MediaInfo) -> Vec<String> {
    let before = media.clone();
    let ids = ranked_video_format_ids(media);
    assert_eq!(media, &before);
    ids
}

fn audio_order(media: &MediaInfo) -> Vec<String> {
    let before = media.clone();
    let ids = ranked_audio_format_ids(media);
    assert_eq!(media, &before);
    ids
}

// Catches arbitrary-prefix classification and cross-codec bitrate ranking.
#[test]
fn video_codec_alias_groups_are_exact_and_case_insensitive() {
    let codecs = [
        "avc1evil",
        "vp09evil",
        "H264",
        "avc1.64001f",
        "avc3",
        "hevc",
        "H265",
        "hev1.2",
        "hvc1",
        "vp9",
        "VP09.00",
        "av1",
        "AV01.0.12M.08",
    ];
    let formats = codecs
        .iter()
        .enumerate()
        .map(|(i, codec)| {
            let mut row = format(&i.to_string(), StreamKind::VideoOnly, codec);
            row.video_bitrate_kbps = Some(if i < 2 { 999999.0 } else { 1.0 });
            row
        })
        .collect();
    assert_eq!(
        video_order(&media(formats)),
        ["11", "12", "9", "10", "5", "6", "7", "8", "2", "3", "4", "0", "1"]
    );
}

// Catches wrong key precedence, long-edge ranking, and unstable ties.
#[test]
fn video_ranking_uses_short_edge_fps_range_codec_bitrate_then_input_order() {
    let mut rows = vec![format("base", StreamKind::Combined, "h264")];
    let mut portrait = rows[0].clone();
    portrait.id = "portrait".into();
    portrait.width = Some(1080);
    portrait.height = Some(1920);
    rows.push(portrait);
    let mut bitrate = rows[0].clone();
    bitrate.id = "bitrate".into();
    bitrate.video_bitrate_kbps = Some(2000.0);
    rows.push(bitrate);
    let mut codec = rows[0].clone();
    codec.id = "codec".into();
    codec.video_codec = Some("av1".into());
    codec.video_bitrate_kbps = None;
    rows.push(codec);
    let mut hdr = rows[0].clone();
    hdr.id = "hdr".into();
    hdr.dynamic_range = Some("HDR10".into());
    rows.push(hdr);
    let mut fps = rows[0].clone();
    fps.id = "fps".into();
    fps.fps = Some(60.0);
    rows.push(fps);
    let mut size = rows[0].clone();
    size.id = "size".into();
    size.width = Some(2560);
    size.height = Some(1440);
    rows.push(size);
    let mut missing = rows[0].clone();
    missing.id = "missing".into();
    missing.width = None;
    missing.height = Some(16384);
    rows.push(missing);
    rows.push(format("audio", StreamKind::AudioOnly, "flac"));
    assert_eq!(
        video_order(&media(rows)),
        ["size", "fps", "hdr", "codec", "bitrate", "base", "portrait", "missing"]
    );
}

#[test]
fn hdr_subtypes_tie_and_dv_joins_missing_range_last() {
    let values = [
        Some("DV"),
        None,
        Some("SDR"),
        Some("HDR"),
        Some("HDR10"),
        Some("HDR10+"),
        Some("HDR12"),
        Some("HLG"),
    ];
    let rows = values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let mut row = format(&i.to_string(), StreamKind::VideoOnly, "avc1");
            row.dynamic_range = value.map(str::to_owned);
            row
        })
        .collect();
    assert_eq!(
        video_order(&media(rows)),
        ["3", "4", "5", "6", "7", "2", "0", "1"]
    );
}

#[test]
fn audio_codec_groups_reject_unsupported_mp4a_and_ignore_cross_group_bitrate() {
    let codecs = [
        "mp4a.40.3",
        "mp4a",
        "mp3",
        "aac",
        "MP4A.40.2",
        "mp4a.40.5",
        "mp4a.40.29",
        "vorbis",
        "OPUS",
        "flac",
        "ALAC",
    ];
    let rows = codecs
        .iter()
        .enumerate()
        .map(|(i, codec)| {
            let mut row = format(&i.to_string(), StreamKind::AudioOnly, codec);
            row.audio_bitrate_kbps = Some(if i < 2 { 99999.0 } else { 1.0 });
            row
        })
        .collect();
    assert_eq!(
        audio_order(&media(rows)),
        ["9", "10", "8", "7", "3", "4", "5", "6", "2", "0", "1"]
    );
}

#[test]
fn audio_ranking_uses_channels_codec_abr_asr_role_then_input_order() {
    let mut rows = vec![
        format("combined", StreamKind::Combined, "h264"),
        format("audio", StreamKind::AudioOnly, "aac"),
        format("tie", StreamKind::AudioOnly, "aac"),
    ];
    let mut sample = rows[0].clone();
    sample.id = "sample".into();
    sample.audio_sample_rate_hz = Some(96000);
    rows.push(sample);
    let mut bitrate = rows[0].clone();
    bitrate.id = "bitrate".into();
    bitrate.audio_bitrate_kbps = Some(256.0);
    rows.push(bitrate);
    let mut codec = rows[0].clone();
    codec.id = "codec".into();
    codec.audio_codec = Some("opus".into());
    codec.audio_bitrate_kbps = None;
    rows.push(codec);
    let mut channels = rows[0].clone();
    channels.id = "channels".into();
    channels.audio_channels = Some(6);
    rows.push(channels);
    let mut missing = rows[1].clone();
    missing.id = "missing".into();
    missing.audio_channels = None;
    rows.push(missing);
    rows.push(format("silent", StreamKind::VideoOnly, "av1"));
    assert_eq!(
        audio_order(&media(rows)),
        ["channels", "codec", "bitrate", "sample", "audio", "tie", "combined", "missing"]
    );
}
