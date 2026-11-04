#[path = "support/x_hls.rs"]
mod hls_fixture;
#[path = "support/output_policy.rs"]
mod policy_fixture;
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use local_media_downloader_lib::{
    application::{
        download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
        download_job::JobStaging,
        instagram_video::InstagramVideoSelector,
        local_processing::{LocalMediaProcessor, LocalProcessingRequest, ProcessedMedia},
        media_url::validate_media_url,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessStream, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::{DownloadErrorCode, DownloadProgress, DownloadState},
        format_selection::{
            ClipRange, SelectionMode, SelectionOperation, SelectionPlan, SelectionWarning,
        },
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
    },
    infrastructure::{output::FilesystemJobStaging, ytdlp::download::YtDlpDownloadExecutor},
};

const INPUT: &str = "https://youtube.com/watch?v=demoYT00001";

fn x_request(staging_dir: PathBuf) -> DownloadExecutionRequest {
    use local_media_downloader_lib::{
        application::{
            network_profile::NetworkProfile,
            x_video::{match_x_video, resolve_x_video},
        },
        infrastructure::x::inventory::parse_x_inventory,
    };
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    let inventory =
        parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), &source).unwrap();
    let raw =
        serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json")).unwrap();
    let resolved = resolve_x_video(
        match_x_video(&inventory, &inventory.items[1], &raw).unwrap(),
        Arc::new(NetworkProfile::current()),
        false,
    )
    .unwrap();
    let (media, plan, x_source) = resolved.into_parts();
    let video_source = media
        .formats
        .iter()
        .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
        .cloned();
    let audio_source = media
        .formats
        .iter()
        .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
        .cloned();
    DownloadExecutionRequest {
        job_id: "71".into(),
        source,
        playlist_item: None,
        instagram_selector: None,
        tiktok_source: None,
        x_source: Some(x_source),
        bilibili_source: None,
        plan,
        video_source,
        audio_source,
        staging_dir,
    }
}
#[test]
fn x_download_loads_only_private_verified_info_json_without_page_reextraction() {
    let root = tempfile::tempdir().unwrap();
    let staging = FilesystemJobStaging::new(root.path().to_owned())
        .create("71")
        .unwrap();
    struct InfoProcess {
        seen: Mutex<bool>,
    }
    impl ProcessSupervisor for InfoProcess {
        fn run(
            &self,
            request: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let args = request
                .args
                .iter()
                .map(|s| s.to_str().unwrap())
                .collect::<Vec<_>>();
            let info = args
                .windows(2)
                .find(|a| a[0] == "--load-info-json")
                .expect("must not re-extract page")[1];
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(info).unwrap()).unwrap();
            assert_eq!(value["id"], "10000000000000000002");
            assert_eq!(value["formats"][0]["format_id"], "high");
            assert!(value.get("extractor").is_none());
            assert!(value.get("webpage_url").is_none());
            assert!(!args.iter().any(|a| a.contains("/status/")));
            assert!(!args.contains(&"--playlist-items"));
            assert!(args.windows(2).any(|a| a == ["--downloader", "ffmpeg"]));
            let guards = args
                .windows(2)
                .find(|a| a[0] == "--downloader-args")
                .expect("X transfer must use guarded FFmpeg input")[1];
            for required in [
                "ffmpeg_i:",
                "-max_redirects 0",
                "-protocol_whitelist https,http,tcp,tls,httpproxy",
                "-format_whitelist mov,matroska,webm",
                "-rw_timeout 8000000",
                "-http_proxy ",
            ] {
                assert!(
                    guards.contains(required),
                    "missing transfer guard {required}"
                );
            }
            fs::write(request.working_dir.join("source.mp4"), b"verified input").unwrap();
            *self.seen.lock().unwrap() = true;
            Ok(ProcessExit {
                code: 0,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }
    let process = InfoProcess {
        seen: Mutex::new(false),
    };
    let executor = YtDlpDownloadExecutor::new(FakeResolver::ready(), &process, &DEFAULT_PROCESSOR);
    executor
        .download(
            x_request(staging.clone()),
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    assert!(*process.seen.lock().unwrap());
    assert!(!staging.join("source").exists());
}
#[test]
fn x_download_refuses_selector_or_plan_tampering_before_any_tool_launch() {
    let root = tempfile::tempdir().unwrap();
    let staging = FilesystemJobStaging::new(root.path().to_owned())
        .create("71")
        .unwrap();
    let process = FakeProcess::success();
    let executor = YtDlpDownloadExecutor::new(FakeResolver::ready(), &process, &DEFAULT_PROCESSOR);
    let mut altered = x_request(staging.clone());
    altered.plan.source_format_ids = vec!["low".into()];
    altered.plan.video_format_id = Some("low".into());
    altered.plan.audio_format_id = Some("low".into());
    assert!(matches!(
        executor.download(
            altered,
            CancellationToken::new(),
            &ProgressRecorder::default()
        ),
        Err(DownloadErrorCode::InvalidRequest)
    ));
    let mut altered = x_request(staging);
    altered.source = validate_media_url(INPUT).unwrap();
    assert!(matches!(
        executor.download(
            altered,
            CancellationToken::new(),
            &ProgressRecorder::default()
        ),
        Err(DownloadErrorCode::InvalidRequest)
    ));
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn x_hls_download_uses_sealed_playlist_and_copy_without_yt_dlp_page_fetch() {
    use local_media_downloader_lib::{
        application::{
            network_profile::NetworkProfile, x_source::XRequestContext, x_video::XVideoAnalyzer,
        },
        domain::media::AnalysisErrorCode,
        infrastructure::{
            x::{inventory::parse_x_inventory, manifest::HlsManifestTransport},
            ytdlp::x::YtDlpXVideoAnalyzer,
        },
    };
    struct Manifest;
    impl HlsManifestTransport for Manifest {
        fn read(&self, _: &url::Url, _: &XRequestContext) -> Result<Vec<u8>, AnalysisErrorCode> {
            Ok(b"#EXTM3U\n#EXTINF:1,\npart.ts\n#EXT-X-ENDLIST\n".to_vec())
        }
    }
    struct Metadata;
    impl ProcessSupervisor for Metadata {
        fn run(
            &self,
            _: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let mut raw: serde_json::Value =
                serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json"))
                    .unwrap();
            raw["entries"][1]["formats"].as_array_mut().unwrap().push(serde_json::json!({"format_id":"hls","url":"https://video.twimg.com/fixture/high.m3u8","manifest_url":"https://video.twimg.com/fixture/master.m3u8","protocol":"m3u8_native","ext":"mp4","width":3840,"height":2160,"fps":60,"vcodec":"av1","acodec":"opus","vbr":16000,"abr":160,"audio_channels":2,"asr":48000}));
            Ok(ProcessExit {
                code: 0,
                stdout: serde_json::to_vec(&raw).unwrap(),
                stderr: vec![],
            })
        }
    }
    let root = tempfile::tempdir().unwrap();
    let staging = FilesystemJobStaging::new(root.path().to_owned())
        .create("71")
        .unwrap();
    let mut req = x_request(staging.clone());
    let mut raw: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../fixtures/x/mixed-post.json")).unwrap();
    raw[0][1]["legacy"]["extended_entities"]["media"][1]["video_info"]["variants"].as_array_mut().unwrap().push(serde_json::json!({"content_type":"application/x-mpegURL","url":"https://video.twimg.com/fixture/master.m3u8"}));
    let inventory = parse_x_inventory(&serde_json::to_vec(&raw).unwrap(), &req.source).unwrap();
    let ctx = XRequestContext::new(
        CancellationToken::new(),
        std::time::Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let resolved =
        YtDlpXVideoAnalyzer::with_manifest_transport(FakeResolver::ready(), Metadata, Manifest)
            .analyze_item(
                &inventory,
                inventory.items[1].identity.as_ref().unwrap(),
                false,
                &ctx,
            )
            .unwrap();
    let (media, plan, source) = resolved.into_parts();
    req.video_source = media
        .formats
        .iter()
        .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
        .cloned();
    req.audio_source = media
        .formats
        .iter()
        .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
        .cloned();
    req.plan = plan;
    req.x_source = Some(source);
    let process = FakeProcess::success();
    let executor = YtDlpDownloadExecutor::with_hls_transport(
        FakeResolver::ready(),
        &process,
        &DEFAULT_PROCESSOR,
        hls_fixture::Segments,
    );
    executor
        .download(req, CancellationToken::new(), &ProgressRecorder::default())
        .unwrap();
    let calls = process.calls.lock().unwrap();
    // Video and audio rank independently: this fixture's best audio remains
    // the progressive high stream, while its best video is HLS.
    assert_eq!(calls.len(), 2);
    assert!(calls[0].program.ends_with("ffmpeg.exe"));
    assert!(calls[1].program.ends_with("yt-dlp.exe"));
    assert!(calls[1].args.iter().any(|a| a == "--load-info-json"));
    assert!(!calls[1]
        .args
        .iter()
        .any(|a| a.to_str().unwrap().contains("/status/")));
    let args = calls[0]
        .args
        .iter()
        .map(|s| s.to_str().unwrap())
        .collect::<Vec<_>>();
    assert!(args.windows(2).any(|a| a == ["-c", "copy"]));
    assert!(!args.contains(&"-http_proxy") && !args.contains(&"-max_redirects"));
    assert!(args
        .windows(2)
        .any(|a| a == ["-protocol_whitelist", "file"]));
    let input = args.windows(2).find(|a| a[0] == "-i").unwrap()[1];
    assert!(!input.starts_with("https://"));
    assert!(!args.contains(&"--load-info-json"));
    assert!(!staging.join("source").exists());
}

struct FakeResolver {
    calls: Mutex<Vec<ToolName>>,
    fail_on: Option<ToolName>,
}

impl FakeResolver {
    fn ready() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_on: None,
        }
    }
}

impl ManagedToolResolver for FakeResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        self.calls.lock().unwrap().push(name);
        if self.fail_on == Some(name) {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::ToolIntegrityFailed,
                "private",
            ));
        }
        let file = match name {
            ToolName::YtDlp => "yt-dlp.exe",
            ToolName::Deno => "deno.exe",
            ToolName::GalleryDl => "gallery-dl.exe",
            ToolName::Ffmpeg => "ffmpeg.exe",
            ToolName::Ffprobe => "ffprobe.exe",
        };
        Ok(ResolvedTool {
            name,
            version: "1.2.3".to_owned(),
            path: PathBuf::from(r"C:\managed").join(file),
        })
    }
}

struct Invocation {
    program: PathBuf,
    args: Vec<OsString>,
    working_dir: PathBuf,
    timeout: Option<Duration>,
    inactivity_timeout: Option<Duration>,
    line_limit: usize,
    retained_stderr_limit: usize,
}

struct FakeProcess {
    response: Mutex<VecDeque<Result<ProcessExit, ProcessFailureKind>>>,
    calls: Mutex<Vec<Invocation>>,
    lines: Vec<(ProcessStream, Vec<u8>)>,
    action: ProcessAction,
    events: Option<Arc<Mutex<Vec<&'static str>>>>,
}

#[derive(Clone, Copy)]
enum ProcessAction {
    None,
    MaterializeSource,
    Cancel,
}

impl FakeProcess {
    fn success() -> Self {
        Self {
            response: Mutex::new(VecDeque::from([
                Ok(ProcessExit {
                    code: 0,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                }),
                Ok(ProcessExit {
                    code: 0,
                    stdout: vec![],
                    stderr: vec![],
                }),
            ])),
            calls: Mutex::new(Vec::new()),
            lines: Vec::new(),
            action: ProcessAction::MaterializeSource,
            events: None,
        }
    }

    fn materializing() -> Self {
        Self {
            action: ProcessAction::MaterializeSource,
            ..Self::success()
        }
    }
}

impl ProcessSupervisor for FakeProcess {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let (line_limit, retained_stderr_limit) = match request.output_mode {
            OutputMode::Lines {
                line_limit,
                retained_stderr_limit,
            } => (line_limit, retained_stderr_limit),
            OutputMode::Capture { .. } => panic!("download must use line mode"),
        };
        let working_dir = request.working_dir.clone();
        self.calls.lock().unwrap().push(Invocation {
            program: request.program.path().to_owned(),
            args: request.args,
            working_dir: request.working_dir,
            timeout: request.timeout,
            inactivity_timeout: request.inactivity_timeout,
            line_limit,
            retained_stderr_limit,
        });
        if let Some(events) = &self.events {
            events.lock().unwrap().push("transfer");
        }
        match self.action {
            ProcessAction::None => {}
            ProcessAction::MaterializeSource => {
                fs::write(working_dir.join("source.mp4"), b"complete source").unwrap();
            }
            ProcessAction::Cancel => cancellation.cancel(),
        }
        for (stream, line) in &self.lines {
            sink.on_line(*stream, line);
        }
        self.response.lock().unwrap().pop_front().unwrap()
    }
}

#[derive(Default)]
struct ProgressRecorder(
    Mutex<Vec<DownloadProgress>>,
    Option<Arc<Mutex<Vec<&'static str>>>>,
);

impl ProgressRecorder {
    fn with_events(events: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self(Mutex::new(Vec::new()), Some(events))
    }
}

impl DownloadProgressSink for ProgressRecorder {
    fn on_progress(&self, progress: DownloadProgress) {
        if let Some(events) = &self.1 {
            events.lock().unwrap().push(match progress.state {
                DownloadState::Downloading => "downloading",
                DownloadState::Processing => "processing",
                _ => "unexpected-progress",
            });
        }
        self.0.lock().unwrap().push(progress);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClipCall {
    job_id: String,
    staging_dir: PathBuf,
    source_dir: PathBuf,
    clip: Option<ClipRange>,
    video_only: bool,
}

enum ClipAction {
    Success,
    Fail,
    Cancel,
    SwapSourceToLink(PathBuf),
}

struct FakeClipper {
    action: ClipAction,
    calls: Mutex<Vec<ClipCall>>,
    events: Option<Arc<Mutex<Vec<&'static str>>>>,
}

impl FakeClipper {
    fn success() -> Self {
        Self {
            action: ClipAction::Success,
            calls: Mutex::new(Vec::new()),
            events: None,
        }
    }
}

impl LocalMediaProcessor for FakeClipper {
    fn process(
        &self,
        request: LocalProcessingRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode> {
        self.calls.lock().unwrap().push(ClipCall {
            job_id: request.job_id.clone(),
            staging_dir: request.staging_dir.clone(),
            source_dir: request.source_dir.clone(),
            clip: request.plan.clip,
            video_only: request.plan.audio_format_id.is_none(),
        });
        if let Some(events) = &self.events {
            events.lock().unwrap().push("clip");
        }
        match &self.action {
            ClipAction::Success => {
                fs::write(
                    request.staging_dir.join(format!(
                        "stage.{}",
                        request.plan.output_container.as_deref().unwrap()
                    )),
                    b"clip",
                )
                .unwrap();
                Ok(ProcessedMedia {
                    evidence: policy_fixture::evidence(&request.plan),
                })
            }
            ClipAction::Fail => Err(DownloadErrorCode::ProcessFailed),
            ClipAction::Cancel => {
                cancellation.cancel();
                Err(DownloadErrorCode::ProcessFailed)
            }
            ClipAction::SwapSourceToLink(target) => {
                for source in &request.sources {
                    let slot = request.source_dir.join(format!("input-{}", source.slot));
                    fs::remove_file(slot.join("source.mp4")).unwrap();
                    fs::remove_dir(slot).unwrap();
                }
                fs::remove_dir(&request.source_dir).unwrap();
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir(target, &request.source_dir).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &request.source_dir).unwrap();
                fs::write(request.staging_dir.join("stage.mp4"), b"clip").unwrap();
                Ok(ProcessedMedia {
                    evidence: policy_fixture::evidence(&request.plan),
                })
            }
        }
    }
}

struct DefaultProcessor;

impl LocalMediaProcessor for DefaultProcessor {
    fn process(
        &self,
        _request: LocalProcessingRequest,
        _cancellation: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode> {
        FakeClipper::success().process(_request, _cancellation)
    }
}

static DEFAULT_PROCESSOR: DefaultProcessor = DefaultProcessor;

fn plan(operation: SelectionOperation) -> SelectionPlan {
    let (source_format_ids, video_format_id, audio_format_id, output_container) = match operation {
        SelectionOperation::Keep => (
            vec!["18".to_owned()],
            Some("18".to_owned()),
            Some("18".to_owned()),
            Some("mp4".to_owned()),
        ),
        SelectionOperation::Merge => (
            vec!["137".to_owned(), "140".to_owned()],
            Some("137".to_owned()),
            Some("140".to_owned()),
            Some("mp4".to_owned()),
        ),
        SelectionOperation::ExtractAudio => (
            vec!["22".to_owned()],
            None,
            Some("22".to_owned()),
            Some("m4a".to_owned()),
        ),
        SelectionOperation::ExtractVideo => (
            vec!["22".to_owned()],
            Some("22".to_owned()),
            None,
            Some("mp4".to_owned()),
        ),
    };
    SelectionPlan {
        compatibility_mode: false,
        container_selection:
            local_media_downloader_lib::domain::output_policy::ContainerSelection::Automatic,
        video_processing: video_format_id
            .as_ref()
            .map(|_| local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy),
        output_video_codec: video_format_id.as_ref().map(|_| "h264".into()),
        mode: if operation == SelectionOperation::ExtractVideo {
            SelectionMode::VideoOnly
        } else {
            SelectionMode::BestAvailable
        },
        source_format_ids,
        video_format_id,
        audio_format_id,
        operation,
        output_container,
        output_audio_codec: (operation != SelectionOperation::ExtractVideo)
            .then_some(local_media_downloader_lib::domain::output_policy::TargetAudioCodec::Aac),
        audio_processing: (operation != SelectionOperation::ExtractVideo)
            .then_some(local_media_downloader_lib::domain::output_policy::AudioProcessing::Copy),
        clip: None,
        estimated_size_bytes: None,
        warnings: vec![SelectionWarning::ContainerUnverified],
    }
}

std::thread_local! {
    static FIXTURE_ROOTS: std::cell::RefCell<Vec<tempfile::TempDir>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn request(operation: SelectionOperation) -> DownloadExecutionRequest {
    DownloadExecutionRequest {
        job_id: "42".to_owned(),
        source: validate_media_url(INPUT).unwrap(),
        playlist_item: None,
        instagram_selector: None,
        tiktok_source: None,
        x_source: None,
        bilibili_source: None,
        plan: plan(operation),
        audio_source: plan(operation)
            .audio_format_id
            .as_deref()
            .map(|id| policy_fixture::audio_format(id, "aac")),
        video_source: plan(operation)
            .video_format_id
            .as_deref()
            .map(|id| policy_fixture::video_format(id, "h264")),
        staging_dir: {
            let root = tempfile::tempdir().unwrap();
            let staging = root.path().join("42");
            fs::create_dir(&staging).unwrap();
            FIXTURE_ROOTS.with(|roots| roots.borrow_mut().push(root));
            staging
        },
    }
}

fn clip_request() -> (tempfile::TempDir, DownloadExecutionRequest, ClipRange) {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("42");
    fs::create_dir(&staging).unwrap();
    let clip = ClipRange {
        start_ms: 10_025,
        end_ms: 30_075,
    };
    let mut request = request(SelectionOperation::Merge);
    request.staging_dir = staging;
    request.plan.clip = Some(clip);
    (root, request, clip)
}

fn args(process: &FakeProcess) -> Vec<String> {
    process.calls.lock().unwrap()[0]
        .args
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn combined_video_only_materializes_source_and_invokes_local_extraction() {
    let (_root, mut input, _) = clip_request();
    input.plan = plan(SelectionOperation::ExtractVideo);
    input.video_source = Some(policy_fixture::video_format("22", "h264"));
    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper::success();
    let result = YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
        .download(
            input,
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    assert_eq!(clipper.calls.lock().unwrap().len(), 1);
    assert!(clipper.calls.lock().unwrap()[0].video_only);
    assert_eq!(clipper.calls.lock().unwrap()[0].clip, None);
    assert!(result.staging_dir.join("stage.mp4").is_file());
    assert!(!result.staging_dir.join("source").exists());
    let arguments = args(&process);
    let selector = arguments.iter().position(|arg| arg == "--format").unwrap();
    assert_eq!(arguments[selector + 1], "22");
    assert!(!arguments.iter().any(|arg| arg == "--extract-audio"));
}

#[test]
fn unauthorized_conversion_is_rejected_before_any_transfer() {
    let (_root, mut input, _) = clip_request();
    input.plan = plan(SelectionOperation::Keep);
    input.video_source = Some(policy_fixture::video_format("18", "h264"));
    input.plan.audio_processing =
        Some(local_media_downloader_lib::domain::output_policy::AudioProcessing::Transcode);
    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper::success();
    let result = YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
        .download(
            input,
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .err();
    assert_eq!(result, Some(DownloadErrorCode::InvalidRequest));
    assert!(process.calls.lock().unwrap().is_empty());
    assert!(clipper.calls.lock().unwrap().is_empty());
}

#[test]
fn clipped_video_only_processing_keeps_the_selected_source_and_range() {
    for operation in [SelectionOperation::Keep, SelectionOperation::ExtractVideo] {
        let (_root, mut input, range) = clip_request();
        input.plan = plan(operation);
        input.plan.mode = SelectionMode::VideoOnly;
        input.plan.audio_format_id = None;
        input.plan.output_audio_codec = None;
        input.plan.audio_processing = None;
        input.audio_source = None;
        input.video_source = input
            .plan
            .video_format_id
            .as_deref()
            .map(|id| policy_fixture::video_format(id, "h264"));
        input.plan.clip = Some(range);
        let resolver = FakeResolver::ready();
        let process = FakeProcess::materializing();
        let clipper = FakeClipper::success();
        YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
            .download(
                input,
                CancellationToken::new(),
                &ProgressRecorder::default(),
            )
            .unwrap();
        let calls = clipper.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].video_only);
        assert_eq!(calls[0].clip, Some(range));
    }
}

#[test]
fn keep_uses_verified_tools_fixed_safety_arguments_and_neutral_output() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess::success();
    let progress = ProgressRecorder::default();
    let staged = YtDlpDownloadExecutor::new(&resolver, &process, &DEFAULT_PROCESSOR)
        .download(
            request(SelectionOperation::Keep),
            CancellationToken::new(),
            &progress,
        )
        .unwrap();
    assert_eq!(
        *resolver.calls.lock().unwrap(),
        [
            ToolName::YtDlp,
            ToolName::Deno,
            ToolName::Ffmpeg,
            ToolName::Ffprobe
        ]
    );
    assert_eq!(staged.job_id, "42");
    assert_eq!(staged.staging_dir.file_name().unwrap(), "42");
    assert_eq!(staged.expected_operation, SelectionOperation::Keep);
    assert_eq!(staged.expected_container.as_deref(), Some("mp4"));
    assert_eq!(staged.requested_clip, None);

    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, PathBuf::from(r"C:\managed\yt-dlp.exe"));
    assert_eq!(
        calls[0].working_dir,
        staged.staging_dir.join("source/input-0")
    );
    assert_eq!(calls[0].timeout, None);
    assert_eq!(calls[0].inactivity_timeout, Some(Duration::from_secs(120)));
    assert_eq!(calls[0].line_limit, 4096);
    assert_eq!(calls[0].retained_stderr_limit, 65_536);
    drop(calls);

    assert_eq!(args(&process), vec![
        "--ignore-config", "--no-remote-components", "--no-plugin-dirs", "--js-runtimes", r"deno:C:\managed\deno.exe",
        "--ffmpeg-location", r"C:\managed\ffmpeg.exe", "--no-playlist", "--no-warnings",
        "--no-continue", "--no-overwrites", "--newline", "--progress", "--progress-delta", "0.5",
        "--progress-template", "download:LMD_PROGRESS\tjob=42\tdownloaded=%(progress.downloaded_bytes)s\ttotal=%(progress.total_bytes)s\testimate=%(progress.total_bytes_estimate)s\tspeed=%(progress.speed)s\teta=%(progress.eta)s\tstatus=%(progress.status)s",
        "--output", "source.%(ext)s", "--format", "18", "--", INPUT,
    ]);
}

#[test]
fn merge_and_extract_audio_use_exact_format_operations() {
    let merge_resolver = FakeResolver::ready();
    let merge_process = FakeProcess::success();
    YtDlpDownloadExecutor::new(&merge_resolver, &merge_process, &DEFAULT_PROCESSOR)
        .download(
            request(SelectionOperation::Merge),
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    let merge = args(&merge_process);
    assert_eq!(&merge[merge.len() - 4..], ["--format", "137", "--", INPUT]);

    let audio_resolver = FakeResolver::ready();
    let audio_process = FakeProcess::success();
    YtDlpDownloadExecutor::new(&audio_resolver, &audio_process, &DEFAULT_PROCESSOR)
        .download(
            request(SelectionOperation::ExtractAudio),
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    let audio = args(&audio_process);
    assert_eq!(&audio[audio.len() - 4..], ["--format", "22", "--", INPUT]);
}

#[test]
fn instagram_reel_and_post_download_only_the_native_selected_item_and_formats() {
    for (url, playlist_item, selector) in [
        (
            "https://www.instagram.com/reel/Reel_01/",
            None,
            InstagramVideoSelector::reel(),
        ),
        (
            "https://www.instagram.com/p/Post_01/",
            Some(3),
            InstagramVideoSelector::post_item(3, "1").unwrap(),
        ),
    ] {
        let mut input = request(SelectionOperation::Keep);
        input.source = validate_media_url(url).unwrap();
        input.playlist_item = playlist_item;
        input.instagram_selector = Some(selector);
        let process = FakeProcess::success();
        YtDlpDownloadExecutor::new(&FakeResolver::ready(), &process, &DEFAULT_PROCESSOR)
            .download(
                input,
                CancellationToken::new(),
                &ProgressRecorder::default(),
            )
            .unwrap();
        assert_eq!(process.calls.lock().unwrap().len(), 1);
        let arguments = args(&process);
        assert_eq!(arguments.last().unwrap(), url);
        let format = arguments.iter().position(|arg| arg == "--format").unwrap();
        assert_eq!(arguments[format + 1], "18");
        let profile = arguments
            .iter()
            .position(|arg| arg == "--impersonate")
            .unwrap();
        assert_eq!(arguments[profile + 1], "chrome-110");
        assert!(!arguments.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "best" | "bestvideo" | "bestaudio" | "--recode-video" | "--merge-output-format"
            )
        }));
        if let Some(expected) = playlist_item {
            assert!(arguments.iter().any(|arg| arg == "--yes-playlist"));
            let at = arguments
                .iter()
                .position(|arg| arg == "--playlist-items")
                .unwrap();
            assert_eq!(arguments[at + 1], expected.to_string());
            assert!(!arguments.iter().any(|arg| arg == "--no-playlist"));
        } else {
            assert!(arguments.iter().any(|arg| arg == "--no-playlist"));
            assert!(!arguments.iter().any(|arg| arg == "--yes-playlist"));
        }
    }
}

#[test]
fn proven_silent_instagram_video_downloads_exact_native_stream_without_audio_invention() {
    let mut input = request(SelectionOperation::Keep);
    input.source = validate_media_url("https://www.instagram.com/p/Post_01/").unwrap();
    input.playlist_item = Some(3);
    input.instagram_selector = Some(InstagramVideoSelector::post_item(3, "1").unwrap());
    input.plan.mode = SelectionMode::VideoOnly;
    input.plan.audio_format_id = None;
    input.plan.output_audio_codec = None;
    input.plan.audio_processing = None;
    input.audio_source = None;
    let video = input.video_source.as_mut().unwrap();
    video.kind = local_media_downloader_lib::domain::media::StreamKind::VideoOnly;
    video.audio_codec = None;
    video.audio_channels = None;
    video.audio_sample_rate_hz = None;
    let process = FakeProcess::success();
    YtDlpDownloadExecutor::new(&FakeResolver::ready(), &process, &DEFAULT_PROCESSOR)
        .download(
            input,
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    let arguments = args(&process);
    let selected = arguments.iter().position(|arg| arg == "--format").unwrap();
    assert_eq!(arguments[selected + 1], "18");
    assert!(!arguments.iter().any(|arg| arg.contains("bestaudio")));
}

#[test]
fn changed_instagram_post_video_fails_without_neighbor_retry() {
    let mut input = request(SelectionOperation::Keep);
    input.source = validate_media_url("https://www.instagram.com/p/Post_01/").unwrap();
    input.playlist_item = Some(3);
    input.instagram_selector = Some(InstagramVideoSelector::post_item(3, "1").unwrap());
    let process = FakeProcess {
        response: Mutex::new(VecDeque::from([Ok(ProcessExit {
            code: 1,
            stdout: vec![],
            stderr: b"There is no video in this post".to_vec(),
        })])),
        calls: Mutex::new(vec![]),
        lines: vec![],
        action: ProcessAction::None,
        events: None,
    };
    assert_eq!(
        YtDlpDownloadExecutor::new(&FakeResolver::ready(), &process, &DEFAULT_PROCESSOR)
            .download(
                input,
                CancellationToken::new(),
                &ProgressRecorder::default(),
            )
            .err(),
        Some(DownloadErrorCode::ContentChanged)
    );
    assert_eq!(process.calls.lock().unwrap().len(), 1);
}

#[test]
fn instagram_download_rejects_mismatched_or_frontend_shaped_selectors_before_transfer() {
    let mut input = request(SelectionOperation::Keep);
    input.source = validate_media_url("https://www.instagram.com/p/Post_01/").unwrap();
    input.playlist_item = Some(2);
    input.instagram_selector = Some(InstagramVideoSelector::post_item(3, "1").unwrap());
    let process = FakeProcess::success();
    let result = YtDlpDownloadExecutor::new(&FakeResolver::ready(), &process, &DEFAULT_PROCESSOR)
        .download(
            input,
            CancellationToken::new(),
            &ProgressRecorder::default(),
        );
    assert_eq!(result.map(|_| ()), Err(DownloadErrorCode::InvalidRequest));
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn downloads_each_combined_source_once_and_maps_selected_roles() {
    let mut input = request(SelectionOperation::Merge);
    input.plan.source_format_ids = vec!["video-combined".into(), "audio-combined".into()];
    input.plan.video_format_id = Some("video-combined".into());
    input.plan.audio_format_id = Some("audio-combined".into());
    input.video_source = Some(policy_fixture::video_format("video-combined", "h264"));
    input.audio_source = Some(policy_fixture::audio_format("audio-combined", "aac"));
    let process = FakeProcess::materializing();
    process.response.lock().unwrap().push_back(Ok(ProcessExit {
        code: 0,
        stdout: vec![],
        stderr: vec![],
    }));
    let clipper = FakeClipper::success();
    YtDlpDownloadExecutor::new(&FakeResolver::ready(), &process, &clipper)
        .download(
            input,
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    let calls = process.calls.lock().unwrap();
    let selectors: Vec<_> = calls
        .iter()
        .map(|call| {
            let at = call.args.iter().position(|a| a == "--format").unwrap();
            call.args[at + 1].to_string_lossy().into_owned()
        })
        .collect();
    assert_eq!(selectors, ["video-combined", "audio-combined"]);
    for call in calls.iter() {
        assert!(!call
            .args
            .iter()
            .any(|arg| arg == "--merge-output-format" || arg == "--recode-video"));
    }
    assert_eq!(clipper.calls.lock().unwrap().len(), 1);
}

#[test]
fn second_source_failure_never_processes_or_retries() {
    let input = request(SelectionOperation::Merge);
    let staging = input.staging_dir.clone();
    let process = FakeProcess::materializing();
    *process.response.lock().unwrap() = VecDeque::from([
        Ok(ProcessExit {
            code: 0,
            stdout: vec![],
            stderr: vec![],
        }),
        Err(ProcessFailureKind::TimedOut),
    ]);
    let clipper = FakeClipper::success();
    let result = YtDlpDownloadExecutor::new(&FakeResolver::ready(), &process, &clipper).download(
        input,
        CancellationToken::new(),
        &ProgressRecorder::default(),
    );
    assert_eq!(result.map(|_| ()), Err(DownloadErrorCode::DownloadTimeout));
    assert_eq!(process.calls.lock().unwrap().len(), 2);
    assert!(clipper.calls.lock().unwrap().is_empty());
    assert!(!staging.join("source").exists());
    assert!(!staging.join("stage.mp4").exists());
}

#[test]
fn clip_materializes_complete_media_before_local_cutting() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper::success();
    let (_root, input, clip) = clip_request();
    let staging = input.staging_dir.clone();
    let canonical_staging = fs::canonicalize(&staging).unwrap();
    let staged = YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
        .download(
            input,
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    assert_eq!(staged.staging_dir, canonical_staging);
    assert_eq!(staged.requested_clip, Some(clip));

    let invocation = args(&process);
    assert!(invocation
        .windows(2)
        .any(|part| part == ["--output", "source.%(ext)s"]));
    assert!(invocation
        .windows(2)
        .any(|part| part == ["--format", "137"]));
    assert!(!invocation
        .iter()
        .any(|argument| argument == "--download-sections"));
    assert!(!invocation
        .iter()
        .any(|argument| argument.contains("force-keyframes") || argument.contains("recode")));
    assert_eq!(
        process.calls.lock().unwrap()[0].working_dir,
        canonical_staging.join("source").join("input-0")
    );
    assert_eq!(
        *clipper.calls.lock().unwrap(),
        [ClipCall {
            job_id: "42".to_owned(),
            staging_dir: canonical_staging.clone(),
            source_dir: canonical_staging.join("source"),
            clip: Some(clip),
            video_only: false,
        }]
    );
}

#[test]
fn clip_source_workspace_is_one_ordinary_direct_child_and_is_removed_on_success() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper::success();
    let (_root, input, _) = clip_request();
    let staging = input.staging_dir.clone();

    YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
        .download(
            input,
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();

    assert!(!staging.join("source").exists());
    let entries: Vec<_> = fs::read_dir(&staging)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, ["stage.mp4"]);
}

#[test]
fn clip_source_is_removed_on_transfer_clip_and_cleanup_failures() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess {
        response: Mutex::new(VecDeque::from([Err(ProcessFailureKind::LaunchFailed)])),
        calls: Mutex::new(Vec::new()),
        lines: Vec::new(),
        action: ProcessAction::MaterializeSource,
        events: None,
    };
    let (_root, input, _) = clip_request();
    let staging = input.staging_dir.clone();
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &FakeClipper::success())
            .download(
                input,
                CancellationToken::new(),
                &ProgressRecorder::default()
            )
            .err(),
        Some(DownloadErrorCode::ProcessFailed)
    );
    assert!(!staging.join("source").exists());

    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper {
        action: ClipAction::Fail,
        calls: Mutex::new(Vec::new()),
        events: None,
    };
    let (_root, input, _) = clip_request();
    let staging = input.staging_dir.clone();
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
            .download(
                input,
                CancellationToken::new(),
                &ProgressRecorder::default()
            )
            .err(),
        Some(DownloadErrorCode::ProcessFailed)
    );
    assert!(!staging.join("source").exists());

    let root = tempfile::tempdir().unwrap();
    let external = root.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("sentinel.txt"), b"keep").unwrap();
    let probe = root.path().join("link-probe");
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&external, &probe).is_err() {
        return;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &probe).unwrap();
    fs::remove_dir(&probe).unwrap();

    let staging_root = root.path().join("jobs");
    fs::create_dir(&staging_root).unwrap();
    let staging = staging_root.join("42");
    fs::create_dir(&staging).unwrap();
    let mut input = request(SelectionOperation::Merge);
    input.staging_dir = staging.clone();
    input.plan.clip = Some(ClipRange {
        start_ms: 10_025,
        end_ms: 30_075,
    });
    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper {
        action: ClipAction::SwapSourceToLink(external.clone()),
        calls: Mutex::new(Vec::new()),
        events: None,
    };
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
            .download(
                input,
                CancellationToken::new(),
                &ProgressRecorder::default()
            )
            .err(),
        Some(DownloadErrorCode::OutputInvalid)
    );
    FilesystemJobStaging::new(staging_root)
        .cleanup("42")
        .unwrap();
    assert!(!staging.exists());
    assert_eq!(fs::read(external.join("sentinel.txt")).unwrap(), b"keep");
}

#[test]
fn clip_moves_from_transfer_progress_to_processing_before_ffmpeg() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let resolver = FakeResolver::ready();
    let mut process = FakeProcess::materializing();
    process.events = Some(Arc::clone(&events));
    process.lines = vec![(
        ProcessStream::Stdout,
        b"LMD_PROGRESS\tjob=42\tdownloaded=100\ttotal=100\testimate=NA\tspeed=5\teta=0\tstatus=finished".to_vec(),
    )];
    let clipper = FakeClipper {
        action: ClipAction::Success,
        calls: Mutex::new(Vec::new()),
        events: Some(Arc::clone(&events)),
    };
    let recorder = ProgressRecorder::with_events(Arc::clone(&events));
    let (_root, input, _) = clip_request();

    YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
        .download(input, CancellationToken::new(), &recorder)
        .unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        [
            "transfer",
            "downloading",
            "transfer",
            "downloading",
            "processing",
            "clip"
        ]
    );
    let progress = recorder.0.lock().unwrap();
    assert_eq!(
        progress.iter().map(|item| item.state).collect::<Vec<_>>(),
        [
            DownloadState::Downloading,
            DownloadState::Downloading,
            DownloadState::Processing
        ]
    );
    assert_eq!(progress[0].downloaded_bytes, Some(100));
    assert_eq!(progress[1].downloaded_bytes, Some(200));
    assert_eq!(progress[0].percent, None);
    assert_eq!(progress[1].total_bytes, None);
    let processing = &progress[2];
    assert_eq!(processing.downloaded_bytes, None);
    assert_eq!(processing.total_bytes, None);
    assert_eq!(processing.percent, None);
    assert_eq!(processing.speed_bytes_per_second, None);
    assert_eq!(processing.eta_seconds, None);
}

#[test]
fn clip_cancellation_before_or_during_cut_returns_process_failed_without_late_progress() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper::success();
    let (_root, input, _) = clip_request();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
            .download(input, cancellation, &ProgressRecorder::default())
            .err(),
        Some(DownloadErrorCode::ProcessFailed)
    );
    assert!(process.calls.lock().unwrap().is_empty());
    assert!(clipper.calls.lock().unwrap().is_empty());

    let resolver = FakeResolver::ready();
    let mut process = FakeProcess::success();
    process.action = ProcessAction::Cancel;
    process.lines = vec![(
        ProcessStream::Stdout,
        b"LMD_PROGRESS\tjob=42\tdownloaded=100\ttotal=100\testimate=NA\tspeed=5\teta=0\tstatus=finished".to_vec(),
    )];
    let clipper = FakeClipper::success();
    let recorder = ProgressRecorder::default();
    let (_root, input, _) = clip_request();
    let cancellation = CancellationToken::new();
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
            .download(input, cancellation.clone(), &recorder)
            .err(),
        Some(DownloadErrorCode::ProcessFailed)
    );
    assert!(cancellation.is_canceled());
    assert!(clipper.calls.lock().unwrap().is_empty());
    assert!(recorder.0.lock().unwrap().is_empty());

    let resolver = FakeResolver::ready();
    let process = FakeProcess::materializing();
    let clipper = FakeClipper {
        action: ClipAction::Cancel,
        calls: Mutex::new(Vec::new()),
        events: None,
    };
    let (_root, input, _) = clip_request();
    let staging = input.staging_dir.clone();
    let cancellation = CancellationToken::new();
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
            .download(input, cancellation.clone(), &ProgressRecorder::default(),)
            .err(),
        Some(DownloadErrorCode::ProcessFailed)
    );
    assert!(cancellation.is_canceled());
    assert_eq!(clipper.calls.lock().unwrap().len(), 1);
    assert!(!staging.join("source").exists());
}

#[test]
fn non_clip_download_normalizes_then_cleans_source_workspace() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess::success();
    let clipper = FakeClipper::success();
    let recorder = ProgressRecorder::default();

    let staged = YtDlpDownloadExecutor::new(&resolver, &process, &clipper)
        .download(
            request(SelectionOperation::Keep),
            CancellationToken::new(),
            &recorder,
        )
        .unwrap();

    assert_eq!(staged.requested_clip, None);
    assert!(args(&process)
        .windows(2)
        .any(|part| part == ["--output", "source.%(ext)s"]));
    assert_eq!(clipper.calls.lock().unwrap().len(), 1);
    assert!(!staged.staging_dir.join("source").exists());
}

#[test]
fn forwards_only_structured_progress_and_rejects_malformed_progress() {
    let resolver = FakeResolver::ready();
    let mut process = FakeProcess::success();
    process.lines = vec![
        (ProcessStream::Stderr, b"private signed URL https://secret.invalid".to_vec()),
        (ProcessStream::Stdout, b"LMD_PROGRESS\tjob=42\tdownloaded=25\ttotal=100\testimate=NA\tspeed=5\teta=15\tstatus=downloading".to_vec()),
    ];
    let recorder = ProgressRecorder::default();
    YtDlpDownloadExecutor::new(&resolver, &process, &DEFAULT_PROCESSOR)
        .download(
            request(SelectionOperation::Keep),
            CancellationToken::new(),
            &recorder,
        )
        .unwrap();
    let progress = recorder.0.lock().unwrap();
    assert_eq!(progress.len(), 2);
    assert_eq!(progress[0].percent, Some(25.0));
    drop(progress);

    let resolver = FakeResolver::ready();
    let mut process = FakeProcess::success();
    process.lines = vec![(ProcessStream::Stdout, b"LMD_PROGRESS\tprivate".to_vec())];
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &DEFAULT_PROCESSOR)
            .download(
                request(SelectionOperation::Keep),
                CancellationToken::new(),
                &ProgressRecorder::default()
            )
            .err(),
        Some(DownloadErrorCode::OutputInvalid)
    );
}

#[test]
fn maps_resolution_process_and_nonzero_failures_to_stable_codes() {
    let resolver = FakeResolver {
        calls: Mutex::new(Vec::new()),
        fail_on: Some(ToolName::Ffprobe),
    };
    let process = FakeProcess::success();
    assert_eq!(
        YtDlpDownloadExecutor::new(&resolver, &process, &DEFAULT_PROCESSOR)
            .download(
                request(SelectionOperation::Keep),
                CancellationToken::new(),
                &ProgressRecorder::default()
            )
            .err(),
        Some(DownloadErrorCode::ToolIntegrityFailed)
    );
    assert!(process.calls.lock().unwrap().is_empty());

    for (response, expected) in [
        (
            Err(ProcessFailureKind::InactivityTimedOut),
            DownloadErrorCode::DownloadTimeout,
        ),
        (
            Err(ProcessFailureKind::LineLimitExceeded),
            DownloadErrorCode::OutputInvalid,
        ),
        (
            Err(ProcessFailureKind::LaunchFailed),
            DownloadErrorCode::ProcessFailed,
        ),
        (
            Ok(ProcessExit {
                code: 1,
                stdout: Vec::new(),
                stderr: b"HTTP Error 429".to_vec(),
            }),
            DownloadErrorCode::RateLimited,
        ),
    ] {
        let resolver = FakeResolver::ready();
        let process = FakeProcess {
            response: Mutex::new(VecDeque::from([response])),
            calls: Mutex::new(Vec::new()),
            lines: Vec::new(),
            action: ProcessAction::None,
            events: None,
        };
        assert_eq!(
            YtDlpDownloadExecutor::new(&resolver, &process, &DEFAULT_PROCESSOR)
                .download(
                    request(SelectionOperation::Keep),
                    CancellationToken::new(),
                    &ProgressRecorder::default()
                )
                .err(),
            Some(expected)
        );
    }
}

#[test]
fn arguments_never_enable_credentials_proxy_plugins_or_extra_artifacts() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess::success();
    YtDlpDownloadExecutor::new(&resolver, &process, &DEFAULT_PROCESSOR)
        .download(
            request(SelectionOperation::Keep),
            CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .unwrap();
    let arguments = args(&process);
    let joined = arguments.join(" ").to_ascii_lowercase();
    assert!(joined.contains("--no-plugin-dirs"));
    assert!(!arguments.iter().any(|argument| argument == "--plugin-dirs"));
    for forbidden in [
        "cookie",
        "password",
        "username",
        "netrc",
        "proxy",
        "write-info",
        "thumbnail",
        "embed",
        "subtitles",
        "exec",
        "force-keyframes",
        "recode",
    ] {
        assert!(
            !joined.contains(forbidden),
            "found forbidden argument fragment {forbidden}"
        );
    }
}
