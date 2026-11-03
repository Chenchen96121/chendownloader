use std::{collections::VecDeque, ffi::OsString, path::PathBuf, sync::Mutex, time::Duration};

use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        managed_tools::{ManagedToolError, ToolName},
        media::AnalysisErrorCode,
    },
    infrastructure::ytdlp::YtDlpInstagramVideoAnalyzer,
};

const REEL_URL: &str = "https://www.instagram.com/reel/Reel_01/";
const POST_URL: &str = "https://www.instagram.com/p/Post_01/";
const REEL: &[u8] = include_bytes!("../../tests/fixtures/ytdlp/instagram-reel.json");
const POST: &[u8] = include_bytes!("../../tests/fixtures/ytdlp/instagram-post-mixed.json");

struct FakeResolver;
impl ManagedToolResolver for FakeResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixture".into(),
            path: PathBuf::from(r"C:\managed").join(match name {
                ToolName::YtDlp => "yt-dlp.exe",
                ToolName::Deno => "deno.exe",
                ToolName::GalleryDl => "gallery-dl.exe",
                ToolName::Ffmpeg => "ffmpeg.exe",
                ToolName::Ffprobe => "ffprobe.exe",
            }),
        })
    }
}

#[derive(Debug)]
struct Invocation {
    program: PathBuf,
    args: Vec<OsString>,
    timeout: Option<Duration>,
    stdout_limit: usize,
    stderr_limit: usize,
}
struct FakeProcess {
    responses: Mutex<VecDeque<Result<ProcessExit, ProcessFailureKind>>>,
    calls: Mutex<Vec<Invocation>>,
}
impl FakeProcess {
    fn output(bytes: &[u8]) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([Ok(ProcessExit {
                code: 0,
                stdout: bytes.to_vec(),
                stderr: vec![],
            })])),
            calls: Mutex::new(vec![]),
        }
    }
}
impl ProcessSupervisor for FakeProcess {
    fn run(
        &self,
        request: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let (stdout_limit, stderr_limit) = match request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => (stdout_limit, stderr_limit),
            _ => panic!("capture required"),
        };
        self.calls.lock().unwrap().push(Invocation {
            program: request.program.path().to_owned(),
            args: request.args,
            timeout: request.timeout,
            stdout_limit,
            stderr_limit,
        });
        self.responses.lock().unwrap().pop_front().unwrap()
    }
}

#[test]
fn reel_and_post_use_distinct_fixed_bounded_metadata_commands() {
    let reel_process = FakeProcess::output(REEL);
    let analyzer = YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &reel_process);
    analyzer
        .analyze_reel(
            &validate_media_url(REEL_URL).unwrap(),
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let reel = &reel_process.calls.lock().unwrap()[0];
    assert_eq!(reel.program, PathBuf::from(r"C:\managed\yt-dlp.exe"));
    let reel_args = reel
        .args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(reel_args.contains(&"--dump-single-json".into()));
    let profile = reel_args
        .iter()
        .position(|arg| arg == "--impersonate")
        .unwrap();
    assert_eq!(reel_args[profile + 1], "chrome-110");
    assert!(reel_args.contains(&"--no-playlist".into()));
    assert!(!reel_args.contains(&"--yes-playlist".into()));
    assert_eq!(reel_args.last().unwrap(), REEL_URL);
    assert_eq!(reel.timeout, Some(Duration::from_secs(45)));
    assert_eq!((reel.stdout_limit, reel.stderr_limit), (1_048_576, 65_536));

    let post_process = FakeProcess::output(POST);
    let analyzer = YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &post_process);
    analyzer
        .analyze_post_item(
            &validate_media_url(POST_URL).unwrap(),
            3,
            "1",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let post = &post_process.calls.lock().unwrap()[0];
    let args = post
        .args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(args.contains(&"--dump-json".into()));
    let profile = args.iter().position(|arg| arg == "--impersonate").unwrap();
    assert_eq!(args[profile + 1], "chrome-110");
    assert!(args.contains(&"--yes-playlist".into()));
    let at = args
        .iter()
        .position(|arg| arg == "--playlist-items")
        .unwrap();
    assert_eq!(args[at + 1], "3");
    assert_eq!(args.last().unwrap(), POST_URL);
    let joined = args.join(" ").to_ascii_lowercase();
    assert!(!joined.contains("cookie"));
    assert!(!joined.contains("proxy"));
    assert!(!joined.contains("--config"));
}

#[test]
fn maps_process_and_identity_failures_without_leaking_tool_diagnostics() {
    let source = validate_media_url(POST_URL).unwrap();
    for (response, expected) in [
        (
            Err(ProcessFailureKind::TimedOut),
            AnalysisErrorCode::AnalysisTimeout,
        ),
        (
            Ok(ProcessExit {
                code: 1,
                stdout: vec![],
                stderr: b"login required; use cookies".to_vec(),
            }),
            AnalysisErrorCode::AuthenticationRequired,
        ),
        (
            Ok(ProcessExit {
                code: 1,
                stdout: vec![],
                stderr: b"There is no video in this post".to_vec(),
            }),
            AnalysisErrorCode::ContentChanged,
        ),
    ] {
        let process = FakeProcess {
            responses: Mutex::new(VecDeque::from([response])),
            calls: Mutex::new(vec![]),
        };
        let analyzer = YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process);
        assert_eq!(
            analyzer
                .analyze_post_item(&source, 3, "1", false, &CancellationToken::new())
                .err(),
            Some(expected)
        );
    }
}

fn incomplete_post(video_codec: Option<&str>) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "extractor_key":"Instagram", "id":"B", "title":"Carousel video",
        "playlist_id":"Post_01", "playlist_index":3, "playlist_count":3,
        "formats":[{"format_id":"0", "ext":"mp4", "vcodec":video_codec,
            "acodec":null, "width":1080, "height":1920,
            "url":"https://scontent.cdninstagram.com/video.mp4?token=private"}]
    }))
    .unwrap()
}

const COMBINED_PROBE: &[u8] = br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1080,"height":1920,"avg_frame_rate":"30/1","color_transfer":"bt709","bit_rate":"4000000"},{"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2,"bit_rate":"128000"}],"format":{"duration":"8.25","size":"4200000"}}"#;

fn probe_process(metadata: Vec<u8>, probe: &[u8]) -> FakeProcess {
    FakeProcess {
        responses: Mutex::new(VecDeque::from([
            Ok(ProcessExit {
                code: 0,
                stdout: metadata,
                stderr: vec![],
            }),
            Ok(ProcessExit {
                code: 0,
                stdout: probe.to_vec(),
                stderr: vec![],
            }),
        ])),
        calls: Mutex::new(vec![]),
    }
}

#[test]
fn instagram_unknown_codecs_are_probed_instead_of_discarding_the_video_or_audio() {
    for video_codec in [None, Some("h264")] {
        let process = probe_process(incomplete_post(video_codec), COMBINED_PROBE);
        let resolved = YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process)
            .analyze_post_item(
                &validate_media_url(POST_URL).unwrap(),
                3,
                "1",
                false,
                &CancellationToken::new(),
            )
            .unwrap();
        let format = &resolved.media().formats[0];
        assert_eq!(
            format.kind,
            local_media_downloader_lib::domain::media::StreamKind::Combined
        );
        assert_eq!(format.video_codec.as_deref(), Some("h264"));
        assert_eq!(format.audio_codec.as_deref(), Some("aac"));
        assert_eq!(format.fps, Some(30.0));
        assert_eq!(
            format.file_size.as_ref().map(|size| size.bytes),
            Some(4200000)
        );
        assert_eq!(resolved.plan().output_container.as_deref(), Some("mp4"));
        assert_eq!(
            resolved.plan().video_processing,
            Some(local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy)
        );
        let calls = process.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let call = &calls[1];
        assert_eq!(call.program, PathBuf::from(r"C:\managed\ffprobe.exe"));
        assert!(call.timeout.unwrap() <= Duration::from_secs(15));
        let args = call
            .args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();
        let redirects = args.iter().position(|arg| arg == "-max_redirects").unwrap();
        assert_eq!(args[redirects + 1], "0");
        let formats = args
            .iter()
            .position(|arg| arg == "-format_whitelist")
            .unwrap();
        assert_eq!(args[formats + 1], "mov,matroska,webm");
        assert!(!serde_json::to_string(resolved.media())
            .unwrap()
            .contains("token=private"));
    }
}

#[test]
fn missing_high_quality_codec_is_not_hidden_by_a_known_lower_quality_format() {
    let mut value: serde_json::Value = serde_json::from_slice(&incomplete_post(None)).unwrap();
    value["formats"].as_array_mut().unwrap().push(serde_json::json!({
        "format_id":"low", "ext":"mp4", "vcodec":"h264", "acodec":"aac", "width":720, "height":1280
    }));
    let process = probe_process(serde_json::to_vec(&value).unwrap(), COMBINED_PROBE);
    let resolved = YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process)
        .analyze_post_item(
            &validate_media_url(POST_URL).unwrap(),
            3,
            "1",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(resolved.plan().video_format_id.as_deref(), Some("0"));
}

#[test]
fn unknown_codecs_never_allow_untrusted_probe_targets_or_wrong_post_identity() {
    for target in [
        "file:///C:/secret",
        "https://evil.test/video.mp4",
        "https://cdninstagram.com.evil.test/a",
        "https://user:pass@cdninstagram.com/a",
        "https://cdninstagram.com:8443/a",
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&incomplete_post(None)).unwrap();
        value["formats"][0]["url"] = serde_json::json!(target);
        let process = probe_process(serde_json::to_vec(&value).unwrap(), COMBINED_PROBE);
        let result = YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process).analyze_post_item(
            &validate_media_url(POST_URL).unwrap(),
            3,
            "1",
            false,
            &CancellationToken::new(),
        );
        assert_eq!(result.err(), Some(AnalysisErrorCode::InvalidMetadata));
        assert_eq!(process.calls.lock().unwrap().len(), 1);
    }
    let process = probe_process(incomplete_post(None), COMBINED_PROBE);
    assert_eq!(
        YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process)
            .analyze_post_item(
                &validate_media_url(POST_URL).unwrap(),
                3,
                "2",
                false,
                &CancellationToken::new()
            )
            .err(),
        Some(AnalysisErrorCode::ContentChanged)
    );
    assert_eq!(process.calls.lock().unwrap().len(), 1);
}

#[test]
fn unknown_codecs_do_not_probe_drm_or_live_metadata() {
    for (field, value) in [
        ("has_drm", serde_json::json!(true)),
        ("is_live", serde_json::json!(true)),
        ("_type", serde_json::json!("playlist")),
    ] {
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&incomplete_post(None)).unwrap();
        metadata[field] = value;
        let process = probe_process(serde_json::to_vec(&metadata).unwrap(), COMBINED_PROBE);
        assert_eq!(
            YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process)
                .analyze_post_item(
                    &validate_media_url(POST_URL).unwrap(),
                    3,
                    "1",
                    false,
                    &CancellationToken::new()
                )
                .err(),
            Some(AnalysisErrorCode::UnsupportedMedia)
        );
        assert_eq!(process.calls.lock().unwrap().len(), 1);
    }
}

#[test]
fn probe_failure_or_contradictory_streams_never_guess_or_drop_to_lower_quality() {
    let source = validate_media_url(POST_URL).unwrap();
    for (probe, expected) in [
        (b"not json".as_slice(), AnalysisErrorCode::InvalidMetadata),
        (br#"{"streams":[],"format":{}}"#.as_slice(), AnalysisErrorCode::InvalidMetadata),
        (br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":720,"height":1280}],"format":{}}"#.as_slice(), AnalysisErrorCode::ContentChanged),
        (br#"{"streams":[{"codec_type":"video","codec_name":"vp9","width":1080,"height":1920}],"format":{}}"#.as_slice(), AnalysisErrorCode::ContentChanged),
    ] {
        let process = probe_process(incomplete_post(Some("h264")), probe);
        assert_eq!(YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process).analyze_post_item(&source, 3, "1", false, &CancellationToken::new()).err(), Some(expected));
    }
    let process = probe_process(incomplete_post(None), COMBINED_PROBE);
    *process.responses.lock().unwrap().back_mut().unwrap() = Err(ProcessFailureKind::TimedOut);
    assert_eq!(
        YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process)
            .analyze_post_item(&source, 3, "1", false, &CancellationToken::new())
            .err(),
        Some(AnalysisErrorCode::AnalysisTimeout)
    );
}

#[test]
fn silent_video_is_proven_by_probe_instead_of_fabricating_an_audio_stream() {
    let probe = br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1080,"height":1920,"avg_frame_rate":"30/1"}],"format":{}}"#;
    let process = probe_process(incomplete_post(None), probe);
    let resolved = YtDlpInstagramVideoAnalyzer::new(&FakeResolver, &process)
        .analyze_post_item(
            &validate_media_url(POST_URL).unwrap(),
            3,
            "1",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(resolved.media().formats[0].audio_codec.is_none());
    assert_eq!(
        resolved.media().formats[0].kind,
        local_media_downloader_lib::domain::media::StreamKind::VideoOnly
    );
}
