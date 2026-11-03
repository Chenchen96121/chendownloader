use local_media_downloader_lib::{
    application::preview_selection::{preview_selection, SelectionOutcome},
    domain::{
        format_selection::{ClipRange, SelectionIntent, SelectionMode, SelectionUnavailableCode},
        managed_tools::ToolState,
        media::{AnalysisErrorCode, FileSize, MediaFormat, MediaInfo, Platform, StreamKind},
    },
    runtime::AppState,
};

fn format(id: &str, kind: StreamKind) -> MediaFormat {
    let has_video = kind != StreamKind::AudioOnly;
    let has_audio = kind != StreamKind::VideoOnly;
    MediaFormat {
        id: id.into(),
        container: "mp4".into(),
        kind,
        width: has_video.then_some(1920),
        height: has_video.then_some(1080),
        fps: has_video.then_some(30.0),
        video_bitrate_kbps: has_video.then_some(1_000.0),
        audio_bitrate_kbps: has_audio.then_some(128.0),
        audio_sample_rate_hz: has_audio.then_some(48_000),
        audio_channels: has_audio.then_some(2),
        video_codec: has_video.then(|| "h264".into()),
        audio_codec: has_audio.then(|| "aac".into()),
        dynamic_range: has_video.then(|| "SDR".into()),
        file_size: Some(FileSize {
            bytes: 100,
            is_approximate: false,
        }),
    }
}

fn media(formats: Vec<MediaFormat>) -> MediaInfo {
    MediaInfo {
        platform: Platform::Youtube,
        media_id: "sample".into(),
        title: "Selection sample".into(),
        creator: None,
        duration_seconds: Some(60.0),
        formats,
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
fn preview_returns_separate_ranks_without_reordering_media_and_correlates_the_plan() {
    let original = media(vec![
        format("low", StreamKind::Combined),
        format("audio", StreamKind::AudioOnly),
        {
            let mut high = format("high", StreamKind::VideoOnly);
            high.width = Some(3840);
            high.height = Some(2160);
            high
        },
    ]);

    let preview =
        preview_selection(original.clone(), intent(SelectionMode::BestAvailable)).unwrap();

    assert_eq!(preview.media, original);
    assert_eq!(
        preview
            .media
            .formats
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["low", "audio", "high"]
    );
    assert_eq!(preview.ranked_video_format_ids, ["high", "low"]);
    assert_eq!(preview.ranked_audio_format_ids, ["audio", "low"]);
    let SelectionOutcome::Selected { plan } = preview.outcome else {
        panic!("valid media should produce a selected plan");
    };
    assert_eq!(plan.source_format_ids, ["high", "audio"]);
    assert!(plan.source_format_ids.iter().all(|id| preview
        .media
        .formats
        .iter()
        .any(|row| &row.id == id)));
}

#[test]
fn normal_selection_failures_are_unavailable_with_media_and_ranks_retained() {
    let cases = [
        (
            media(vec![format("video", StreamKind::VideoOnly)]),
            intent(SelectionMode::BestAvailable),
            SelectionUnavailableCode::FormatUnavailable,
            "format_unavailable",
        ),
        (
            media(vec![format("source", StreamKind::Combined)]),
            {
                let mut request = intent(SelectionMode::Manual);
                request.video_format_id = Some("missing".into());
                request
            },
            SelectionUnavailableCode::InvalidSelection,
            "invalid_selection",
        ),
        (
            media(vec![format("source", StreamKind::Combined)]),
            {
                let mut request = intent(SelectionMode::BestAvailable);
                request.clip = Some(ClipRange {
                    start_ms: 1_000,
                    end_ms: 1_000,
                });
                request
            },
            SelectionUnavailableCode::InvalidClip,
            "invalid_clip",
        ),
        (
            {
                let mut value = media(vec![format("source", StreamKind::Combined)]);
                value.duration_seconds = None;
                value
            },
            {
                let mut request = intent(SelectionMode::BestAvailable);
                request.clip = Some(ClipRange {
                    start_ms: 0,
                    end_ms: 1_000,
                });
                request
            },
            SelectionUnavailableCode::ClipDurationUnknown,
            "clip_duration_unknown",
        ),
    ];

    for (media, request, expected_code, literal) in cases {
        let preview = preview_selection(media.clone(), request).unwrap();
        assert_eq!(preview.media, media);
        assert!(!preview.ranked_video_format_ids.is_empty());
        assert_eq!(
            preview.outcome,
            SelectionOutcome::Unavailable {
                code: expected_code
            }
        );
        assert_eq!(
            serde_json::to_value(&preview.outcome).unwrap(),
            serde_json::json!({"status":"unavailable","code":literal})
        );
    }
}

#[test]
fn invalid_native_metadata_remains_a_top_level_analysis_error() {
    let mut invalid = media(vec![format("source", StreamKind::Combined)]);
    invalid.formats[0].audio_codec = None;

    assert_eq!(
        preview_selection(invalid, intent(SelectionMode::BestAvailable)),
        Err(AnalysisErrorCode::InvalidMetadata)
    );
}

fn fixture_manual_intent(platform: Platform) -> SelectionIntent {
    let mut request = intent(SelectionMode::Manual);
    match platform {
        Platform::Youtube => {
            request.video_format_id = Some("v4k".into());
        }
        Platform::Bilibili => {
            request.video_format_id = Some("v1080".into());
        }
        Platform::X | Platform::Instagram => {
            request.video_format_id = Some("combined".into());
        }
    }
    request
}

#[test]
fn runtime_reloads_every_platform_fixture_for_every_mode_without_managed_tools() {
    for (platform, expected_id) in [
        (Platform::Youtube, "sample-youtube"),
        (Platform::Bilibili, "sample-bilibili"),
        (Platform::X, "sample-x"),
        (Platform::Instagram, "sample-instagram"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(root.path().into());
        let managed_tools = root.path().join("managed-tools");
        assert!(!managed_tools.exists());

        for request in [
            intent(SelectionMode::BestAvailable),
            intent(SelectionMode::VideoOnly),
            intent(SelectionMode::AudioOnly),
            fixture_manual_intent(platform),
        ] {
            let preview = state
                .preview_media_selection_fixture(platform, request)
                .unwrap();
            assert_eq!(preview.media.platform, platform);
            assert_eq!(preview.media.media_id, expected_id);
            assert!(!preview.ranked_video_format_ids.is_empty());
            assert!(!preview.ranked_audio_format_ids.is_empty());
            assert!(matches!(preview.outcome, SelectionOutcome::Selected { .. }));
        }

        let mut first = state
            .preview_media_selection_fixture(platform, intent(SelectionMode::BestAvailable))
            .unwrap();
        first.media.title = "mutated caller copy".into();
        let second = state
            .preview_media_selection_fixture(platform, intent(SelectionMode::BestAvailable))
            .unwrap();
        assert_ne!(second.media.title, first.media.title);
        assert!(!managed_tools.exists());
        assert!(state
            .tool_inventory()
            .tools
            .iter()
            .all(|tool| tool.state == ToolState::Missing));
    }
}

#[test]
fn runtime_returns_adjustable_unavailable_previews_for_invalid_manual_id_and_clip() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(root.path().into());
    let mut manual = intent(SelectionMode::Manual);
    manual.video_format_id = Some("missing".into());
    let manual_preview = state
        .preview_media_selection_fixture(Platform::Youtube, manual)
        .unwrap();
    assert!(!manual_preview.media.formats.is_empty());
    assert!(!manual_preview.ranked_video_format_ids.is_empty());
    assert_eq!(
        manual_preview.outcome,
        SelectionOutcome::Unavailable {
            code: SelectionUnavailableCode::InvalidSelection
        }
    );

    let mut clipped = intent(SelectionMode::BestAvailable);
    clipped.clip = Some(ClipRange {
        start_ms: 0,
        end_ms: 125_001,
    });
    let clip_preview = state
        .preview_media_selection_fixture(Platform::Youtube, clipped)
        .unwrap();
    assert!(!clip_preview.media.formats.is_empty());
    assert!(!clip_preview.ranked_audio_format_ids.is_empty());
    assert_eq!(
        clip_preview.outcome,
        SelectionOutcome::Unavailable {
            code: SelectionUnavailableCode::InvalidClip
        }
    );
}

#[test]
fn serialized_fixture_preview_has_the_closed_camel_case_shape() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(root.path().into());
    let selected = serde_json::to_value(
        state
            .preview_media_selection_fixture(
                Platform::Youtube,
                intent(SelectionMode::BestAvailable),
            )
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        selected.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "media",
            "outcome",
            "rankedAudioFormatIds",
            "rankedVideoFormatIds"
        ]
    );
    assert_eq!(selected["outcome"]["status"], "selected");
    assert_eq!(selected["outcome"]["plan"]["mode"], "best_available");
    assert_eq!(selected["outcome"]["plan"]["operation"], "merge");
    assert_eq!(
        selected["outcome"]["plan"]["sourceFormatIds"],
        serde_json::json!(["v4k", "a1"])
    );
    assert_eq!(
        selected["outcome"]["plan"]["warnings"],
        serde_json::json!(["container_unverified"])
    );
    assert!(selected["outcome"]["plan"].get("clip").is_none());

    let one_source = serde_json::to_value(
        state
            .preview_media_selection_fixture(Platform::X, intent(SelectionMode::BestAvailable))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        one_source["outcome"]["plan"]["sourceFormatIds"],
        serde_json::json!(["combined"])
    );
    assert_eq!(
        one_source["outcome"]["plan"]["warnings"],
        serde_json::json!([
            "metadata_incomplete",
            "container_unverified",
            "output_size_unknown"
        ])
    );
    assert!(one_source["media"].get("creator").is_none());
    assert!(one_source["media"].get("durationSeconds").is_none());
    assert!(one_source["media"]["formats"][0]
        .as_object()
        .unwrap()
        .get("fileSize")
        .is_none());

    for value in [&selected, &one_source] {
        assert_no_private_field_names(value);
        let ids = value["outcome"]["plan"]["sourceFormatIds"]
            .as_array()
            .unwrap();
        let unique = ids
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), ids.len());
        assert!((1..=2).contains(&ids.len()));
    }
}

#[test]
fn selection_intent_rejects_every_private_or_open_ended_input_field() {
    for field in [
        "url",
        "path",
        "cookie",
        "authorization",
        "header",
        "digest",
        "stdout",
        "stderr",
        "argument",
        "commandLine",
        "media",
        "json",
        "processOptions",
    ] {
        let mut object = serde_json::json!({"mode":"best_available"});
        object
            .as_object_mut()
            .unwrap()
            .insert(field.into(), serde_json::json!("sentinel"));
        assert!(
            serde_json::from_value::<SelectionIntent>(object).is_err(),
            "accepted field {field}"
        );
    }
}

fn assert_no_private_field_names(value: &serde_json::Value) {
    const PRIVATE: [&str; 10] = [
        "url",
        "path",
        "cookie",
        "authorization",
        "header",
        "digest",
        "stdout",
        "stderr",
        "argument",
        "commandline",
    ];
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object {
                assert!(
                    !PRIVATE.contains(&key.to_ascii_lowercase().as_str()),
                    "private field name serialized: {key}"
                );
                assert_no_private_field_names(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                assert_no_private_field_names(child);
            }
        }
        _ => {}
    }
}
