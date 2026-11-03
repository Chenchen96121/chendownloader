use local_media_downloader_lib::{
    domain::{
        download::DownloadErrorCode,
        format_selection::{SelectionIntent, SelectionMode},
        media::AnalysisErrorCode,
    },
    ipc::commands::{
        AnalyzeBilibiliRequest, BilibiliAnalysisRequest, BilibiliJobRequest,
        BilibiliResolutionRequest, PreviewBilibiliRequest, StartBilibiliDownloadRequest,
    },
    runtime::AppState,
};
use serde_json::{json, Value};
fn intent() -> SelectionIntent {
    SelectionIntent {
        mode: SelectionMode::BestAvailable,
        compatibility_mode: false,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}
#[test]
fn bilibili_runtime_respects_readiness_shutdown_and_platform_registry() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(root.path().to_owned());
    assert_eq!(
        state
            .analyze_bilibili("https://www.bilibili.com/video/BV1xx411c7mD", intent())
            .err(),
        Some(AnalysisErrorCode::ToolMissing)
    );
    assert_eq!(
        state.preview_bilibili("1", intent()).err(),
        Some(AnalysisErrorCode::ToolMissing)
    );
    assert_eq!(
        state.get_bilibili_cover("1").err(),
        Some(AnalysisErrorCode::ToolMissing)
    );
    assert_eq!(
        state.start_bilibili_download("1", "2", intent()).err(),
        Some(DownloadErrorCode::ToolMissing)
    );
    assert_eq!(
        state.cancel_bilibili_download("1"),
        Err(DownloadErrorCode::InvalidRequest)
    );
    assert_eq!(
        state.bilibili_download_result("1").err(),
        Some(DownloadErrorCode::InvalidRequest)
    );
    assert_eq!(
        state.open_bilibili_completed_location("1"),
        Err(DownloadErrorCode::InvalidRequest)
    );
    assert!(state.open_bilibili_source("1").is_err());
    assert!(!root.path().join("bilibili-download-jobs").exists());
    state.shutdown();
    state.shutdown();
    assert_eq!(
        state
            .analyze_bilibili("https://www.bilibili.com/video/BV1xx411c7mD", intent())
            .err(),
        Some(AnalysisErrorCode::AnalysisUnavailable)
    );
    assert_eq!(
        state.preview_bilibili("1", intent()).err(),
        Some(AnalysisErrorCode::AnalysisUnavailable)
    );
    assert_eq!(
        state.get_bilibili_cover("1").err(),
        Some(AnalysisErrorCode::AnalysisUnavailable)
    );
    assert_eq!(
        state.start_bilibili_download("1", "2", intent()).err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
}
#[test]
fn bilibili_requests_are_closed_without_cdn_identity_path_or_tool_authority() {
    fn check<T: serde::de::DeserializeOwned>(valid: Value) {
        assert!(serde_json::from_value::<T>(valid.clone()).is_ok());
        for (key, value) in [
            ("cid", json!("123")),
            ("sourceUrl", json!("https://evil.test")),
            ("headers", json!({})),
            ("path", json!("C:\\private")),
            ("cookie", json!("secret")),
            ("args", json!([])),
        ] {
            let mut extra = valid.clone();
            extra[key] = value;
            assert!(serde_json::from_value::<T>(extra).is_err(), "{key}");
        }
    }
    check::<AnalyzeBilibiliRequest>(
        json!({"url":"https://www.bilibili.com/video/BV1xx411c7mD?p=2","intent":{"mode":"best_available"}}),
    );
    check::<PreviewBilibiliRequest>(json!({"analysisId":"1","intent":{"mode":"best_available"}}));
    check::<BilibiliResolutionRequest>(
        json!({"analysisId":"1","mode":"manual","compatibilityMode":false}),
    );
    check::<BilibiliAnalysisRequest>(json!({"analysisId":"1"}));
    check::<StartBilibiliDownloadRequest>(
        json!({"analysisId":"1","destinationId":"2","intent":{"mode":"best_available"}}),
    );
    check::<BilibiliJobRequest>(json!({"jobId":"4"}));
}
#[test]
fn nine_bilibili_commands_only_have_main_window_native_permissions() {
    let build = include_str!("../build.rs");
    let handler = include_str!("../src/lib.rs");
    let commands = include_str!("../src/ipc/commands.rs");
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
    assert_eq!(capability["windows"], json!(["main"]));
    assert!(capability.get("remote").is_none());
    let permissions = capability["permissions"].as_array().unwrap();
    for command in [
        "analyze_bilibili",
        "preview_bilibili_selection",
        "get_bilibili_resolution_options",
        "get_bilibili_cover",
        "open_bilibili_source",
        "start_bilibili_download",
        "cancel_bilibili_download",
        "get_bilibili_download_result",
        "open_bilibili_completed_location",
    ] {
        assert!(build.contains(&format!("\"{command}\"")));
        assert!(handler.contains(&format!("ipc::commands::{command}")));
        assert!(commands.contains(&format!("fn {command}(")));
        assert!(permissions.contains(&json!(format!("allow-{}", command.replace('_', "-")))));
        let generated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("permissions/autogenerated")
            .join(format!("{command}.toml"));
        assert!(std::fs::read_to_string(generated)
            .unwrap()
            .contains(&format!("commands.allow = [\"{command}\"]")));
    }
    assert!(!permissions.iter().any(|p| p.as_str().is_some_and(|s| [
        "shell:", "http:", "opener:"
    ]
    .iter()
    .any(|prefix| s.starts_with(prefix)))));
}
