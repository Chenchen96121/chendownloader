use local_media_downloader_lib::{
    application::{
        download_activity::{DownloadActivityGate, DownloadOwner, TaskPhase},
        process::CancellationToken,
    },
    domain::{
        managed_tools::ToolName,
        media::{AnalysisErrorCode, Platform},
    },
    runtime::AppState,
};

#[test]
fn desktop_registers_each_tiktok_command_exactly_once() {
    let source = include_str!("../src/lib.rs");
    let build = include_str!("../build.rs");
    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
    let handler = source
        .split("tauri::generate_handler![")
        .nth(1)
        .unwrap()
        .split("])")
        .next()
        .unwrap();
    for command in [
        "analyze_tiktok",
        "get_tiktok_preview",
        "open_tiktok_source",
        "start_tiktok_download",
        "cancel_tiktok_download",
        "get_tiktok_download_result",
        "open_tiktok_completed_location",
        "get_tiktok_runtime_status",
        "analyze_x",
    ] {
        let registration = format!("ipc::commands::{command},");
        assert_eq!(handler.matches(&registration).count(), 1, "{command}");
        assert!(
            build.contains(&format!("\"{command}\"")),
            "build: {command}"
        );
        assert!(
            capability["permissions"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(format!(
                    "allow-{}",
                    command.replace('_', "-")
                ))),
            "permission: {command}"
        );
    }
    assert_eq!(capability["windows"], serde_json::json!(["main"]));
}

#[test]
fn optional_component_never_expands_core_gate_and_shutdown_is_terminal() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(root.path().to_owned());
    assert_eq!(ToolName::required().len(), 5);
    state.shutdown();
    assert_eq!(
        state.analyze_tiktok("https://www.tiktok.com/@sample/video/123"),
        Err(AnalysisErrorCode::AnalysisUnavailable)
    );
}
#[test]
fn all_five_platforms_share_fifo_and_independent_slots() {
    for limit in [1, 2, 4] {
        let gate = DownloadActivityGate::new();
        gate.set_limit(limit).unwrap();
        let owners = [
            DownloadOwner::Youtube,
            DownloadOwner::Instagram,
            DownloadOwner::Tiktok,
            DownloadOwner::X,
            DownloadOwner::Bilibili,
        ];
        let mut leases = owners
            .into_iter()
            .map(|owner| gate.reserve(owner, "task").unwrap())
            .collect::<Vec<_>>();
        for lease in leases.iter().take(limit as usize) {
            lease.wait_turn(&CancellationToken::new()).unwrap();
        }
        let snapshot = gate.snapshot().unwrap();
        assert_eq!(snapshot.tasks[2].platform, Platform::Tiktok);
        assert_eq!(
            snapshot
                .tasks
                .iter()
                .filter(|t| t.state == TaskPhase::Preparing)
                .count(),
            limit as usize
        );
        leases.remove(0);
        leases[limit as usize - 1]
            .wait_turn(&CancellationToken::new())
            .unwrap();
        assert_eq!(
            gate.snapshot()
                .unwrap()
                .tasks
                .iter()
                .filter(|t| t.state == TaskPhase::Preparing)
                .count(),
            limit as usize
        );
    }
}

#[test]
fn a_silent_declaration_cannot_drop_an_actual_audio_track() {
    use local_media_downloader_lib::{
        application::{
            process::{
                ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
                ProcessSupervisor,
            },
            tool_resolver::{ManagedToolResolver, ResolvedTool},
        },
        domain::{download::DownloadErrorCode, managed_tools::ManagedToolError},
        infrastructure::{ffmpeg::FfmpegMediaProcessor, tiktok::download::TikTokSourceInspector},
    };
    struct Resolver;
    impl ManagedToolResolver for Resolver {
        fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
            Ok(ResolvedTool {
                name,
                version: "fixture".into(),
                path: "C:/trusted/ffprobe.exe".into(),
            })
        }
    }
    struct Probe(bool);
    impl ProcessSupervisor for Probe {
        fn run(
            &self,
            request: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            assert!(request.args.iter().any(|a| a == "-protocol_whitelist"));
            let mut streams = vec![
                serde_json::json!({"codec_type":"video","codec_name":"h264","width":1080,"height":1920}),
            ];
            if self.0 {
                streams.push(serde_json::json!({"codec_type":"audio","codec_name":"aac"}));
            }
            Ok(ProcessExit {
                code: 0,
                stdout: serde_json::to_vec(
                    &serde_json::json!({"streams":streams,"format":{"format_name":"mp4"}}),
                )
                .unwrap(),
                stderr: vec![],
            })
        }
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.mp4");
    std::fs::write(&source, b"owned source").unwrap();
    for actual_audio in [false, true] {
        let processor = FfmpegMediaProcessor::new(Resolver, Probe(actual_audio));
        let result =
            processor.verify_silent_source(&source, root.path(), &CancellationToken::new());
        assert_eq!(
            result,
            if actual_audio {
                Err(DownloadErrorCode::ContentChanged)
            } else {
                Ok(())
            }
        );
    }
}
