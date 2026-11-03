use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use local_media_downloader_lib::{
    application::{
        analyze_media::MetadataAnalyzer, live_analysis::LiveAnalysisService,
        media_url::ValidatedMediaUrl, preview_selection::SelectionOutcome,
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::{SelectionIntent, SelectionMode},
        media::{AnalysisErrorCode, MediaFormat, MediaInfo, Platform, StreamKind},
    },
};

#[derive(Default)]
struct Analyzer {
    canonical_sources: Arc<Mutex<Vec<String>>>,
}

impl MetadataAnalyzer for Analyzer {
    fn analyze(&self, source: &ValidatedMediaUrl) -> Result<MediaInfo, AnalysisErrorCode> {
        self.canonical_sources
            .lock()
            .unwrap()
            .push(source.url().to_owned());
        Ok(MediaInfo {
            platform: Platform::Youtube,
            media_id: source.media_id().to_owned(),
            title: "Live session example".into(),
            creator: None,
            duration_seconds: Some(60.0),
            formats: vec![
                MediaFormat {
                    id: "combined-720".into(),
                    container: "mp4".into(),
                    kind: StreamKind::Combined,
                    width: Some(1280),
                    height: Some(720),
                    fps: Some(30.0),
                    video_bitrate_kbps: Some(1800.0),
                    audio_bitrate_kbps: Some(128.0),
                    audio_sample_rate_hz: Some(48_000),
                    audio_channels: Some(2),
                    video_codec: Some("avc1.64001f".into()),
                    audio_codec: Some("mp4a.40.2".into()),
                    dynamic_range: Some("SDR".into()),
                    file_size: None,
                },
                MediaFormat {
                    id: "audio-1".into(),
                    container: "m4a".into(),
                    kind: StreamKind::AudioOnly,
                    width: None,
                    height: None,
                    fps: None,
                    video_bitrate_kbps: None,
                    audio_bitrate_kbps: Some(160.0),
                    audio_sample_rate_hz: Some(48_000),
                    audio_channels: Some(2),
                    video_codec: None,
                    audio_codec: Some("mp4a.40.2".into()),
                    dynamic_range: None,
                    file_size: None,
                },
            ],
        })
    }
}

fn best() -> SelectionIntent {
    SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::BestAvailable,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}

fn manual() -> SelectionIntent {
    SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::Manual,
        video_format_id: Some("combined-720".into()),
        output_container: None,
        clip: None,
    }
}

const FIRST: &str = " https://YOUTUBE.COM:443/watch?v=demoYT00001 ";
const SECOND: &str = "https://youtu.be/demoYT00002";

struct DualContainerAnalyzer;
impl MetadataAnalyzer for DualContainerAnalyzer {
    fn analyze(&self, source: &ValidatedMediaUrl) -> Result<MediaInfo, AnalysisErrorCode> {
        let mut media = Analyzer::default().analyze(source)?;
        media.formats[0].video_codec = Some("vp09.00.31.08".into());
        Ok(media)
    }
}

#[test]
fn resolution_rows_enumerate_same_video_in_both_safe_target_containers() {
    let service = LiveAnalysisService::new(DualContainerAnalyzer);
    let now = Instant::now();
    let preview = service.analyze(FIRST, best(), now).unwrap();
    for mode in [SelectionMode::Manual, SelectionMode::VideoOnly] {
        let rows = service
            .resolution_options(&preview.analysis_id, mode, false, now)
            .unwrap();
        assert_eq!(rows.options.len(), 2);
        assert_eq!(rows.options[0].video_format_id, "combined-720");
        assert_eq!(rows.options[1].video_format_id, "combined-720");
        assert_eq!(
            serde_json::to_value(rows.options[0].output_container).unwrap(),
            "mp4"
        );
        assert_eq!(
            serde_json::to_value(rows.options[1].output_container).unwrap(),
            "webm"
        );
        for row in rows.options {
            match row.outcome {
                SelectionOutcome::Selected { plan } => assert_eq!(
                    plan.output_container.as_deref(),
                    Some(row.output_container.as_str())
                ),
                SelectionOutcome::Unavailable { code } => assert_eq!(code, local_media_downloader_lib::domain::format_selection::SelectionUnavailableCode::InvalidSelection),
            }
        }
    }
}

#[test]
fn original_page_resolves_only_a_current_native_canonical_source() {
    let service = LiveAnalysisService::new(Analyzer::default());
    let now = Instant::now();
    let first = service.analyze(FIRST, best(), now).unwrap();
    assert_eq!(
        service
            .resolve_media_page(&first.analysis_id, now)
            .unwrap()
            .url(),
        "https://youtube.com/watch?v=demoYT00001"
    );
    for id in [
        "",
        "0",
        "01",
        "https://evil.invalid",
        "file:///C:/private",
        "99",
    ] {
        assert!(matches!(
            service.resolve_media_page(id, now),
            Err(AnalysisErrorCode::AnalysisUnavailable)
        ));
    }
    let second = service.analyze(SECOND, best(), now).unwrap();
    assert!(service.resolve_media_page(&first.analysis_id, now).is_err());
    assert!(service
        .resolve_media_page(&second.analysis_id, now + Duration::from_secs(900))
        .is_err());
}

#[test]
fn resolution_rows_use_native_plans_and_current_session_only() {
    let service = LiveAnalysisService::new(Analyzer::default());
    let now = Instant::now();
    let preview = service.analyze(FIRST, best(), now).unwrap();
    let manual = service
        .resolution_options(&preview.analysis_id, SelectionMode::Manual, false, now)
        .unwrap();
    assert_eq!(manual.options.len(), 2);
    assert!(!manual.compatibility_mode);
    assert_eq!(manual.options[0].video_format_id, "combined-720");
    match &manual.options[0].outcome {
        SelectionOutcome::Selected { plan } => {
            assert_eq!(plan.output_container.as_deref(), Some("mp4"))
        }
        _ => panic!("manual option unavailable"),
    }
    let video = service
        .resolution_options(&preview.analysis_id, SelectionMode::VideoOnly, false, now)
        .unwrap();
    match &video.options[0].outcome {
        SelectionOutcome::Selected { plan } => {
            assert_eq!(plan.operation, local_media_downloader_lib::domain::format_selection::SelectionOperation::ExtractVideo);
            assert_eq!(plan.estimated_size_bytes, None);
        }
        _ => panic!("video option unavailable"),
    }
    assert!(service
        .resolution_options(&preview.analysis_id, SelectionMode::AudioOnly, false, now)
        .is_err());
    assert!(service
        .resolution_options(
            &preview.analysis_id,
            SelectionMode::Manual,
            false,
            now + Duration::from_secs(900)
        )
        .is_err());
}

#[test]
fn compatibility_options_keep_the_video_source_and_expose_only_mp4() {
    let service = LiveAnalysisService::new(Analyzer::default());
    let now = Instant::now();
    let preview = service.analyze(FIRST, best(), now).unwrap();
    let rows = service
        .resolution_options(&preview.analysis_id, SelectionMode::Manual, true, now)
        .unwrap();
    assert!(rows.compatibility_mode);
    assert_eq!(rows.options.len(), 1);
    assert_eq!(rows.options[0].video_format_id, "combined-720");
    let SelectionOutcome::Selected { plan } = &rows.options[0].outcome else {
        panic!("supported compatibility candidate unavailable");
    };
    assert!(plan.compatibility_mode);
    assert_eq!(plan.source_format_ids, ["combined-720", "audio-1"]);
    assert_eq!(plan.output_container.as_deref(), Some("mp4"));
}

#[test]
fn analyze_retains_the_canonical_source_only_in_native_state() {
    let analyzer = Analyzer::default();
    let canonical_sources = Arc::clone(&analyzer.canonical_sources);
    let service = LiveAnalysisService::new(analyzer);
    let now = Instant::now();
    let preview = service.analyze(FIRST, best(), now).unwrap();
    assert_eq!(preview.analysis_id, "1");
    assert_eq!(preview.media.media_id, "demoYT00001");
    assert_eq!(preview.ranked_video_format_ids, ["combined-720"]);
    assert_eq!(preview.ranked_audio_format_ids, ["audio-1", "combined-720"]);
    assert!(matches!(preview.outcome, SelectionOutcome::Selected { .. }));

    let serialized = serde_json::to_string(&preview).unwrap();
    assert!(!serialized.contains("youtube.com"));
    assert!(!serialized.contains("sourceUrl"));
    assert_eq!(
        canonical_sources.lock().unwrap().as_slice(),
        ["https://youtube.com/watch?v=demoYT00001"]
    );
}

#[test]
fn preview_reselects_from_the_active_sanitized_media() {
    let service = LiveAnalysisService::new(Analyzer::default());
    let now = Instant::now();
    let analyzed = service.analyze(FIRST, best(), now).unwrap();
    let selected = service
        .preview(
            &analyzed.analysis_id,
            manual(),
            now + Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(selected.analysis_id, analyzed.analysis_id);
    match selected.outcome {
        SelectionOutcome::Selected { plan } => {
            assert_eq!(plan.mode, SelectionMode::Manual);
            assert_eq!(plan.source_format_ids, ["combined-720", "audio-1"]);
        }
        SelectionOutcome::Unavailable { code } => panic!("manual selection failed: {code:?}"),
    }
}

#[test]
fn download_resolution_rechecks_session_selection_and_keeps_the_source_native() {
    let service = LiveAnalysisService::new(Analyzer::default());
    let now = Instant::now();
    let analyzed = service.analyze(FIRST, best(), now).unwrap();
    let resolved = service
        .resolve_download(
            &analyzed.analysis_id,
            manual(),
            now + Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(
        resolved.source.url(),
        "https://youtube.com/watch?v=demoYT00001"
    );
    assert_eq!(resolved.media.media_id, "demoYT00001");
    assert_eq!(resolved.plan.mode, SelectionMode::Manual);
    assert_eq!(resolved.plan.source_format_ids, ["combined-720", "audio-1"]);
    assert_eq!(
        service
            .resolve_download("0", best(), now + Duration::from_secs(1))
            .err(),
        Some(DownloadErrorCode::AnalysisUnavailable)
    );
}

#[test]
fn sessions_expire_at_fifteen_minutes_and_ids_remain_monotonic() {
    let service = LiveAnalysisService::new(Analyzer::default());
    let now = Instant::now();
    let first = service.analyze(FIRST, best(), now).unwrap();
    assert!(service
        .preview(
            &first.analysis_id,
            best(),
            now + Duration::from_millis(899_999),
        )
        .is_ok());
    assert_eq!(
        service.preview(&first.analysis_id, best(), now + Duration::from_secs(900),),
        Err(AnalysisErrorCode::AnalysisUnavailable)
    );
    let second = service
        .analyze(SECOND, best(), now + Duration::from_secs(901))
        .unwrap();
    assert_eq!(second.analysis_id, "2");
}

#[test]
fn replacement_invalidates_stale_and_unknown_ids() {
    let service = LiveAnalysisService::new(Analyzer::default());
    let now = Instant::now();
    let first = service.analyze(FIRST, best(), now).unwrap();
    let second = service
        .analyze(SECOND, best(), now + Duration::from_secs(1))
        .unwrap();
    assert_ne!(first.analysis_id, second.analysis_id);
    for stale in [&first.analysis_id, "0", "01", "-1", "unknown"] {
        assert_eq!(
            service.preview(stale, best(), now + Duration::from_secs(2)),
            Err(AnalysisErrorCode::AnalysisUnavailable),
            "{stale}"
        );
    }
    assert!(service
        .preview(&second.analysis_id, best(), now + Duration::from_secs(2))
        .is_ok());
}

#[test]
fn concurrent_reads_and_replacement_keep_one_coherent_session() {
    let service = Arc::new(LiveAnalysisService::new(Analyzer::default()));
    let now = Instant::now();
    let first = service.analyze(FIRST, best(), now).unwrap();
    let readers = (0..8)
        .map(|_| {
            let service = Arc::clone(&service);
            let id = first.analysis_id.clone();
            thread::spawn(move || service.preview(&id, best(), now + Duration::from_secs(1)))
        })
        .collect::<Vec<_>>();
    for reader in readers {
        assert_eq!(
            reader.join().unwrap().unwrap().analysis_id,
            first.analysis_id
        );
    }

    let replacing = {
        let service = Arc::clone(&service);
        thread::spawn(move || service.analyze(SECOND, best(), now + Duration::from_secs(2)))
    };
    let second = replacing.join().unwrap().unwrap();
    assert_eq!(
        service.preview(&first.analysis_id, best(), now + Duration::from_secs(3)),
        Err(AnalysisErrorCode::AnalysisUnavailable)
    );
    assert_eq!(
        service
            .preview(&second.analysis_id, best(), now + Duration::from_secs(3))
            .unwrap()
            .media
            .media_id,
        "demoYT00002"
    );
}
