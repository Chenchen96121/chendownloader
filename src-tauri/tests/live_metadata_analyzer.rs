use std::{collections::VecDeque, ffi::OsString, path::PathBuf, sync::Mutex, time::Duration};

use local_media_downloader_lib::{
    application::{
        analyze_media::{AnalyzeMediaService, MetadataAnalyzer},
        media_url::validate_media_url,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
        media::{AnalysisErrorCode, Platform},
    },
    infrastructure::ytdlp::YtDlpMetadataAnalyzer,
};

const INPUT: &str = "https://youtube.com/watch?v=demoYT00001";

fn valid_json(id: &str) -> Vec<u8> {
    format!(
        r#"{{"id":"{id}","title":"Live example","formats":[{{"format_id":"18","ext":"mp4","vcodec":"avc1.64001f","acodec":"mp4a.40.2","width":1280,"height":720}}]}}"#
    )
    .into_bytes()
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

    fn failing(tool: ToolName) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_on: Some(tool),
        }
    }
}

impl ManagedToolResolver for FakeResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        self.calls.lock().unwrap().push(name);
        if self.fail_on == Some(name) {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::ToolIntegrityFailed,
                "private resolver detail",
            ));
        }
        let (version, file) = match name {
            ToolName::YtDlp => ("2026.09.15", "yt-dlp.exe"),
            ToolName::Deno => ("2.5.0", "deno.exe"),
            ToolName::GalleryDl => ("1.32.15", "gallery-dl.exe"),
            ToolName::Ffmpeg => ("8.0.1", "ffmpeg.exe"),
            ToolName::Ffprobe => ("8.0.1", "ffprobe.exe"),
        };
        Ok(ResolvedTool {
            name,
            version: version.to_owned(),
            path: PathBuf::from(r"C:\managed").join(file),
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Invocation {
    program: PathBuf,
    args: Vec<OsString>,
    working_dir: PathBuf,
    timeout: Option<Duration>,
    inactivity_timeout: Option<Duration>,
    stdout_limit: usize,
    stderr_limit: usize,
    canceled: bool,
}

struct FakeProcess {
    responses: Mutex<VecDeque<Result<ProcessExit, ProcessFailureKind>>>,
    calls: Mutex<Vec<Invocation>>,
}

impl FakeProcess {
    fn returning(response: Result<ProcessExit, ProcessFailureKind>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([response])),
            calls: Mutex::new(Vec::new()),
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
        let (stdout_limit, stderr_limit) = match request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => (stdout_limit, stderr_limit),
            OutputMode::Lines { .. } => panic!("analysis must use capture mode"),
        };
        self.calls.lock().unwrap().push(Invocation {
            program: request.program.path().to_owned(),
            args: request.args,
            working_dir: request.working_dir,
            timeout: request.timeout,
            inactivity_timeout: request.inactivity_timeout,
            stdout_limit,
            stderr_limit,
            canceled: cancellation.is_canceled(),
        });
        self.responses.lock().unwrap().pop_front().unwrap()
    }
}

fn exit(code: i32, stdout: Vec<u8>, stderr: &[u8]) -> Result<ProcessExit, ProcessFailureKind> {
    Ok(ProcessExit {
        code,
        stdout,
        stderr: stderr.to_vec(),
    })
}

#[test]
fn invokes_verified_yt_dlp_with_only_the_fixed_safe_argument_set() {
    let resolver = FakeResolver::ready();
    let process = FakeProcess::returning(exit(0, valid_json("demoYT00001"), b""));
    let analyzer = YtDlpMetadataAnalyzer::new(&resolver, &process);
    let media = AnalyzeMediaService::new(&analyzer).analyze(INPUT).unwrap();
    assert_eq!(media.platform, Platform::Youtube);
    assert_eq!(media.media_id, "demoYT00001");

    assert_eq!(
        *resolver.calls.lock().unwrap(),
        [ToolName::YtDlp, ToolName::Deno]
    );
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0],
        Invocation {
            program: PathBuf::from(r"C:\managed\yt-dlp.exe"),
            args: [
                "--ignore-config".into(),
                "--no-remote-components".into(),
                "--no-plugin-dirs".into(),
                "--js-runtimes".into(),
                r"deno:C:\managed\deno.exe".into(),
                "--simulate".into(),
                "--dump-single-json".into(),
                "--no-playlist".into(),
                "--no-warnings".into(),
                "--".into(),
                INPUT.into(),
            ]
            .into(),
            working_dir: PathBuf::from(r"C:\managed"),
            timeout: Some(Duration::from_secs(45)),
            inactivity_timeout: None,
            stdout_limit: 8 * 1024 * 1024,
            stderr_limit: 65_536,
            canceled: false,
        }
    );
}

#[test]
fn public_video_metadata_larger_than_one_megabyte_keeps_its_formats() {
    let mut response: serde_json::Value =
        serde_json::from_slice(&valid_json("demoYT00001")).unwrap();
    response["description"] = "x".repeat(1_200_000).into();
    let resolver = FakeResolver::ready();
    let process = FakeProcess::returning(exit(0, serde_json::to_vec(&response).unwrap(), b""));
    let analyzer = YtDlpMetadataAnalyzer::new(&resolver, &process);
    let media = AnalyzeMediaService::new(&analyzer).analyze(INPUT).unwrap();
    assert_eq!(media.formats.len(), 1);
    assert_eq!(media.formats[0].width, Some(1280));
    assert!(!serde_json::to_string(&media)
        .unwrap()
        .contains("description"));
}

#[test]
fn classifies_nonzero_exit_without_returning_diagnostics() {
    for (stderr, expected) in [
        (
            "ERROR: Sign in to confirm you’re not a bot",
            AnalysisErrorCode::AuthenticationRequired,
        ),
        (
            "ERROR: HTTP Error 403: Forbidden",
            AnalysisErrorCode::AccessDenied,
        ),
        (
            "ERROR: Video unavailable. This video is private",
            AnalysisErrorCode::ContentUnavailable,
        ),
        (
            "ERROR: [youtube] demoYT00001: This video is unavailable",
            AnalysisErrorCode::ContentUnavailable,
        ),
        (
            "ERROR: HTTP Error 429: Too Many Requests",
            AnalysisErrorCode::RateLimited,
        ),
        (
            "ERROR: Please update yt-dlp to a newer version",
            AnalysisErrorCode::ExtractorOutdated,
        ),
        (
            "ERROR: Unable to download webpage: SSL CERTIFICATE_VERIFY_FAILED",
            AnalysisErrorCode::NetworkFailed,
        ),
        (
            "private signed URL https://secret.invalid/token",
            AnalysisErrorCode::Unknown,
        ),
    ] {
        let resolver = FakeResolver::ready();
        let process = FakeProcess::returning(exit(1, Vec::new(), stderr.as_bytes()));
        let analyzer = YtDlpMetadataAnalyzer::new(&resolver, &process);
        let error = analyzer
            .analyze(&validate_media_url(INPUT).unwrap())
            .unwrap_err();
        assert_eq!(error, expected, "{stderr}");
    }
}

#[test]
fn rejects_malformed_oversized_and_wrong_identity_responses() {
    let mut oversized: serde_json::Value =
        serde_json::from_slice(&valid_json("demoYT00001")).unwrap();
    oversized["description"] = "x".repeat(8 * 1024 * 1024).into();
    for stdout in [
        serde_json::to_vec(&oversized).unwrap(),
        vec![0xff, 0xfe],
        [valid_json("demoYT00001"), valid_json("demoYT00001")].concat(),
    ] {
        let resolver = FakeResolver::ready();
        let process = FakeProcess::returning(exit(0, stdout, b""));
        let analyzer = YtDlpMetadataAnalyzer::new(&resolver, &process);
        assert_eq!(
            analyzer.analyze(&validate_media_url(INPUT).unwrap()),
            Err(AnalysisErrorCode::InvalidMetadata)
        );
    }

    let resolver = FakeResolver::ready();
    let process = FakeProcess::returning(exit(0, valid_json("wrongYT0001"), b""));
    let analyzer = YtDlpMetadataAnalyzer::new(&resolver, &process);
    assert_eq!(
        AnalyzeMediaService::new(&analyzer).analyze(INPUT),
        Err(AnalysisErrorCode::InvalidMetadata)
    );
}

#[test]
fn maps_process_failures_and_re_resolves_tools_before_every_run() {
    for (failure, expected) in [
        (
            ProcessFailureKind::TimedOut,
            AnalysisErrorCode::AnalysisTimeout,
        ),
        (
            ProcessFailureKind::InactivityTimedOut,
            AnalysisErrorCode::AnalysisTimeout,
        ),
        (
            ProcessFailureKind::StdoutLimitExceeded,
            AnalysisErrorCode::InvalidMetadata,
        ),
        (
            ProcessFailureKind::LaunchFailed,
            AnalysisErrorCode::ProcessFailed,
        ),
    ] {
        let resolver = FakeResolver::ready();
        let process = FakeProcess::returning(Err(failure));
        let analyzer = YtDlpMetadataAnalyzer::new(&resolver, &process);
        assert_eq!(
            analyzer.analyze(&validate_media_url(INPUT).unwrap()),
            Err(expected)
        );
    }

    let resolver = FakeResolver::failing(ToolName::Deno);
    let process = FakeProcess {
        responses: Mutex::new(VecDeque::new()),
        calls: Mutex::new(Vec::new()),
    };
    let analyzer = YtDlpMetadataAnalyzer::new(&resolver, &process);
    assert_eq!(
        analyzer.analyze(&validate_media_url(INPUT).unwrap()),
        Err(AnalysisErrorCode::ToolIntegrityFailed)
    );
    assert_eq!(
        *resolver.calls.lock().unwrap(),
        [ToolName::YtDlp, ToolName::Deno]
    );
    assert!(process.calls.lock().unwrap().is_empty());
}
