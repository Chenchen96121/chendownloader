#[path = "support/output_policy.rs"]
mod policy_fixture;
use std::{ffi::OsString, fs, path::PathBuf, sync::Mutex, time::Duration};

use local_media_downloader_lib::{
    application::{
        local_processing::{LocalMediaProcessor, LocalProcessingRequest},
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::ClipRange,
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
    },
    infrastructure::ffmpeg::FfmpegMediaProcessor,
};

struct FakeResolver {
    calls: Mutex<Vec<ToolName>>,
    error: Option<ManagedToolErrorCode>,
}

impl FakeResolver {
    fn ready() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: None,
        }
    }

    fn failing(error: ManagedToolErrorCode) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            error: Some(error),
        }
    }
}

impl ManagedToolResolver for FakeResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        self.calls.lock().unwrap().push(name);
        if let Some(code) = self.error {
            return Err(ManagedToolError::new(code, "private tool detail"));
        }
        Ok(ResolvedTool {
            name,
            version: "1.2.3".to_owned(),
            path: PathBuf::from(r"C:\managed\ffmpeg.exe"),
        })
    }
}

#[derive(Debug)]
struct Invocation {
    program: PathBuf,
    args: Vec<OsString>,
    working_dir: PathBuf,
    timeout: Option<Duration>,
    inactivity_timeout: Option<Duration>,
    stdout_limit: usize,
    stderr_limit: usize,
}

enum OutputAction {
    None,
    Expected,
    ExpectedAndUnexpected,
    Cancel,
    AttemptSourceSwap {
        input_index: usize,
        replacement_file: PathBuf,
        replacement_dir: PathBuf,
    },
}

struct FakeProcess {
    result: Mutex<Option<Result<ProcessExit, ProcessFailureKind>>>,
    action: OutputAction,
    calls: Mutex<Vec<Invocation>>,
    swap_attempts: Mutex<Vec<(bool, bool)>>,
    video_codec: &'static str,
    audio_codec: &'static str,
    channels: u32,
    sample_rate: &'static str,
}

impl FakeProcess {
    fn success(action: OutputAction) -> Self {
        Self {
            result: Mutex::new(Some(Ok(ProcessExit {
                code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }))),
            action,
            calls: Mutex::new(Vec::new()),
            swap_attempts: Mutex::new(Vec::new()),
            video_codec: "h264",
            audio_codec: "aac",
            channels: 2,
            sample_rate: "48000",
        }
    }

    fn exit(code: i32, stderr: &[u8]) -> Self {
        Self {
            result: Mutex::new(Some(Ok(ProcessExit {
                code,
                stdout: Vec::new(),
                stderr: stderr.to_vec(),
            }))),
            action: OutputAction::None,
            calls: Mutex::new(Vec::new()),
            swap_attempts: Mutex::new(Vec::new()),
            video_codec: "h264",
            audio_codec: "aac",
            channels: 2,
            sample_rate: "48000",
        }
    }

    fn failure(error: ProcessFailureKind) -> Self {
        Self {
            result: Mutex::new(Some(Err(error))),
            action: OutputAction::None,
            calls: Mutex::new(Vec::new()),
            swap_attempts: Mutex::new(Vec::new()),
            video_codec: "h264",
            audio_codec: "aac",
            channels: 2,
            sample_rate: "48000",
        }
    }
}

impl ProcessSupervisor for FakeProcess {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        _sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        if request.args.iter().any(|a| a == "-show_frames") {
            return Ok(ProcessExit {code:0,stderr:vec![],stdout:serde_json::to_vec(&serde_json::json!({"frames":[{"pix_fmt":"yuv420p10le","color_range":"tv","color_space":"bt2020nc","color_primaries":"bt2020","color_transfer":"smpte2084","side_data_list":[{"side_data_type":"Mastering display metadata","max_luminance":"1000/1"}]}]})).unwrap()});
        }
        if request.args.iter().any(|arg| arg == "-show_entries") {
            return Ok(ProcessExit { code: 0, stderr: vec![], stdout: serde_json::to_vec(&serde_json::json!({
                "programs":[], "stream_groups":[], "format": {"format_name":"mov,mp4,m4a","size":"6","duration":"10"},
                "streams":[
                    {"codec_type":"video","codec_name":self.video_codec,"width":1280,"height":720,"avg_frame_rate":"30/1","pix_fmt":if self.video_codec == "hevc" {"yuv420p10le"}else{"yuv420p"}},
                    {"codec_type":"audio","codec_name":self.audio_codec,"channels":self.channels,"sample_rate":self.sample_rate}
                ]
            })).unwrap() });
        }
        let (stdout_limit, stderr_limit) = match request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => (stdout_limit, stderr_limit),
            OutputMode::Lines { .. } => panic!("ffmpeg clipping must use bounded capture mode"),
        };
        let output = request.args.last().map(PathBuf::from);
        let source = request
            .args
            .windows(2)
            .filter(|pair| pair[0] == "-i")
            .nth(match &self.action {
                OutputAction::AttemptSourceSwap { input_index, .. } => *input_index,
                _ => 0,
            })
            .map(|pair| PathBuf::from(&pair[1]));
        self.calls.lock().unwrap().push(Invocation {
            program: request.program.path().to_owned(),
            args: request.args,
            working_dir: request.working_dir,
            timeout: request.timeout,
            inactivity_timeout: request.inactivity_timeout,
            stdout_limit,
            stderr_limit,
        });
        match &self.action {
            OutputAction::None => {}
            OutputAction::Expected => fs::write(output.unwrap(), b"clip").unwrap(),
            OutputAction::ExpectedAndUnexpected => {
                let output = output.unwrap();
                fs::write(&output, b"clip").unwrap();
                fs::write(output.parent().unwrap().join("unexpected.tmp"), b"partial").unwrap();
            }
            OutputAction::Cancel => cancellation.cancel(),
            OutputAction::AttemptSourceSwap {
                replacement_file,
                replacement_dir,
                ..
            } => {
                let source = source.unwrap();
                let source_dir = source.parent().unwrap();
                let file_backup = source.with_extension("validated");
                let file_swapped = if fs::rename(&source, &file_backup).is_ok() {
                    let swapped = fs::rename(replacement_file, &source).is_ok();
                    if swapped {
                        fs::rename(&source, replacement_file).unwrap();
                    }
                    fs::rename(&file_backup, &source).unwrap();
                    swapped
                } else {
                    false
                };

                let directory_backup = source_dir.with_file_name("source-validated");
                let directory_swapped = if fs::rename(source_dir, &directory_backup).is_ok() {
                    let swapped = fs::rename(replacement_dir, source_dir).is_ok();
                    if swapped {
                        fs::rename(source_dir, replacement_dir).unwrap();
                    }
                    fs::rename(&directory_backup, source_dir).unwrap();
                    swapped
                } else {
                    false
                };
                self.swap_attempts
                    .lock()
                    .unwrap()
                    .push((file_swapped, directory_swapped));
                fs::write(output.unwrap(), b"clip").unwrap();
            }
        }
        self.result.lock().unwrap().take().unwrap()
    }
}

fn fixture(extension: &str) -> (tempfile::TempDir, LocalProcessingRequest) {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("42");
    let source = staging.join("source");
    fs::create_dir_all(source.join("input-0")).unwrap();
    fs::write(
        source.join("input-0").join(format!("source.{extension}")),
        b"source",
    )
    .unwrap();
    (
        root,
        LocalProcessingRequest {
            job_id: "42".to_owned(),
            staging_dir: staging,
            source_dir: source,
            plan: local_media_downloader_lib::domain::format_selection::SelectionPlan {
                compatibility_mode: false,
                container_selection:
                    local_media_downloader_lib::domain::output_policy::ContainerSelection::Automatic,
                video_processing: Some(
                    local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy,
                ),
                output_video_codec: Some("h264".into()),
                mode: local_media_downloader_lib::domain::format_selection::SelectionMode::Manual,
                source_format_ids: vec!["18".into()],
                video_format_id: Some("18".into()),
                audio_format_id: Some("18".into()),
                operation:
                    local_media_downloader_lib::domain::format_selection::SelectionOperation::Keep,
                output_container: Some("mp4".into()),
                output_audio_codec: Some(
                    local_media_downloader_lib::domain::output_policy::TargetAudioCodec::Aac,
                ),
                audio_processing: Some(
                    local_media_downloader_lib::domain::output_policy::AudioProcessing::Copy,
                ),
                estimated_size_bytes: None,
                warnings: vec![],
                clip: Some(ClipRange {
                    start_ms: 1_234,
                    end_ms: 6_912,
                }),
            },
            video_source: Some(policy_fixture::video_format("18", "h264")),
            audio_source: Some(policy_fixture::audio_format("18", "aac")),
            sources: policy_fixture::slots(&["18".into()]),
        },
    )
}

fn run(
    request: LocalProcessingRequest,
    resolver: &FakeResolver,
    process: &FakeProcess,
) -> Result<(), DownloadErrorCode> {
    FfmpegMediaProcessor::new(resolver, process)
        .process(request, CancellationToken::new())
        .map(|_| ())
}

#[test]
fn video_only_processing_never_maps_audio_for_full_or_clipped_outputs() {
    for full in [false, true] {
        let (_root, mut request) = fixture("mp4");
        request.plan.mode =
            local_media_downloader_lib::domain::format_selection::SelectionMode::VideoOnly;
        request.plan.audio_format_id = None;
        request.audio_source = None;
        request.plan.output_audio_codec = None;
        request.plan.audio_processing = None;
        if full {
            request.plan.clip = None;
        }
        let resolver = FakeResolver::ready();
        let process = FakeProcess::success(OutputAction::Expected);
        run(request, &resolver, &process).unwrap();
        let calls = process.calls.lock().unwrap();
        let arguments: Vec<_> = calls[0]
            .args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(arguments.iter().any(|arg| arg == "-an"));
        assert!(!arguments.iter().any(|arg| arg.starts_with("0:a")));
        assert_eq!(arguments.iter().any(|arg| arg == "-ss"), !full);
        assert_eq!(arguments.iter().any(|arg| arg == "-t"), !full);
        assert!(arguments.windows(2).any(|pair| pair == ["-c:v", "copy"]));
    }
}

#[test]
fn local_processing_uses_per_stream_video_copy_and_fixed_audio() {
    let (_root, request) = fixture("mp4");
    let resolver = FakeResolver::ready();
    let process = FakeProcess::success(OutputAction::Expected);
    run(request, &resolver, &process).unwrap();
    let calls = process.calls.lock().unwrap();
    assert!(calls[0]
        .args
        .windows(2)
        .any(|pair| pair == ["-c:v", "copy"]));
    assert!(calls[0]
        .args
        .windows(2)
        .any(|pair| pair == ["-c:a", "copy"]));
}

#[test]
fn rejects_missing_multiple_malformed_and_outside_source_entries() {
    let mut cases = Vec::new();

    let (missing_root, missing) = fixture("mp4");
    fs::remove_file(missing.source_dir.join("input-0").join("source.mp4")).unwrap();
    cases.push((missing_root, missing));

    let (multiple_root, multiple) = fixture("mp4");
    fs::write(multiple.source_dir.join("extra.mp4"), b"extra").unwrap();
    cases.push((multiple_root, multiple));

    let (non_file_root, non_file) = fixture("mp4");
    fs::remove_file(non_file.source_dir.join("input-0").join("source.mp4")).unwrap();
    fs::create_dir(non_file.source_dir.join("input-0").join("source.mp4")).unwrap();
    cases.push((non_file_root, non_file));

    let (malformed_root, malformed) = fixture("mp4");
    fs::rename(
        malformed.source_dir.join("input-0").join("source.mp4"),
        malformed.source_dir.join("input-0").join("media.mp4"),
    )
    .unwrap();
    cases.push((malformed_root, malformed));

    let (unsafe_root, unsafe_extension) = fixture("exe");
    cases.push((unsafe_root, unsafe_extension));

    let (outside_root, mut outside) = fixture("mp4");
    let external = outside_root.path().join("outside");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("source.mp4"), b"source").unwrap();
    outside.source_dir = external;
    cases.push((outside_root, outside));

    for (_root, request) in cases {
        let resolver = FakeResolver::ready();
        let process = FakeProcess::success(OutputAction::Expected);
        assert_eq!(
            run(request, &resolver, &process),
            Err(DownloadErrorCode::OutputInvalid)
        );
        assert!(resolver.calls.lock().unwrap().is_empty());
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn rejects_symlink_or_reparse_source_components_when_windows_allows_them() {
    let (root, request) = fixture("mp4");
    let real = root.path().join("outside.mp4");
    fs::write(&real, b"source").unwrap();
    fs::remove_file(request.source_dir.join("input-0").join("source.mp4")).unwrap();

    #[cfg(windows)]
    if std::os::windows::fs::symlink_file(
        &real,
        request.source_dir.join("input-0").join("source.mp4"),
    )
    .is_err()
    {
        return;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, request.source_dir.join("input-0").join("source.mp4"))
        .unwrap();

    assert_eq!(fs::read_dir(&request.source_dir).unwrap().count(), 1);

    let resolver = FakeResolver::ready();
    let process = FakeProcess::success(OutputAction::Expected);
    assert_eq!(
        run(request, &resolver, &process),
        Err(DownloadErrorCode::OutputInvalid)
    );
    assert!(resolver.calls.lock().unwrap().is_empty());
    assert!(process.calls.lock().unwrap().is_empty());
}

#[cfg(windows)]
#[test]
fn holds_validated_source_file_and_directory_stable_across_process_launch() {
    let (root, request) = fixture("mp4");
    let replacement_file = root.path().join("replacement.mp4");
    fs::write(&replacement_file, b"replacement").unwrap();
    let replacement_dir = root.path().join("replacement-source");
    fs::create_dir(&replacement_dir).unwrap();
    fs::write(replacement_dir.join("source.mp4"), b"replacement").unwrap();
    let resolver = FakeResolver::ready();
    let process = FakeProcess::success(OutputAction::AttemptSourceSwap {
        input_index: 0,
        replacement_file,
        replacement_dir,
    });

    run(request, &resolver, &process).unwrap();

    assert_eq!(*process.swap_attempts.lock().unwrap(), [(false, false)]);
}

#[cfg(windows)]
#[test]
fn second_combined_source_is_pinned_against_file_and_slot_swap() {
    let (root, mut request) = fixture("mp4");
    fs::create_dir(request.source_dir.join("input-1")).unwrap();
    fs::write(request.source_dir.join("input-1/source.mp4"), b"second").unwrap();
    request.plan.source_format_ids.push("second".into());
    request.plan.audio_format_id = Some("second".into());
    request.audio_source = Some(policy_fixture::audio_format("second", "aac"));
    request.sources = policy_fixture::slots(&request.plan.source_format_ids);
    let replacement_file = root.path().join("replacement.mp4");
    let replacement_dir = root.path().join("replacement-slot");
    fs::write(&replacement_file, b"replacement").unwrap();
    fs::create_dir(&replacement_dir).unwrap();
    fs::write(replacement_dir.join("source.mp4"), b"replacement").unwrap();
    let process = FakeProcess::success(OutputAction::AttemptSourceSwap {
        input_index: 1,
        replacement_file,
        replacement_dir,
    });
    run(request, &FakeResolver::ready(), &process).unwrap();
    assert_eq!(*process.swap_attempts.lock().unwrap(), [(false, false)]);
}

#[test]
fn resolves_fresh_ffmpeg_and_stream_copies_the_validated_local_source() {
    let (_root, request) = fixture("mp4");
    let canonical_staging = fs::canonicalize(&request.staging_dir).unwrap();
    let canonical_source =
        fs::canonicalize(request.source_dir.join("input-0").join("source.mp4")).unwrap();
    let expected_output = canonical_staging.join("stage.mp4");
    let resolver = FakeResolver::ready();
    let process = FakeProcess::success(OutputAction::Expected);

    run(request, &resolver, &process).unwrap();

    assert_eq!(
        *resolver.calls.lock().unwrap(),
        [ToolName::Ffmpeg, ToolName::Ffprobe]
    );
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.program, PathBuf::from(r"C:\managed\ffmpeg.exe"));
    assert_eq!(call.working_dir, canonical_staging);
    assert_eq!(call.timeout, Some(Duration::from_secs(3600)));
    assert_eq!(call.inactivity_timeout, None);
    assert_eq!(call.stdout_limit, 32_768);
    assert_eq!(call.stderr_limit, 32_768);
    let args: Vec<_> = call
        .args
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "-hide_banner".to_owned(),
            "-loglevel".to_owned(),
            "error".to_owned(),
            "-nostdin".to_owned(),
            "-ss".to_owned(),
            "00:00:01.234".to_owned(),
            "-i".to_owned(),
            canonical_source.to_string_lossy().into_owned(),
            "-t".to_owned(),
            "00:00:05.678".to_owned(),
            "-map".to_owned(),
            "0:v:0".to_owned(),
            "-c:v".to_owned(),
            "copy".to_owned(),
            "-map".to_owned(),
            "0:a:0".to_owned(),
            "-c:a".to_owned(),
            "copy".to_owned(),
            "-sn".to_owned(),
            "-dn".to_owned(),
            "-avoid_negative_ts".to_owned(),
            "make_zero".to_owned(),
            "-movflags".to_owned(),
            "+faststart".to_owned(),
            "-n".to_owned(),
            expected_output.to_string_lossy().into_owned(),
        ]
    );
    assert_eq!(fs::read(expected_output).unwrap(), b"clip");
}

#[test]
fn maps_tool_process_nonzero_disk_full_and_invalid_output_failures() {
    let tool_cases = [
        (
            ManagedToolErrorCode::ToolMissing,
            DownloadErrorCode::ToolMissing,
        ),
        (
            ManagedToolErrorCode::ToolIntegrityFailed,
            DownloadErrorCode::ToolIntegrityFailed,
        ),
        (
            ManagedToolErrorCode::ToolProbeFailed,
            DownloadErrorCode::ProcessFailed,
        ),
    ];
    for (native, public) in tool_cases {
        let (_root, request) = fixture("mp4");
        let resolver = FakeResolver::failing(native);
        let process = FakeProcess::success(OutputAction::Expected);
        assert_eq!(run(request, &resolver, &process), Err(public));
        assert!(process.calls.lock().unwrap().is_empty());
    }

    for failure in [
        ProcessFailureKind::LaunchFailed,
        ProcessFailureKind::TimedOut,
        ProcessFailureKind::Canceled,
    ] {
        let (_root, request) = fixture("mp4");
        let resolver = FakeResolver::ready();
        let process = FakeProcess::failure(failure);
        assert_eq!(
            run(request, &resolver, &process),
            Err(DownloadErrorCode::ProcessFailed)
        );
    }

    let (_root, request) = fixture("mp4");
    let resolver = FakeResolver::ready();
    let process = FakeProcess::failure(ProcessFailureKind::StderrLimitExceeded);
    assert_eq!(
        run(request, &resolver, &process),
        Err(DownloadErrorCode::OutputInvalid)
    );

    let (_root, request) = fixture("mp4");
    let resolver = FakeResolver::ready();
    let process = FakeProcess::exit(1, b"No space left on device: private path");
    assert_eq!(
        run(request, &resolver, &process),
        Err(DownloadErrorCode::DiskFull)
    );

    let (_root, request) = fixture("mp4");
    let resolver = FakeResolver::ready();
    let process = FakeProcess::exit(1, b"private ffmpeg diagnostic and path");
    assert_eq!(
        run(request, &resolver, &process),
        Err(DownloadErrorCode::ProcessFailed)
    );

    for action in [OutputAction::None, OutputAction::ExpectedAndUnexpected] {
        let (_root, request) = fixture("mp4");
        let resolver = FakeResolver::ready();
        let process = FakeProcess::success(action);
        assert_eq!(
            run(request, &resolver, &process),
            Err(DownloadErrorCode::OutputInvalid)
        );
    }
}

#[test]
fn passes_the_same_cancellation_token_to_ffmpeg() {
    let (_root, request) = fixture("mp4");
    let resolver = FakeResolver::ready();
    let process = FakeProcess::success(OutputAction::Cancel);
    let cancellation = CancellationToken::new();

    assert_eq!(
        FfmpegMediaProcessor::new(&resolver, &process)
            .process(request, cancellation.clone())
            .map(|_| ()),
        Err(DownloadErrorCode::ProcessFailed)
    );
    assert!(cancellation.is_canceled());
}
#[test]
fn ordinary_processing_rejects_unauthorized_audio_conversion_before_tool_use() {
    use local_media_downloader_lib::domain::output_policy::{AudioProcessing, TargetAudioCodec};
    for (source_video, source_audio, target, codec, audio_arg, bitrate) in [
        (
            "av1",
            "opus",
            "mp4",
            TargetAudioCodec::Aac,
            "aac",
            Some("192k"),
        ),
        (
            "vp9",
            "aac",
            "webm",
            TargetAudioCodec::Opus,
            "libopus",
            Some("128k"),
        ),
        ("vp9", "opus", "webm", TargetAudioCodec::Opus, "copy", None),
        ("h264", "aac", "mp4", TargetAudioCodec::Aac, "copy", None),
    ] {
        let (_root, mut request) = fixture("mkv");
        request.plan.clip = None;
        request.plan.output_container = Some(target.into());
        request.plan.output_audio_codec = Some(codec);
        request.plan.audio_processing = Some(if bitrate.is_some() {
            AudioProcessing::Transcode
        } else {
            AudioProcessing::Copy
        });
        request.video_source = Some(policy_fixture::video_format("18", source_video));
        request.plan.output_video_codec = Some(source_video.into());
        request.audio_source = Some(policy_fixture::audio_format("18", source_audio));
        request.plan.container_selection =
            local_media_downloader_lib::domain::output_policy::ContainerSelection::Explicit;
        let process = FakeProcess {
            video_codec: source_video,
            audio_codec: source_audio,
            ..FakeProcess::success(OutputAction::Expected)
        };
        let output = request.staging_dir.join(format!("stage.{target}"));
        if bitrate.is_some() {
            assert_eq!(
                run(request, &FakeResolver::ready(), &process),
                Err(DownloadErrorCode::FormatUnavailable)
            );
            assert!(process.calls.lock().unwrap().is_empty());
            assert!(!output.exists());
            continue;
        }
        run(request, &FakeResolver::ready(), &process).unwrap();
        let calls = process.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "one encoding/remux pass after source probe");
        assert!(calls[0]
            .args
            .windows(2)
            .any(|args| args == ["-c:v", "copy"]));
        assert!(calls[0]
            .args
            .windows(2)
            .any(|args| args == ["-c:a", audio_arg]));
        if let Some(rate) = bitrate {
            assert!(calls[0].args.windows(2).any(|args| args == ["-b:a", rate]));
        } else {
            assert!(!calls[0].args.iter().any(|arg| arg == "-b:a"));
        }
        assert!(output.is_file());
    }
}

#[test]
fn ordinary_processing_rejects_unauthorized_video_conversion_before_tool_use() {
    let (_root, mut request) = fixture("mp4");
    request.plan.video_processing =
        Some(local_media_downloader_lib::domain::output_policy::StreamProcessing::Transcode);
    let process = FakeProcess::success(OutputAction::Expected);
    assert_eq!(
        run(request, &FakeResolver::ready(), &process),
        Err(DownloadErrorCode::FormatUnavailable)
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn bounded_hevc_refinement_proves_colors_missing_from_stream_summary() {
    let (_root, mut request) = fixture("mkv");
    request.plan.clip = None;
    request.video_source = Some(policy_fixture::video_format("18", "hevc"));
    request.plan.output_video_codec = Some("hevc".into());
    let process = FakeProcess {
        video_codec: "hevc",
        ..FakeProcess::success(OutputAction::Expected)
    };
    let result = FfmpegMediaProcessor::new(&FakeResolver::ready(), &process)
        .process(request, CancellationToken::new())
        .unwrap();
    let source = result.evidence.source_video.unwrap();
    assert_eq!(source.color_transfer.as_deref(), Some("smpte2084"));
    assert_eq!(source.color_primaries.as_deref(), Some("bt2020"));
    assert_eq!(source.hdr_side_data.len(), 1);
    let calls = process.calls.lock().unwrap();
    assert!(calls[0]
        .args
        .windows(2)
        .any(|args| args == ["-c:v", "copy"]));
}

#[test]
fn advertised_hdr_without_verified_transfer_is_not_silently_saved_as_sdr() {
    let (_root, mut request) = fixture("mp4");
    request.video_source.as_mut().unwrap().dynamic_range = Some("HDR10".into());
    let process = FakeProcess::success(OutputAction::Expected);
    assert_eq!(
        run(request, &FakeResolver::ready(), &process),
        Err(DownloadErrorCode::FormatUnavailable)
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn automatic_mux_refines_container_without_changing_sources() {
    let (_root, mut request) = fixture("mkv");
    request.plan.clip = None;
    request.video_source = Some(policy_fixture::video_format("18", "vp8"));
    request.plan.output_video_codec = Some("vp8".into());
    let staging = request.staging_dir.clone();
    let process = FakeProcess {
        video_codec: "vp8",
        ..FakeProcess::success(OutputAction::Expected)
    };
    let result = FfmpegMediaProcessor::new(&FakeResolver::ready(), &process)
        .process(request, CancellationToken::new())
        .unwrap();
    assert_eq!(
        result.evidence.actual_container,
        local_media_downloader_lib::domain::output_policy::OutputContainer::Mkv
    );
    assert!(staging.join("stage.mkv").is_file());
    assert!(!staging.join("stage.mp4").exists());
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].args.windows(2).any(|p| p == ["-c:v", "copy"]));
    assert!(calls[0].args.windows(2).any(|p| p == ["-c:a", "copy"]));
    assert!(!calls[0]
        .args
        .iter()
        .any(|a| a == "-movflags" || a == "-b:a"));
}

#[test]
fn distinct_combined_sources_map_only_the_selected_roles() {
    let (_root, mut request) = fixture("mp4");
    fs::create_dir(request.source_dir.join("input-1")).unwrap();
    fs::write(request.source_dir.join("input-1/source.mp4"), b"second").unwrap();
    request.plan.source_format_ids = vec!["18".into(), "second".into()];
    request.plan.audio_format_id = Some("second".into());
    request.plan.operation =
        local_media_downloader_lib::domain::format_selection::SelectionOperation::Merge;
    request.audio_source = Some(policy_fixture::audio_format("second", "aac"));
    request.sources = policy_fixture::slots(&request.plan.source_format_ids);
    let process = FakeProcess::success(OutputAction::Expected);
    run(request, &FakeResolver::ready(), &process).unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls[0].args.iter().filter(|a| *a == "-i").count(), 2);
    let maps: Vec<_> = calls[0]
        .args
        .windows(2)
        .filter(|p| p[0] == "-map")
        .map(|p| p[1].to_string_lossy().into_owned())
        .collect();
    assert_eq!(maps, ["0:v:0", "1:a:0"]);
    assert_eq!(calls[0].args.iter().filter(|a| *a == "-ss").count(), 2);
}

#[test]
fn copy_pipeline_rejects_a_false_output_video_codec_claim() {
    let (_root, mut request) = fixture("mp4");
    request.plan.video_processing =
        Some(local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy);
    request.plan.output_video_codec = Some("av1".into());
    let process = FakeProcess::success(OutputAction::Expected);
    assert_eq!(
        run(request, &FakeResolver::ready(), &process),
        Err(DownloadErrorCode::OutputInvalid)
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn audio_only_removes_video_and_unknown_audio_properties_fail_without_output() {
    for invalid in [false, true] {
        let (_root, mut request) = fixture("mkv");
        request.plan.video_format_id = None;
        request.video_source = None;
        request.plan.video_processing = None;
        request.plan.output_video_codec = None;
        request.plan.output_container = Some("m4a".into());
        request.plan.clip = None;
        let process = FakeProcess {
            channels: if invalid { 0 } else { 2 },
            ..FakeProcess::success(OutputAction::Expected)
        };
        let output = request.staging_dir.join("stage.m4a");
        let result = run(request, &FakeResolver::ready(), &process);
        if invalid {
            assert_eq!(result, Err(DownloadErrorCode::OutputInvalid));
            assert!(!output.exists());
        } else {
            result.unwrap();
            let calls = process.calls.lock().unwrap();
            assert!(calls[0].args.iter().any(|arg| arg == "-vn"));
            assert!(!calls[0]
                .args
                .iter()
                .any(|arg| arg.to_string_lossy().starts_with("0:v")));
            assert!(output.is_file());
        }
    }
}
