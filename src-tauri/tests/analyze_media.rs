use local_media_downloader_lib::{
    application::{
        analyze_media::{AnalyzeMediaService, MetadataAnalyzer},
        media_url::ValidatedMediaUrl,
    },
    domain::{
        format_selection::{SelectionIntent, SelectionMode},
        media::{AnalysisErrorCode, FileSize, MediaFormat, MediaInfo, Platform, StreamKind},
        tool_provisioning::{
            ToolProvisioningErrorCode, ToolProvisioningState, ToolProvisioningStatus,
        },
    },
};

#[test]
fn runtime_rejects_live_analysis_before_local_components_are_ready() {
    use local_media_downloader_lib::runtime::AppState;

    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(root.path().into());
    let intent = SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::BestAvailable,
        video_format_id: None,
        output_container: None,
        clip: None,
    };

    assert_eq!(
        state.analyze_media("not even a URL", intent),
        Err(AnalysisErrorCode::ToolMissing)
    );
}

#[test]
fn live_analysis_is_gated_by_the_closed_provisioning_status() {
    assert_eq!(ToolProvisioningStatus::ready().analysis_readiness(), Ok(()));
    for status in [
        ToolProvisioningStatus::preparing(),
        ToolProvisioningStatus::repairing(),
        ToolProvisioningStatus::failed(ToolProvisioningErrorCode::PackageMissing),
    ] {
        assert_eq!(
            status.analysis_readiness(),
            Err(AnalysisErrorCode::ToolMissing)
        );
    }
    for code in [
        ToolProvisioningErrorCode::PackageInvalid,
        ToolProvisioningErrorCode::ToolProbeFailed,
        ToolProvisioningErrorCode::ActivationFailed,
    ] {
        let status =
            ToolProvisioningStatus::new(ToolProvisioningState::Failed, Some(code)).unwrap();
        assert_eq!(
            status.analysis_readiness(),
            Err(AnalysisErrorCode::ToolIntegrityFailed)
        );
    }
}

// This port has no production adapter in this slice. The double supplies complete,
// hand-authored normalized metadata, while URL and response validation remain real.
struct AnalyzerDouble {
    response: Result<MediaInfo, AnalysisErrorCode>,
}

impl MetadataAnalyzer for AnalyzerDouble {
    fn analyze(&self, source: &ValidatedMediaUrl) -> Result<MediaInfo, AnalysisErrorCode> {
        assert_eq!(source.platform(), Platform::Youtube);
        assert_eq!(source.media_id(), "demoYT00001");
        assert_eq!(
            source.url(),
            "https://youtube.com/watch?v=demoYT00001&t=5#part"
        );
        self.response.clone()
    }
}

fn literal_media() -> MediaInfo {
    MediaInfo {
        platform: Platform::Youtube,
        media_id: "demoYT00001".into(),
        title: "Neutral analysis example".into(),
        creator: Some("Sample creator".into()),
        duration_seconds: Some(12.5),
        formats: vec![MediaFormat {
            id: "combined-1".into(),
            container: "mp4".into(),
            kind: StreamKind::Combined,
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            video_bitrate_kbps: None,
            audio_bitrate_kbps: None,
            audio_sample_rate_hz: None,
            audio_channels: None,
            video_codec: Some("avc1.64001f".into()),
            audio_codec: Some("mp4a.40.2".into()),
            dynamic_range: Some("SDR".into()),
            file_size: Some(FileSize {
                bytes: 12000,
                is_approximate: false,
            }),
        }],
    }
}

const INPUT: &str = " https://YOUTUBE.COM:443/watch?v=demoYT00001&t=5#part ";

// Catches skipped port invocation, changed metadata, or a noncanonical source.
#[test]
fn valid_source_returns_the_complete_normalized_response() {
    let analyzer = AnalyzerDouble {
        response: Ok(literal_media()),
    };
    let media = AnalyzeMediaService::new(&analyzer).analyze(INPUT).unwrap();
    assert_eq!(
        serde_json::to_value(media).unwrap(),
        serde_json::json!({
            "platform": "youtube", "mediaId": "demoYT00001",
            "title": "Neutral analysis example", "creator": "Sample creator",
            "durationSeconds": 12.5,
            "formats": [{
                "id": "combined-1", "container": "mp4", "kind": "combined",
                "width": 1280, "height": 720, "fps": 30.0,
                "videoCodec": "avc1.64001f", "audioCodec": "mp4a.40.2",
                "dynamicRange": "SDR", "fileSize": {"bytes": 12000, "isApproximate": false}
            }]
        })
    );
}

struct NeverAnalyze;

impl MetadataAnalyzer for NeverAnalyze {
    fn analyze(&self, _: &ValidatedMediaUrl) -> Result<MediaInfo, AnalysisErrorCode> {
        panic!("rejected URLs must not reach the analyzer")
    }
}

// Catches consulting the provider before URL validation or losing URL error codes.
#[test]
fn rejected_urls_never_consult_the_analyzer() {
    let service = AnalyzeMediaService::new(&NeverAnalyze);
    for (input, expected) in [
        (
            "http://youtube.com/watch?v=demoYT00001",
            AnalysisErrorCode::InvalidUrl,
        ),
        (
            "https://example.invalid/watch?v=demoYT00001",
            AnalysisErrorCode::UnsupportedPlatform,
        ),
        (
            "https://youtube.com/playlist?list=neutral",
            AnalysisErrorCode::UnsupportedMedia,
        ),
    ] {
        assert_eq!(service.analyze(input), Err(expected), "{input}");
    }
}

// Catches replacing a stable provider failure with a different error or success.
#[test]
fn stable_provider_failures_propagate_unchanged() {
    for expected in [
        AnalysisErrorCode::InvalidUrl,
        AnalysisErrorCode::UnsupportedPlatform,
        AnalysisErrorCode::UnsupportedMedia,
        AnalysisErrorCode::InvalidMetadata,
        AnalysisErrorCode::FormatUnavailable,
        AnalysisErrorCode::AnalysisUnavailable,
        AnalysisErrorCode::ToolMissing,
        AnalysisErrorCode::ToolIntegrityFailed,
        AnalysisErrorCode::AuthenticationRequired,
        AnalysisErrorCode::AccessDenied,
        AnalysisErrorCode::ContentUnavailable,
        AnalysisErrorCode::ContentChanged,
        AnalysisErrorCode::RateLimited,
        AnalysisErrorCode::ExtractorOutdated,
        AnalysisErrorCode::NetworkFailed,
        AnalysisErrorCode::AnalysisTimeout,
        AnalysisErrorCode::ProcessFailed,
        AnalysisErrorCode::Unknown,
    ] {
        let analyzer = AnalyzerDouble {
            response: Err(expected),
        };
        assert_eq!(
            AnalyzeMediaService::new(&analyzer).analyze(INPUT),
            Err(expected)
        );
    }
}

// Catches accepting metadata for another platform or another resource.
#[test]
fn response_identity_must_match_the_validated_source() {
    let mut wrong_platform = literal_media();
    wrong_platform.platform = Platform::Instagram;
    let mut wrong_id = literal_media();
    wrong_id.media_id = "otherYT0001".into();
    for response in [wrong_platform, wrong_id] {
        let analyzer = AnalyzerDouble {
            response: Ok(response),
        };
        assert_eq!(
            AnalyzeMediaService::new(&analyzer).analyze(INPUT),
            Err(AnalysisErrorCode::InvalidMetadata)
        );
    }
}

// Catches removing domain validation after the injected producer returns.
#[test]
fn invalid_response_format_invariants_are_rejected() {
    let mut empty = literal_media();
    empty.formats.clear();
    let mut duplicate = literal_media();
    duplicate.formats.push(duplicate.formats[0].clone());
    let mut mismatched_stream = literal_media();
    mismatched_stream.formats[0].audio_codec = None;
    let mut invalid_fps = literal_media();
    invalid_fps.formats[0].fps = Some(f64::NAN);
    let mut invalid_title = literal_media();
    invalid_title.title = " untrimmed ".into();
    for response in [
        empty,
        duplicate,
        mismatched_stream,
        invalid_fps,
        invalid_title,
    ] {
        let analyzer = AnalyzerDouble {
            response: Ok(response),
        };
        assert_eq!(
            AnalyzeMediaService::new(&analyzer).analyze(INPUT),
            Err(AnalysisErrorCode::InvalidMetadata)
        );
    }
}

// Catches wrong fixture selection, depending on tools, or creating a catalog.
#[test]
fn runtime_previews_each_platform_without_tools_or_managed_catalogs() {
    use local_media_downloader_lib::{
        domain::managed_tools::{ToolState, BUILD_TARGET},
        runtime::AppState,
    };

    for (platform, expected_id) in [
        (Platform::Youtube, "sample-youtube"),
        (Platform::Bilibili, "sample-bilibili"),
        (Platform::X, "sample-x"),
        (Platform::Instagram, "sample-instagram"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(root.path().into());
        let media = state.preview_media_fixture(platform).unwrap();
        assert_eq!(media.platform, platform);
        assert_eq!(media.media_id, expected_id);
        assert!(!media.formats.is_empty());
        media.validate().unwrap();
        assert_eq!(state.tool_inventory().tools[0].state, ToolState::Missing);
        let target = root.path().join("managed-tools").join(BUILD_TARGET);
        for name in [
            "catalog.json",
            "catalog.previous.json",
            "catalog.pending.json",
        ] {
            assert!(!target.join(name).exists(), "preview created {name}");
        }
    }
}
