#[path = "support/output_policy.rs"]
mod policy_fixture;
use std::{ffi::OsString, fs, path::PathBuf, sync::Mutex, time::Duration};

use local_media_downloader_lib::{
    application::{
        download_executor::StagedDownload,
        output_verifier::OutputVerifier,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::{
            ClipRange, SelectionMode, SelectionOperation, SelectionPlan, SelectionWarning,
        },
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
    },
    infrastructure::ffprobe::FfprobeOutputVerifier,
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
}

impl ManagedToolResolver for FakeResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        self.calls.lock().unwrap().push(name);
        if let Some(code) = self.error {
            return Err(ManagedToolError::new(code, "private"));
        }
        Ok(ResolvedTool {
            name,
            version: "1.2.3".to_owned(),
            path: PathBuf::from(r"C:\managed\ffprobe.exe"),
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

struct FakeProcess {
    result: Mutex<Option<Result<ProcessExit, ProcessFailureKind>>>,
    calls: Mutex<Vec<Invocation>>,
}

impl FakeProcess {
    fn json(stdout: Vec<u8>) -> Self {
        Self {
            result: Mutex::new(Some(Ok(ProcessExit {
                code: 0,
                stdout,
                stderr: Vec::new(),
            }))),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn failure(error: ProcessFailureKind) -> Self {
        Self {
            result: Mutex::new(Some(Err(error))),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl ProcessSupervisor for FakeProcess {
    fn run(
        &self,
        request: ProcessRequest,
        _cancellation: CancellationToken,
        _sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let (stdout_limit, stderr_limit) = match request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => (stdout_limit, stderr_limit),
            OutputMode::Lines { .. } => panic!("ffprobe must use bounded capture mode"),
        };
        self.calls.lock().unwrap().push(Invocation {
            program: request.program.path().to_owned(),
            args: request.args,
            working_dir: request.working_dir,
            timeout: request.timeout,
            inactivity_timeout: request.inactivity_timeout,
            stdout_limit,
            stderr_limit,
        });
        self.result.lock().unwrap().take().unwrap()
    }
}

fn plan(operation: SelectionOperation, clip: Option<ClipRange>) -> SelectionPlan {
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
        mode: SelectionMode::BestAvailable,
        source_format_ids,
        video_format_id,
        audio_format_id,
        operation,
        output_container,
        output_audio_codec: (operation != SelectionOperation::ExtractVideo)
            .then_some(local_media_downloader_lib::domain::output_policy::TargetAudioCodec::Aac),
        audio_processing: (operation != SelectionOperation::ExtractVideo)
            .then_some(local_media_downloader_lib::domain::output_policy::AudioProcessing::Copy),
        clip,
        estimated_size_bytes: None,
        warnings: vec![SelectionWarning::ContainerUnverified],
    }
}

#[test]
fn approved_compatibility_verifies_targets_without_accepting_dimension_rate_or_layout_changes() {
    use local_media_downloader_lib::domain::output_policy::{
        ContainerSelection, StreamProcessing, TargetAudioCodec,
    };
    for corruption in [
        "none",
        "width",
        "fps",
        "pixel",
        "channels",
        "sample_rate",
        "layout",
    ] {
        let (_root, dir) = stage_file("mp4", 20);
        let mut approved = plan(SelectionOperation::Keep, None);
        approved.compatibility_mode = true;
        approved.container_selection = ContainerSelection::Compatibility;
        approved.video_processing = Some(StreamProcessing::Transcode);
        approved.audio_processing = Some(StreamProcessing::Transcode);
        let mut job = staged(dir, SelectionOperation::Keep, None);
        let video = job.evidence.source_video.as_mut().unwrap();
        video.codec = "av1".into();
        video.profile = Some("Main".into());
        video.color_range = Some("tv".into());
        video.color_space = Some("bt709".into());
        video.color_transfer = Some("bt709".into());
        video.color_primaries = Some("bt709".into());
        let audio = job.evidence.source_audio.as_mut().unwrap();
        audio.codec = TargetAudioCodec::Opus;
        audio.channel_layout = Some("stereo".into());
        job.evidence.video_processing = Some(StreamProcessing::Transcode);
        job.evidence.audio_processing = Some(StreamProcessing::Transcode);
        let mut document: serde_json::Value =
            serde_json::from_slice(&probe_json(20, "3", "mov,mp4,m4a", &["video", "audio"]))
                .unwrap();
        document["streams"][0]["profile"] = serde_json::json!("High");
        for (key, value) in [
            ("color_range", "tv"),
            ("color_space", "bt709"),
            ("color_transfer", "bt709"),
            ("color_primaries", "bt709"),
        ] {
            document["streams"][0][key] = serde_json::json!(value);
        }
        document["streams"][1]["channel_layout"] = serde_json::json!("stereo");
        match corruption {
            "width" => document["streams"][0]["width"] = serde_json::json!(1278),
            "fps" => document["streams"][0]["avg_frame_rate"] = serde_json::json!("24/1"),
            "pixel" => document["streams"][0]["pix_fmt"] = serde_json::json!("yuv420p10le"),
            "channels" => document["streams"][1]["channels"] = serde_json::json!(1),
            "sample_rate" => document["streams"][1]["sample_rate"] = serde_json::json!("44100"),
            "layout" => document["streams"][1]["channel_layout"] = serde_json::json!("mono"),
            _ => {}
        }
        let result = FfprobeOutputVerifier::new(
            FakeResolver::ready(),
            FakeProcess::json(serde_json::to_vec(&document).unwrap()),
        )
        .verify(job, &approved, CancellationToken::new());
        if corruption == "none" {
            assert!(result.is_ok(), "approved target rejected: {result:?}");
        } else {
            assert_eq!(
                result.err(),
                Some(DownloadErrorCode::OutputInvalid),
                "{corruption}"
            );
        }
    }
}

#[test]
fn compatibility_hdr_output_frame_must_not_keep_stale_hdr_metadata() {
    use local_media_downloader_lib::domain::output_policy::{ContainerSelection, StreamProcessing};
    struct FrameProcess {
        summary: Vec<u8>,
        stale: Option<usize>,
    }
    impl ProcessSupervisor for FrameProcess {
        fn run(
            &self,
            request: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let stdout = if request.args.iter().any(|a| a == "-show_frames") {
                let frames: Vec<_> = (0..2).map(|index|serde_json::json!({"pix_fmt":"yuv420p","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","side_data_list":if self.stale==Some(index) {vec![serde_json::json!({"side_data_type":"Mastering display metadata","max_luminance":"1000/1"})]}else{vec![]} })).collect();
                serde_json::to_vec(&serde_json::json!({"frames":frames})).unwrap()
            } else {
                self.summary.clone()
            };
            Ok(ProcessExit {
                code: 0,
                stdout,
                stderr: vec![],
            })
        }
    }
    for stale in [Some(0), Some(1), None] {
        let (_root, dir) = stage_file("mp4", 20);
        let mut approved = plan(SelectionOperation::ExtractVideo, None);
        approved.compatibility_mode = true;
        approved.container_selection = ContainerSelection::Compatibility;
        approved.video_processing = Some(StreamProcessing::Transcode);
        let mut job = staged(dir, SelectionOperation::ExtractVideo, None);
        job.evidence.video_processing = Some(StreamProcessing::Transcode);
        let source = job.evidence.source_video.as_mut().unwrap();
        source.codec = "hevc".into();
        source.profile = Some("Main 10".into());
        source.pixel_format = Some("yuv420p10le".into());
        source.color_range = Some("tv".into());
        source.color_space = Some("bt2020nc".into());
        source.color_transfer = Some("smpte2084".into());
        source.color_primaries = Some("bt2020".into());
        source.hdr_side_data = vec![
            serde_json::json!({"side_data_type":"Mastering display metadata","max_luminance":"1000/1"}),
        ];
        let mut summary: serde_json::Value =
            serde_json::from_slice(&probe_json(20, "1", "mov,mp4,m4a", &["video"])).unwrap();
        summary["streams"][0]["profile"] = serde_json::json!("High");
        for (key, value) in [
            ("color_range", "tv"),
            ("color_space", "bt709"),
            ("color_transfer", "bt709"),
            ("color_primaries", "bt709"),
        ] {
            summary["streams"][0][key] = serde_json::json!(value);
        }
        let result = FfprobeOutputVerifier::new(
            FakeResolver::ready(),
            FrameProcess {
                summary: serde_json::to_vec(&summary).unwrap(),
                stale,
            },
        )
        .verify(job, &approved, CancellationToken::new());
        if stale.is_some() {
            assert_eq!(result.err(), Some(DownloadErrorCode::OutputInvalid));
        } else {
            assert!(result.is_ok());
        }
    }
}

fn staged(
    staging_dir: PathBuf,
    operation: SelectionOperation,
    clip: Option<ClipRange>,
) -> StagedDownload {
    StagedDownload {
        job_id: "42".to_owned(),
        staging_dir,
        expected_operation: operation,
        expected_container: Some(
            if operation == SelectionOperation::ExtractAudio {
                "m4a"
            } else {
                "mp4"
            }
            .to_owned(),
        ),
        requested_clip: clip,
        evidence: policy_fixture::evidence(&plan(operation, clip)),
    }
}

fn probe_json(size: u64, duration: &str, container: &str, streams: &[&str]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "programs": [],
        "stream_groups": [],
        "streams": streams
            .iter()
            .map(|stream| match *stream {
                "video" => serde_json::json!({"codec_type": "video", "codec_name": "h264", "width": 1280, "height": 720, "avg_frame_rate": "30/1", "pix_fmt": "yuv420p"}),
                "audio" => serde_json::json!({"codec_type": "audio", "codec_name": "aac", "channels": 2, "sample_rate": "48000", "avg_frame_rate": "0/0"}),
                other => serde_json::json!({"codec_type": other}),
            })
            .collect::<Vec<_>>(),
        "format": {
            "format_name": container,
            "duration": duration,
            "size": size.to_string()
        }
    }))
    .unwrap()
}

fn stage_file(extension: &str, length: usize) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("42");
    fs::create_dir(&staging).unwrap();
    fs::write(
        staging.join(format!("stage.{extension}")),
        vec![b'x'; length],
    )
    .unwrap();
    (root, staging)
}

#[test]
fn copy_output_missing_summary_colors_requires_matching_bounded_frame_evidence() {
    struct CopyFrames {
        frames: serde_json::Value,
        missing_summary_pixel: bool,
    }
    impl ProcessSupervisor for CopyFrames {
        fn run(
            &self,
            request: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let stdout = if request.args.iter().any(|arg| arg == "-show_frames") {
                assert!(request
                    .args
                    .windows(2)
                    .any(|p| p[0] == "-read_intervals" && p[1] == "%+#32"));
                serde_json::to_vec(&serde_json::json!({"frames":self.frames})).unwrap()
            } else {
                let mut summary: serde_json::Value = serde_json::from_slice(&probe_json(
                    20,
                    "3",
                    "mov,mp4,m4a",
                    &["video", "audio"],
                ))
                .unwrap();
                if self.missing_summary_pixel {
                    summary["streams"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("pix_fmt");
                }
                serde_json::to_vec(&summary).unwrap()
            };
            Ok(ProcessExit {
                code: 0,
                stdout,
                stderr: vec![],
            })
        }
    }
    for (case, missing_summary_pixel) in [
        ("matching", false),
        ("different", false),
        ("missing", false),
        ("matching", true),
        ("different_pixel", true),
        ("missing_pixel", true),
        ("missing", true),
    ] {
        let (_root, dir) = stage_file("mp4", 20);
        let mut job = staged(dir, SelectionOperation::Keep, None);
        let video = job.evidence.source_video.as_mut().unwrap();
        video.pixel_format = Some("yuv420p".into());
        video.color_range = Some("tv".into());
        video.color_space = Some("bt709".into());
        video.color_transfer = Some("bt709".into());
        video.color_primaries = Some("bt709".into());
        let frames = if case == "missing" {
            serde_json::json!([])
        } else {
            serde_json::json!([{"pix_fmt":if case == "missing_pixel" {None} else {Some(if case == "different_pixel" {"yuv422p"} else {"yuv420p"})},"color_range":if case == "different" {"pc"} else {"tv"},"color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","side_data_list":[]}])
        };
        let result = FfprobeOutputVerifier::new(
            FakeResolver::ready(),
            CopyFrames {
                frames,
                missing_summary_pixel,
            },
        )
        .verify(
            job,
            &plan(SelectionOperation::Keep, None),
            CancellationToken::new(),
        );
        if case == "matching" {
            assert!(result.is_ok(), "frame-proven original pixel/color must not be rejected because the summary omits it; pixel missing={missing_summary_pixel}");
        } else {
            assert_eq!(result.err(), Some(DownloadErrorCode::OutputInvalid));
        }
    }
}

#[test]
fn resolves_managed_ffprobe_and_uses_an_exact_bounded_probe_contract() {
    let (_root, staging) = stage_file("mp4", 321);
    let resolver = FakeResolver::ready();
    let process = FakeProcess::json(probe_json(
        321,
        "20.050000",
        "mov,mp4,m4a,3gp,3g2,mj2",
        &["video", "audio"],
    ));
    let verified = FfprobeOutputVerifier::new(&resolver, &process)
        .verify(
            staged(staging.clone(), SelectionOperation::Merge, None),
            &plan(SelectionOperation::Merge, None),
            CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(*resolver.calls.lock().unwrap(), [ToolName::Ffprobe]);
    assert_eq!(
        verified.path,
        fs::canonicalize(staging.join("stage.mp4")).unwrap()
    );
    assert_eq!(verified.container, "mp4");
    assert!(verified.has_video);
    assert!(verified.has_audio);
    assert_eq!(verified.duration_ms, 20_050);
    assert_eq!(verified.byte_length, 321);

    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.program, PathBuf::from(r"C:\managed\ffprobe.exe"));
    assert_eq!(call.working_dir, fs::canonicalize(&staging).unwrap());
    assert_eq!(call.timeout, Some(Duration::from_secs(30)));
    assert_eq!(call.inactivity_timeout, None);
    assert_eq!(call.stdout_limit, 262_144);
    assert_eq!(call.stderr_limit, 32_768);
    let args: Vec<_> = call
        .args
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_entries",
            "format=format_name,duration,size:stream=codec_type,codec_name,profile,width,height,avg_frame_rate,pix_fmt,color_range,color_space,color_transfer,color_primaries,channels,sample_rate,channel_layout:stream_side_data",
            "--",
            verified.path.to_string_lossy().as_ref(),
        ]
    );
}

#[test]
fn verifies_stream_presence_for_combined_and_audio_only_outputs() {
    let (_root, staging) = stage_file("mp4", 10);
    let combined = FfprobeOutputVerifier::new(
        FakeResolver::ready(),
        FakeProcess::json(probe_json(10, "1", "mov,mp4,m4a", &["video", "audio"])),
    )
    .verify(
        staged(staging, SelectionOperation::Keep, None),
        &plan(SelectionOperation::Keep, None),
        CancellationToken::new(),
    )
    .unwrap();
    assert!(combined.has_video && combined.has_audio);

    let (_root, staging) = stage_file("m4a", 10);
    let audio = FfprobeOutputVerifier::new(
        FakeResolver::ready(),
        FakeProcess::json(probe_json(10, "1", "mov,mp4,m4a", &["audio"])),
    )
    .verify(
        staged(staging, SelectionOperation::ExtractAudio, None),
        &plan(SelectionOperation::ExtractAudio, None),
        CancellationToken::new(),
    )
    .unwrap();
    assert!(!audio.has_video && audio.has_audio);
}

#[test]
fn video_only_output_requires_one_video_and_zero_audio_streams() {
    for (streams, valid) in [
        (vec!["video"], true),
        (vec!["video", "audio"], false),
        (vec!["audio"], false),
        (vec!["video", "video"], false),
    ] {
        let (_root, staging) = stage_file("mp4", 10);
        let result = FfprobeOutputVerifier::new(
            FakeResolver::ready(),
            FakeProcess::json(probe_json(10, "1", "mov,mp4,m4a", &streams)),
        )
        .verify(
            staged(staging, SelectionOperation::ExtractVideo, None),
            &plan(SelectionOperation::ExtractVideo, None),
            CancellationToken::new(),
        );
        assert_eq!(result.is_ok(), valid);
    }
}

#[test]
fn rejects_zero_multiple_unexpected_empty_excessive_or_non_regular_staging_entries() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("42");
    fs::create_dir(&staging).unwrap();
    for mutate in 0..7 {
        if staging.exists() {
            fs::remove_dir_all(&staging).unwrap();
        }
        fs::create_dir(&staging).unwrap();
        match mutate {
            0 => {}
            1 => {
                fs::write(staging.join("stage.mp4"), b"x").unwrap();
                fs::write(staging.join("stage.webm"), b"x").unwrap();
            }
            2 => fs::write(staging.join("private-info.json"), b"x").unwrap(),
            3 => fs::create_dir(staging.join("stage.mp4")).unwrap(),
            4 => fs::write(staging.join("stage.mp4"), b"").unwrap(),
            5 => {
                for index in 0..9 {
                    fs::write(staging.join(format!("unexpected-{index}")), b"x").unwrap();
                }
            }
            6 => {
                fs::write(staging.join("stage.mp4"), b"x").unwrap();
                fs::create_dir(staging.join("source")).unwrap();
                fs::write(staging.join("source").join("source.mp4"), b"full source").unwrap();
            }
            _ => unreachable!(),
        }
        let process = FakeProcess::json(probe_json(1, "1", "mov,mp4,m4a", &["video", "audio"]));
        assert_eq!(
            FfprobeOutputVerifier::new(FakeResolver::ready(), &process)
                .verify(
                    staged(staging.clone(), SelectionOperation::Keep, None),
                    &plan(SelectionOperation::Keep, None),
                    CancellationToken::new(),
                )
                .err(),
            Some(DownloadErrorCode::OutputInvalid)
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[cfg(windows)]
#[test]
fn rejects_a_symlink_candidate_when_windows_allows_creating_one() {
    use std::os::windows::fs::symlink_file;

    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("42");
    fs::create_dir(&staging).unwrap();
    let outside = root.path().join("outside.mp4");
    fs::write(&outside, b"x").unwrap();
    if symlink_file(&outside, staging.join("stage.mp4")).is_err() {
        return;
    }
    let process = FakeProcess::json(probe_json(1, "1", "mov,mp4,m4a", &["video", "audio"]));
    assert_eq!(
        FfprobeOutputVerifier::new(FakeResolver::ready(), &process)
            .verify(
                staged(staging, SelectionOperation::Keep, None),
                &plan(SelectionOperation::Keep, None),
                CancellationToken::new(),
            )
            .err(),
        Some(DownloadErrorCode::OutputInvalid)
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn rejects_malformed_oversized_nonfinite_unrecognized_and_policy_violating_probe_output() {
    let cases = [
        b"not json".to_vec(),
        br#"{"streams":[],"format":[]}"#.to_vec(),
        probe_json(20, "NaN", "mov,mp4,m4a", &["video", "audio"]),
        probe_json(20, "0", "mov,mp4,m4a", &["video", "audio"]),
        probe_json(20, "1", "unknown", &["video", "audio"]),
        probe_json(21, "1", "mov,mp4,m4a", &["video", "audio"]),
        probe_json(20, "1", "mov,mp4,m4a", &["video"]),
        probe_json(20, "1", "mov,mp4,m4a", &["video", "audio", "subtitle"]),
        probe_json(20, "1", "mov,mp4,m4a", &["video", "video", "audio"]),
        br#"{"programs":[{}],"stream_groups":[],"streams":[{"codec_type":"video"},{"codec_type":"audio"}],"format":{"format_name":"mov,mp4,m4a","duration":"1","size":"20"}}"#.to_vec(),
        br#"{"streams":[{"codec_type":"video"},{"codec_type":"audio"}],"format":{"format_name":"mov,mp4,m4a","duration":"1","size":"20","private":"x"}}"#.to_vec(),
    ];
    for stdout in cases {
        let (_root, staging) = stage_file("mp4", 20);
        assert_eq!(
            FfprobeOutputVerifier::new(FakeResolver::ready(), FakeProcess::json(stdout))
                .verify(
                    staged(staging, SelectionOperation::Merge, None),
                    &plan(SelectionOperation::Merge, None),
                    CancellationToken::new(),
                )
                .err(),
            Some(DownloadErrorCode::OutputInvalid)
        );
    }

    let (_root, staging) = stage_file("mp4", 20);
    assert_eq!(
        FfprobeOutputVerifier::new(
            FakeResolver::ready(),
            FakeProcess::failure(ProcessFailureKind::StdoutLimitExceeded),
        )
        .verify(
            staged(staging, SelectionOperation::Merge, None),
            &plan(SelectionOperation::Merge, None),
            CancellationToken::new(),
        )
        .err(),
        Some(DownloadErrorCode::OutputInvalid)
    );
}

#[test]
fn clip_duration_allows_only_the_requested_range_plus_ten_seconds() {
    let clip = ClipRange {
        start_ms: 10_000,
        end_ms: 30_000,
    };
    for (duration, expected) in [
        ("0.001", Ok(())),
        ("30.000", Ok(())),
        ("30.001", Err(DownloadErrorCode::OutputInvalid)),
    ] {
        let (_root, staging) = stage_file("mp4", 20);
        let result = FfprobeOutputVerifier::new(
            FakeResolver::ready(),
            FakeProcess::json(probe_json(20, duration, "mov,mp4,m4a", &["video", "audio"])),
        )
        .verify(
            staged(staging, SelectionOperation::Merge, Some(clip)),
            &plan(SelectionOperation::Merge, Some(clip)),
            CancellationToken::new(),
        )
        .map(|_| ());
        assert_eq!(result, expected);
    }
}

#[test]
fn maps_tool_and_process_failures_without_exposing_diagnostics() {
    let (_root, staging) = stage_file("mp4", 20);
    let resolver = FakeResolver {
        calls: Mutex::new(Vec::new()),
        error: Some(ManagedToolErrorCode::ToolIntegrityFailed),
    };
    assert_eq!(
        FfprobeOutputVerifier::new(resolver, FakeProcess::json(Vec::new()))
            .verify(
                staged(staging, SelectionOperation::Merge, None),
                &plan(SelectionOperation::Merge, None),
                CancellationToken::new(),
            )
            .err(),
        Some(DownloadErrorCode::ToolIntegrityFailed)
    );

    for (failure, expected) in [
        (
            ProcessFailureKind::TimedOut,
            DownloadErrorCode::ProcessFailed,
        ),
        (
            ProcessFailureKind::Canceled,
            DownloadErrorCode::ProcessFailed,
        ),
        (
            ProcessFailureKind::StderrLimitExceeded,
            DownloadErrorCode::OutputInvalid,
        ),
    ] {
        let (_root, staging) = stage_file("mp4", 20);
        assert_eq!(
            FfprobeOutputVerifier::new(FakeResolver::ready(), FakeProcess::failure(failure),)
                .verify(
                    staged(staging, SelectionOperation::Merge, None),
                    &plan(SelectionOperation::Merge, None),
                    CancellationToken::new(),
                )
                .err(),
            Some(expected)
        );
    }
}
#[test]
fn rejects_wrong_audio_codec_and_changed_video_properties_before_publication() {
    for (field, value, stream_index) in [
        ("codec_name", serde_json::json!("opus"), 1),
        ("channels", serde_json::json!(6), 1),
        ("sample_rate", serde_json::json!("44100"), 1),
        ("channel_layout", serde_json::json!("stereo"), 1),
        ("codec_name", serde_json::json!("av1"), 0),
        ("width", serde_json::json!(1920), 0),
        ("height", serde_json::json!(1080), 0),
        ("avg_frame_rate", serde_json::json!("60/1"), 0),
        ("color_transfer", serde_json::json!("smpte2084"), 0),
        ("pix_fmt", serde_json::json!("yuv420p10le"), 0),
    ] {
        let (_root, staging) = stage_file("mp4", 10);
        let mut json: serde_json::Value =
            serde_json::from_slice(&probe_json(10, "1", "mov,mp4,m4a", &["video", "audio"]))
                .unwrap();
        json["streams"][stream_index][field] = value;
        assert_eq!(
            FfprobeOutputVerifier::new(
                FakeResolver::ready(),
                FakeProcess::json(serde_json::to_vec(&json).unwrap())
            )
            .verify(
                staged(staging, SelectionOperation::Keep, None),
                &plan(SelectionOperation::Keep, None),
                CancellationToken::new()
            )
            .err(),
            Some(DownloadErrorCode::OutputInvalid),
            "changed {field}"
        );
    }
}

#[test]
fn automatic_container_refinement_verifies_the_actual_mkv_without_changing_codecs() {
    use local_media_downloader_lib::domain::output_policy::OutputContainer;
    let (_root, directory) = stage_file("mkv", 10);
    let mut plan = plan(SelectionOperation::Keep, None);
    plan.output_video_codec = Some("vp8".into());
    let mut staged = staged(directory, SelectionOperation::Keep, None);
    staged.expected_container = Some("mkv".into());
    staged.evidence.actual_container = OutputContainer::Mkv;
    staged.evidence.source_video.as_mut().unwrap().codec = "vp8".into();
    let mut json: serde_json::Value =
        serde_json::from_slice(&probe_json(10, "1", "matroska,webm", &["video", "audio"])).unwrap();
    json["streams"][0]["codec_name"] = serde_json::json!("vp8");
    let output = FfprobeOutputVerifier::new(
        FakeResolver::ready(),
        FakeProcess::json(serde_json::to_vec(&json).unwrap()),
    )
    .verify(staged, &plan, CancellationToken::new())
    .unwrap();
    assert_eq!(output.container, "mkv");
    assert!(output.has_video && output.has_audio);
}
