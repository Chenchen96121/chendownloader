#[path = "support/bilibili.rs"]
mod fixtures;
use local_media_downloader_lib::{
    application::{
        bilibili_analysis::{BilibiliAnalysisService, BilibiliCoverLoader},
        bilibili_download::BilibiliDownloadRefresher,
        bilibili_source::{BilibiliPageEvidence, BilibiliRequestContext},
        download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
        download_job::JobStaging,
        local_processing::{
            LocalMediaProcessor, LocalProcessingRequest, ProcessedMedia, ProcessingEvidence,
        },
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
    },
    domain::{
        bilibili::BilibiliCoverPayload,
        download::{DownloadErrorCode, DownloadProgress},
        format_selection::{ClipRange, SelectionIntent, SelectionMode},
        media::AnalysisErrorCode,
        output_policy::{OutputContainer, StreamProcessing},
    },
    infrastructure::{
        output::FilesystemJobStaging,
        ytdlp::{bilibili_download::validate_bilibili_transfer, download::YtDlpDownloadExecutor},
    },
};
use serde_json::Value;
use std::{
    fs,
    sync::{Arc, Mutex},
    time::Instant,
};
struct Covers;
impl BilibiliCoverLoader for Covers {
    fn load(
        &self,
        _: &BilibiliPageEvidence,
        _: &BilibiliRequestContext,
    ) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        Err(AnalysisErrorCode::AnalysisUnavailable)
    }
}
fn intent(mode: SelectionMode) -> SelectionIntent {
    SelectionIntent {
        mode,
        compatibility_mode: false,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}
fn request(root: &std::path::Path, intent: SelectionIntent) -> DownloadExecutionRequest {
    request_using(root, intent, Arc::new(fixtures::FixtureState::default()))
}
fn request_using(
    root: &std::path::Path,
    intent: SelectionIntent,
    state: Arc<fixtures::FixtureState>,
) -> DownloadExecutionRequest {
    let service = BilibiliAnalysisService::new(fixtures::Analyzer(state), Covers);
    let now = Instant::now();
    let p = service.analyze(fixtures::URL, intent.clone(), now).unwrap();
    let snapshot = service
        .prepare_download(&p.analysis_id, intent, now)
        .unwrap();
    let source = service
        .refresh(&snapshot, CancellationToken::new())
        .unwrap();
    DownloadExecutionRequest {
        job_id: "81".into(),
        source: snapshot.source().clone(),
        playlist_item: None,
        instagram_selector: None,
        tiktok_source: None,
        x_source: None,
        bilibili_source: Some(source),
        plan: snapshot.plan().clone(),
        video_source: snapshot
            .media()
            .formats
            .iter()
            .find(|f| Some(&f.id) == snapshot.plan().video_format_id.as_ref())
            .cloned(),
        audio_source: snapshot
            .media()
            .formats
            .iter()
            .find(|f| Some(&f.id) == snapshot.plan().audio_format_id.as_ref())
            .cloned(),
        staging_dir: FilesystemJobStaging::new(root.to_owned())
            .create("81")
            .unwrap(),
    }
}
#[derive(Default)]
struct Process {
    calls: Mutex<Vec<Vec<String>>>,
    change_color: bool,
    short: bool,
    failure: Option<ProcessFailureKind>,
    cancel: bool,
    error_stderr: Option<&'static [u8]>,
    video: Option<Value>,
    secondary_video: Option<Value>,
    video_frame: Option<Value>,
    audio: Option<Value>,
}
impl ProcessSupervisor for Process {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let args: Vec<_> = request
            .args
            .iter()
            .map(|s| s.to_str().unwrap().to_owned())
            .collect();
        self.calls.lock().unwrap().push(args.clone());
        if request.program.path().file_name().unwrap() == "yt-dlp.exe" {
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            if self.cancel {
                cancellation.cancel();
            }
            if let Some(stderr) = self.error_stderr {
                return Ok(ProcessExit {
                    code: 1,
                    stdout: vec![],
                    stderr: stderr.into(),
                });
            }
            let info = args
                .windows(2)
                .find(|p| p[0] == "--load-info-json")
                .expect("only frozen info; no page re-extraction");
            let value: Value = serde_json::from_slice(&fs::read(&info[1]).unwrap()).unwrap();
            assert_eq!(value["formats"].as_array().unwrap().len(), 1);
            assert!(value["formats"][0]["url"]
                .as_str()
                .unwrap()
                .contains("SYNTHETIC"));
            assert!(value.get("http_headers").is_none());
            assert_eq!(
                args.windows(2).find(|p| p[0] == "--referer").unwrap()[1],
                fixtures::URL
            );
            let guards = &args
                .windows(2)
                .find(|p| p[0] == "--downloader-args")
                .unwrap()[1];
            for required in [
                "-max_redirects 0",
                "-rw_timeout 8000000",
                "-protocol_whitelist https,http,tcp,tls,httpproxy",
                "-format_whitelist mov,matroska,webm,flac,aac,mp3,ogg",
                "-http_proxy ",
            ] {
                assert!(guards.contains(required), "{required}");
            }
            fs::write(
                request
                    .working_dir
                    .join(if value["formats"][0]["vcodec"] == "none" {
                        "source.m4a"
                    } else {
                        "source.mp4"
                    }),
                b"synthetic complete source",
            )
            .unwrap();
            return Ok(ProcessExit {
                code: 0,
                stdout: vec![],
                stderr: vec![],
            });
        }
        let is_audio = args.last().unwrap().ends_with("source.m4a");
        let mut value =
            if args.last().unwrap().contains("input-1") && self.secondary_video.is_some() {
                self.secondary_video.clone().unwrap()
            } else if is_audio {
                self.audio.clone().unwrap_or_else(fixtures::audio_probe)
            } else {
                self.video.clone().unwrap_or_else(fixtures::video_probe)
            };
        assert!(
            args.windows(2)
                .any(|p| p == ["-protocol_whitelist", "file"]),
            "local probes cannot open network inputs"
        );
        if args.iter().any(|a| a == "-show_frames") {
            if let Some(frame) = &self.video_frame {
                return Ok(ProcessExit {
                    code: 0,
                    stdout: serde_json::to_vec(&serde_json::json!({"frames":[frame]})).unwrap(),
                    stderr: vec![],
                });
            }
            let stream = &value["streams"][0];
            return Ok(ProcessExit{code:0,stdout:serde_json::to_vec(&serde_json::json!({"frames":[{"pix_fmt":stream["pix_fmt"],"color_range":stream["color_range"],"color_space":stream["color_space"],"color_transfer":stream["color_transfer"],"color_primaries":stream["color_primaries"],"side_data_list":[]}]})).unwrap(),stderr:vec![]});
        }
        for stream in value["streams"].as_array_mut().unwrap() {
            stream.as_object_mut().unwrap().remove("bit_rate");
        }
        if self.change_color && !is_audio {
            value["streams"][0]["color_range"] = "pc".into();
        }
        if self.short {
            value["format"]["duration"] = "10".into();
        }
        Ok(ProcessExit {
            code: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: vec![],
        })
    }
}
struct Processor {
    calls: Mutex<usize>,
    evidence: ProcessingEvidence,
}
impl LocalMediaProcessor for Processor {
    fn process(
        &self,
        request: LocalProcessingRequest,
        _: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode> {
        *self.calls.lock().unwrap() += 1;
        fs::write(
            request.staging_dir.join(format!(
                "stage.{}",
                request.plan.output_container.as_deref().unwrap()
            )),
            b"verified stage",
        )
        .unwrap();
        Ok(ProcessedMedia {
            evidence: self.evidence.clone(),
        })
    }
}
fn processor(request: &DownloadExecutionRequest) -> Processor {
    let source = request.bilibili_source.as_ref().unwrap();
    let video = request
        .plan
        .video_format_id
        .as_deref()
        .map(|id| source.original_properties(id).unwrap().0.unwrap().clone());
    let audio = request
        .plan
        .audio_format_id
        .as_deref()
        .map(|id| source.original_properties(id).unwrap().1.unwrap().clone());
    let decision = request
        .plan
        .compatibility_mode
        .then(|| {
            local_media_downloader_lib::infrastructure::ffmpeg::compatibility::decide_compatibility(
                video.as_ref(),
                audio.as_ref(),
            )
            .ok()
        })
        .flatten();
    let video_processing = decision
        .as_ref()
        .map(|d| d.video_processing)
        .unwrap_or(request.plan.video_processing);
    let audio_processing = decision
        .as_ref()
        .map(|d| d.audio_processing)
        .unwrap_or(request.plan.audio_processing);
    Processor {
        calls: Mutex::new(0),
        evidence: ProcessingEvidence {
            source_video: video,
            source_audio: audio,
            actual_container: OutputContainer::parse(
                request.plan.output_container.as_deref().unwrap(),
            )
            .unwrap(),
            video_processing,
            audio_processing,
        },
    }
}
struct Progress;
impl DownloadProgressSink for Progress {
    fn on_progress(&self, _: DownloadProgress) {}
}

fn combined_state(distinct_roles: bool) -> Arc<fixtures::FixtureState> {
    let state = Arc::new(fixtures::FixtureState::default());
    let mut video = fixtures::video_probe();
    let mut audio = fixtures::audio_probe()["streams"][0].clone();
    audio["bit_rate"] = serde_json::json!(if distinct_roles { "96000" } else { "192000" });
    video["streams"].as_array_mut().unwrap().push(audio);
    *state.video.lock().unwrap() = video;
    let mut metadata = state.metadata.lock().unwrap();
    let mut primary = metadata["formats"][0].clone();
    primary.as_object_mut().unwrap().remove("tbr");
    primary["acodec"] = "mp4a.40.2".into();
    primary["vbr"] = 3500.into();
    primary["abr"] = (if distinct_roles { 96 } else { 192 }).into();
    primary["asr"] = 48000.into();
    primary["audio_channels"] = 2.into();
    if distinct_roles {
        let mut secondary = primary.clone();
        secondary["format_id"] = "combined-audio".into();
        secondary["url"] = metadata["formats"][1]["url"].clone();
        secondary["width"] = 1280.into();
        secondary["height"] = 720.into();
        secondary["vbr"] = 1500.into();
        secondary["abr"] = 192.into();
        let mut probe = state.video.lock().unwrap().clone();
        probe["streams"][0]["width"] = 1280.into();
        probe["streams"][0]["height"] = 720.into();
        probe["streams"][0]["bit_rate"] = "1500000".into();
        probe["streams"][1]["bit_rate"] = "192000".into();
        *state.audio.lock().unwrap() = probe;
        metadata["formats"] = serde_json::json!([primary, secondary]);
    } else {
        metadata["formats"] = serde_json::json!([primary]);
    }
    drop(metadata);
    state
}

#[test]
fn bilibili_combined_source_keeps_only_selected_output_roles_but_verifies_every_stream() {
    for mode in [SelectionMode::VideoOnly, SelectionMode::AudioOnly] {
        for tampered in [false, true] {
            let state = combined_state(false);
            let mut physical = state.video.lock().unwrap().clone();
            if tampered {
                // Even the discarded physical role is still independently verified.
                if mode == SelectionMode::AudioOnly {
                    physical["streams"][0]["color_range"] = "pc".into();
                } else {
                    physical["streams"][1]["sample_rate"] = "44100".into();
                }
            }
            let root = tempfile::tempdir().unwrap();
            let request = request_using(root.path(), intent(mode), state);
            let processor = processor(&request);
            let result = YtDlpDownloadExecutor::new(
                fixtures::Resolver,
                Process {
                    video: Some(physical),
                    ..Default::default()
                },
                &processor,
            )
            .download(request, CancellationToken::new(), &Progress);
            if tampered {
                assert!(matches!(result, Err(DownloadErrorCode::OutputInvalid)));
                assert_eq!(*processor.calls.lock().unwrap(), 0);
            } else {
                let staged = result.unwrap_or_else(|error| panic!("combined {mode:?}: {error:?}"));
                assert_eq!(
                    staged.evidence.source_video.is_some(),
                    mode == SelectionMode::VideoOnly
                );
                assert_eq!(
                    staged.evidence.source_audio.is_some(),
                    mode == SelectionMode::AudioOnly
                );
                assert_eq!(*processor.calls.lock().unwrap(), 1);
            }
        }
    }
}

#[test]
fn bilibili_distinct_combined_sources_bind_evidence_to_each_selected_role() {
    let state = combined_state(true);
    let physical = state.video.lock().unwrap().clone();
    let secondary = state.audio.lock().unwrap().clone();
    let root = tempfile::tempdir().unwrap();
    let request = request_using(root.path(), intent(SelectionMode::BestAvailable), state);
    assert_ne!(request.plan.video_format_id, request.plan.audio_format_id);
    let processor = processor(&request);
    let staged = YtDlpDownloadExecutor::new(
        fixtures::Resolver,
        Process {
            video: Some(physical),
            secondary_video: Some(secondary),
            ..Default::default()
        },
        &processor,
    )
    .download(request, CancellationToken::new(), &Progress)
    .unwrap();
    let video = staged.evidence.source_video.unwrap();
    assert_eq!((video.width, video.height), (1920, 1080));
    assert_eq!(staged.evidence.source_audio.unwrap().sample_rate_hz, 48000);
}

#[test]
fn bilibili_download_uses_frozen_sources_and_guarded_lossless_transfer() {
    let root = tempfile::tempdir().unwrap();
    let request = request(root.path(), intent(SelectionMode::BestAvailable));
    assert_eq!(request.plan.video_processing, Some(StreamProcessing::Copy));
    assert!(validate_bilibili_transfer(&request).is_ok());
    let processor = processor(&request);
    let process = Process::default();
    let staged = YtDlpDownloadExecutor::new(fixtures::Resolver, &process, &processor)
        .download(request, CancellationToken::new(), &Progress)
        .unwrap();
    assert_eq!(*processor.calls.lock().unwrap(), 1);
    assert!(!staged.staging_dir.join("source").exists());
}

#[test]
fn bilibili_copy_reuses_existing_rate_equivalence_without_accepting_changed_fps() {
    for (rate, accepted) in [("48/2", true), ("270625/11276", true), ("25/1", false)] {
        let state = Arc::new(fixtures::FixtureState::default());
        state.metadata.lock().unwrap()["formats"][0]["fps"] = serde_json::json!(24);
        state.video.lock().unwrap()["streams"][0]["avg_frame_rate"] = "24/1".into();
        let mut actual = state.video.lock().unwrap().clone();
        actual["streams"][0]["avg_frame_rate"] = rate.into();
        let root = tempfile::tempdir().unwrap();
        let request = request_using(root.path(), intent(SelectionMode::BestAvailable), state);
        let processor = processor(&request);
        let result = YtDlpDownloadExecutor::new(
            fixtures::Resolver,
            Process {
                video: Some(actual),
                ..Default::default()
            },
            &processor,
        )
        .download(request, CancellationToken::new(), &Progress);
        assert_eq!(
            result.is_ok(),
            accepted,
            "rate {rate}: remux rational rounding uses existing policy, real FPS changes reject"
        );
    }
}
#[test]
fn bilibili_compatibility_accepts_proven_color_refinement_and_already_compatible_copy() {
    let state = Arc::new(fixtures::FixtureState::default());
    state.video.lock().unwrap()["streams"][0]["color_transfer"] = Value::Null;
    state.video.lock().unwrap()["streams"][0]["color_primaries"] = Value::Null;
    let video = state.video.lock().unwrap().clone();
    let root = tempfile::tempdir().unwrap();
    let mut selected = intent(SelectionMode::BestAvailable);
    selected.compatibility_mode = true;
    let request = request_using(root.path(), selected, state);
    assert_eq!(
        request.plan.video_processing,
        Some(StreamProcessing::Transcode)
    );
    let mut processor = processor(&request);
    let properties = processor.evidence.source_video.as_mut().unwrap();
    properties.color_transfer = Some("bt709".into());
    properties.color_primaries = Some("bt709".into());
    processor.evidence.video_processing = Some(StreamProcessing::Copy);
    let process = Process {
        video: Some(video),
        video_frame: Some(
            serde_json::json!({"pix_fmt":"yuv420p","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","side_data_list":[]}),
        ),
        ..Default::default()
    };
    let result = YtDlpDownloadExecutor::new(fixtures::Resolver, process, &processor).download(
        request,
        CancellationToken::new(),
        &Progress,
    );
    assert!(result.is_ok(), "proven local source facts and compatible stream copy must not be mistaken for unauthorized processing");
    let result = result.unwrap();
    assert_eq!(
        result.evidence.video_processing,
        Some(StreamProcessing::Copy)
    );
    assert_eq!(
        result
            .evidence
            .source_video
            .unwrap()
            .color_transfer
            .as_deref(),
        Some("bt709")
    );
}
#[test]
fn bilibili_original_color_and_full_duration_survive_transfer() {
    for process in [
        Process {
            change_color: true,
            ..Default::default()
        },
        Process {
            short: true,
            ..Default::default()
        },
    ] {
        let root = tempfile::tempdir().unwrap();
        let request = request(root.path(), intent(SelectionMode::BestAvailable));
        let staging = request.staging_dir.clone();
        let processor = processor(&request);
        assert!(
            YtDlpDownloadExecutor::new(fixtures::Resolver, &process, &processor)
                .download(request, CancellationToken::new(), &Progress)
                .is_err()
        );
        assert_eq!(*processor.calls.lock().unwrap(), 0);
        assert!(!staging.join("source").exists());
        assert!(!staging.join("stage.mp4").exists());
    }
}
#[test]
fn bilibili_source_or_plan_tampering_is_rejected_before_transfer() {
    for selector in 0..7 {
        let root = tempfile::tempdir().unwrap();
        let mut request = request(root.path(), intent(SelectionMode::BestAvailable));
        let processor = processor(&request);
        match selector {
            0 => request.playlist_item = Some(1),
            1 => request.bilibili_source = None,
            2 => request.plan.source_format_ids = vec!["other".into()],
            3 => {
                request.source =
                    local_media_downloader_lib::application::media_url::validate_media_url(
                        "https://www.bilibili.com/video/BV1xx411c7mD?p=1",
                    )
                    .unwrap()
            }
            4 => request.instagram_selector=Some(local_media_downloader_lib::application::instagram_video::InstagramVideoSelector::Reel),
            5 => {
                use local_media_downloader_lib::{application::{media_url::validate_media_url,network_profile::NetworkProfile,x_video::{match_x_video,resolve_x_video}},infrastructure::x::inventory::parse_x_inventory};
                let source=validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
                let inventory=parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"),&source).unwrap();
                let raw=serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json")).unwrap();
                let resolved=resolve_x_video(match_x_video(&inventory,&inventory.items[1],&raw).unwrap(),Arc::new(NetworkProfile::current()),false).unwrap();
                request.x_source=Some(resolved.into_parts().2);
            }
            _ => request.source=local_media_downloader_lib::application::media_url::validate_media_url("https://www.youtube.com/watch?v=demoYT00001").unwrap(),
        }
        let process = Process::default();
        assert_eq!(
            YtDlpDownloadExecutor::new(fixtures::Resolver, &process, &processor)
                .download(request, CancellationToken::new(), &Progress)
                .err(),
            Some(DownloadErrorCode::InvalidRequest)
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
}
#[test]
fn bilibili_transfer_failure_and_cancellation_do_not_process_or_publish() {
    for process in [
        Process {
            failure: Some(ProcessFailureKind::InactivityTimedOut),
            ..Default::default()
        },
        Process {
            cancel: true,
            ..Default::default()
        },
    ] {
        let root = tempfile::tempdir().unwrap();
        let request = request(root.path(), intent(SelectionMode::BestAvailable));
        let staging = request.staging_dir.clone();
        let processor = processor(&request);
        assert!(
            YtDlpDownloadExecutor::new(fixtures::Resolver, &process, &processor)
                .download(request, CancellationToken::new(), &Progress)
                .is_err()
        );
        assert_eq!(*processor.calls.lock().unwrap(), 0);
        assert!(!staging.join("source").exists());
    }
}
#[test]
fn bilibili_manual_compatibility_and_clip_remain_frozen() {
    let root = tempfile::tempdir().unwrap();
    let mut selection = intent(SelectionMode::Manual);
    selection.video_format_id = Some("100026".into());
    selection.compatibility_mode = true;
    selection.clip = Some(ClipRange {
        start_ms: 1000,
        end_ms: 3000,
    });
    let request = request(root.path(), selection);
    let expected = request.plan.clone();
    let processor = processor(&request);
    let staged = YtDlpDownloadExecutor::new(fixtures::Resolver, Process::default(), &processor)
        .download(request, CancellationToken::new(), &Progress)
        .unwrap();
    assert_eq!(staged.requested_clip, expected.clip);
    assert_eq!(
        staged.evidence.video_processing,
        Some(StreamProcessing::Copy)
    );
}

#[test]
fn bilibili_all_four_modes_preserve_the_selected_plan() {
    for mode in [
        SelectionMode::BestAvailable,
        SelectionMode::Manual,
        SelectionMode::VideoOnly,
        SelectionMode::AudioOnly,
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut selection = intent(mode);
        if mode == SelectionMode::Manual || mode == SelectionMode::VideoOnly {
            selection.video_format_id = Some("100026".into());
        }
        let request = request(root.path(), selection);
        let expected = request.plan.clone();
        let processor = processor(&request);
        let staged = YtDlpDownloadExecutor::new(fixtures::Resolver, Process::default(), &processor)
            .download(request, CancellationToken::new(), &Progress)
            .unwrap();
        assert_eq!(staged.expected_operation, expected.operation);
        assert_eq!(staged.evidence.video_processing, expected.video_processing);
        assert_eq!(staged.evidence.audio_processing, expected.audio_processing);
    }
}

#[test]
fn bilibili_common_container_fallback_keeps_original_codecs() {
    for (audio, container) in [("opus", "webm"), ("aac", "mkv")] {
        let state = Arc::new(fixtures::FixtureState::default());
        state.metadata.lock().unwrap()["formats"][0]["vcodec"] = "vp8".into();
        state.metadata.lock().unwrap()["formats"][0]["ext"] = "webm".into();
        state.metadata.lock().unwrap()["formats"][1]["acodec"] = audio.into();
        state.metadata.lock().unwrap()["formats"][1]["ext"] =
            if audio == "opus" { "webm" } else { "m4a" }.into();
        state.video.lock().unwrap()["streams"][0]["codec_name"] = "vp8".into();
        state.video.lock().unwrap()["streams"][0]["profile"] = Value::Null;
        state.audio.lock().unwrap()["streams"][0]["codec_name"] = audio.into();
        let process = Process {
            video: Some(state.video.lock().unwrap().clone()),
            audio: Some(state.audio.lock().unwrap().clone()),
            ..Default::default()
        };
        let root = tempfile::tempdir().unwrap();
        let request = request_using(root.path(), intent(SelectionMode::BestAvailable), state);
        assert_eq!(request.plan.output_container.as_deref(), Some(container));
        assert_eq!(request.plan.video_processing, Some(StreamProcessing::Copy));
        assert_eq!(request.plan.audio_processing, Some(StreamProcessing::Copy));
        let processor = processor(&request);
        YtDlpDownloadExecutor::new(fixtures::Resolver, process, &processor)
            .download(request, CancellationToken::new(), &Progress)
            .unwrap();
    }
}

#[test]
fn bilibili_disk_full_is_typed_and_preserves_no_output() {
    let root = tempfile::tempdir().unwrap();
    let request = request(root.path(), intent(SelectionMode::BestAvailable));
    let processor = processor(&request);
    let process = Process {
        error_stderr: Some(b"ERROR: [Errno 28] No space left on device"),
        ..Default::default()
    };
    let error = YtDlpDownloadExecutor::new(fixtures::Resolver, process, &processor)
        .download(request, CancellationToken::new(), &Progress)
        .err();
    assert_eq!(error, Some(DownloadErrorCode::DiskFull));
    assert_eq!(*processor.calls.lock().unwrap(), 0);
}

#[test]
fn bilibili_hdr_frame_refinement_is_local_only_and_preserves_source_facts() {
    let state = Arc::new(fixtures::FixtureState::default());
    state.metadata.lock().unwrap()["formats"][0]["vcodec"] = "hevc".into();
    state.metadata.lock().unwrap()["formats"][0]["dynamic_range"] = "HDR10".into();
    for (key, value) in [
        ("codec_name", "hevc"),
        ("profile", "Main 10"),
        ("pix_fmt", "yuv420p10le"),
        ("color_transfer", "smpte2084"),
        ("color_space", "bt2020nc"),
        ("color_primaries", "bt2020"),
    ] {
        state.video.lock().unwrap()["streams"][0][key] = value.into();
    }
    let process = Process {
        video: Some(state.video.lock().unwrap().clone()),
        ..Default::default()
    };
    let root = tempfile::tempdir().unwrap();
    let request = request_using(root.path(), intent(SelectionMode::BestAvailable), state);
    let processor = processor(&request);
    let staged = YtDlpDownloadExecutor::new(fixtures::Resolver, process, &processor)
        .download(request, CancellationToken::new(), &Progress)
        .unwrap();
    assert_eq!(
        staged
            .evidence
            .source_video
            .unwrap()
            .color_transfer
            .as_deref(),
        Some("smpte2084")
    );
}
