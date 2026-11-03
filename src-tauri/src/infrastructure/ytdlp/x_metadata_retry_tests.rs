use super::*;
use crate::{
    application::{
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        process::{CancellationToken, ProcessExit, ProcessFailureKind},
        tool_resolver::ResolvedTool,
        x_source::XVideoCandidate,
    },
    domain::managed_tools::ManagedToolError,
    infrastructure::x::inventory::parse_x_inventory,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

const TLS_EOF: &str = "ERROR: [twitter] Unable to download JSON metadata: EOF occurred in violation of protocol (_ssl.c:1007)";

struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixture".into(),
            path: PathBuf::from(if name == ToolName::YtDlp {
                "C:/managed/yt-dlp.exe"
            } else {
                "C:/managed/ffprobe.exe"
            }),
        })
    }
}

#[derive(Clone, Copy)]
enum Step {
    Failure(&'static str),
    DelayedFailure(&'static str, Duration),
    CancelFailure,
    ProcessFailure(ProcessFailureKind),
    InvalidJson,
    Success,
}

struct ScriptedProcess {
    steps: Vec<Step>,
    requests: Mutex<Vec<ProcessRequest>>,
    network: Arc<NetworkProfile>,
}
impl ScriptedProcess {
    fn new(steps: Vec<Step>, context: &XRequestContext) -> Self {
        Self {
            steps,
            requests: Mutex::new(vec![]),
            network: context.network.clone(),
        }
    }
}
impl ProcessSupervisor for ScriptedProcess {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        // The test exercises real matching/selection. Only the external process
        // is scripted; accidental direct-source fallback cannot pass as recovery.
        if !request.program.path().ends_with("yt-dlp.exe") {
            return Err(ProcessFailureKind::LaunchFailed);
        }
        assert!(
            std::ptr::eq(
                request.program.network_profile().unwrap(),
                self.network.as_ref()
            ),
            "every extraction must use the original network snapshot"
        );
        let index = {
            let mut requests = self.requests.lock().unwrap();
            let index = requests.len();
            requests.push(request);
            index
        };
        let failure = |stderr: &str| {
            Ok(ProcessExit {
                code: 1,
                stdout: vec![],
                stderr: stderr.as_bytes().to_vec(),
            })
        };
        match *self
            .steps
            .get(index)
            .expect("unexpected extra metadata extraction")
        {
            Step::Failure(stderr) => failure(stderr),
            Step::DelayedFailure(stderr, delay) => {
                std::thread::sleep(delay);
                failure(stderr)
            }
            Step::CancelFailure => {
                cancellation.cancel();
                failure(TLS_EOF)
            }
            Step::ProcessFailure(error) => Err(error),
            Step::InvalidJson => Ok(ProcessExit {
                code: 0,
                stdout: b"{".to_vec(),
                stderr: vec![],
            }),
            Step::Success => Ok(ProcessExit {
                code: 0,
                stdout: include_bytes!("../../../../fixtures/x/video-formats.json").to_vec(),
                stderr: vec![],
            }),
        }
    }
}

fn inventory() -> XInventory {
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    parse_x_inventory(
        include_bytes!("../../../../fixtures/x/mixed-post.json"),
        &source,
    )
    .unwrap()
}
fn hls_inventory() -> XInventory {
    let mut inventory = inventory();
    inventory.items[1].video_candidates.push(XVideoCandidate {
        url: "https://video.twimg.com/ext_tw_video/fixtureB/master.m3u8"
            .parse()
            .unwrap(),
        content_type: "application/x-mpegURL".into(),
        bitrate: None,
    });
    inventory
}
fn context() -> XRequestContext {
    XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}
fn analyze(
    process: &ScriptedProcess,
    inventory: &XInventory,
    context: &XRequestContext,
) -> Result<ResolvedXVideo, AnalysisErrorCode> {
    YtDlpXVideoAnalyzer::new(Resolver, process).analyze_item(
        inventory,
        inventory.items[1].identity.as_ref().unwrap(),
        false,
        context,
    )
}

#[test]
fn x_metadata_tls_eof_and_connection_reset_are_network_failures() {
    for stderr in [
        TLS_EOF,
        "ERROR: [SSL: UNEXPECTED_EOF_WHILE_READING] EOF occurred in violation of protocol",
        "ERROR: Unable to download JSON metadata: Connection reset by peer",
        "ERROR: ConnectionResetError(10054, 'An existing connection was forcibly closed by the remote host')",
    ] {
        assert_eq!(super::super::errors::classify_nonzero_exit(stderr.as_bytes()),
            AnalysisErrorCode::NetworkFailed, "{stderr}");
    }
}

#[test]
fn x_metadata_retries_tls_eof_once_with_identical_guards_and_preserves_highest_format() {
    for stderr in [
        TLS_EOF,
        "ERROR: Unable to download JSON metadata: Connection reset by peer",
    ] {
        let context = context();
        let process = ScriptedProcess::new(vec![Step::Failure(stderr), Step::Success], &context);
        let resolved = analyze(&process, &inventory(), &context)
            .unwrap_or_else(|error| panic!("metadata recovery failed: {error:?}"));
        assert_eq!(resolved.plan().video_format_id.as_deref(), Some("high"));
        assert_eq!(resolved.plan().audio_format_id.as_deref(), Some("high"));
        assert_eq!(resolved.media().formats.len(), 2);
        assert!(Arc::ptr_eq(&resolved.source().network(), &context.network));
        let requests = process.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].args, requests[1].args);
        assert_eq!(requests[0].program.path(), requests[1].program.path());
        assert_eq!(requests[0].working_dir, requests[1].working_dir);
        for request in requests.iter() {
            assert!(request
                .timeout
                .is_some_and(|timeout| !timeout.is_zero() && timeout <= Duration::from_secs(45)));
            assert!(request.inactivity_timeout.is_none());
            assert!(matches!(
                request.output_mode,
                OutputMode::Capture {
                    stdout_limit: 16_777_216,
                    stderr_limit: 65_536
                }
            ));
            let args = request
                .args
                .iter()
                .map(|arg| arg.to_str().unwrap())
                .collect::<Vec<_>>();
            for guard in [
                "--ignore-config",
                "--no-plugin-dirs",
                "--no-cookies",
                "--no-cookies-from-browser",
                "--simulate",
                "--dump-single-json",
            ] {
                assert!(args.contains(&guard), "missing {guard}");
            }
            assert!(args.windows(2).any(|pair| pair == ["--retries", "0"]));
            assert!(args
                .windows(2)
                .any(|pair| pair == ["--extractor-retries", "0"]));
            assert_eq!(
                args.last(),
                Some(&"https://x.com/i/web/status/12345678901234567890")
            );
        }
    }
}

#[test]
fn x_metadata_stops_after_one_transient_retry_without_dropping_hls() {
    let context = context();
    let process = ScriptedProcess::new(
        vec![
            Step::Failure(TLS_EOF),
            Step::Failure(TLS_EOF),
            Step::Success,
        ],
        &context,
    );
    assert!(matches!(
        analyze(&process, &hls_inventory(), &context),
        Err(AnalysisErrorCode::NetworkFailed)
    ));
    assert_eq!(process.requests.lock().unwrap().len(), 2);
}

#[test]
fn x_metadata_does_not_retry_unknown_certificate_authentication_rate_or_invalid_json() {
    for (step, expected) in [
        (
            Step::Failure("ERROR: unrelated extractor failure"),
            AnalysisErrorCode::Unknown,
        ),
        (
            Step::Failure("ERROR: Unexpected EOF while parsing JSON"),
            AnalysisErrorCode::Unknown,
        ),
        (
            Step::Failure("ERROR: certificate_verify_failed"),
            AnalysisErrorCode::NetworkFailed,
        ),
        (
            Step::Failure(
                "ERROR: certificate_verify_failed; EOF occurred in violation of protocol",
            ),
            AnalysisErrorCode::NetworkFailed,
        ),
        (
            Step::Failure("ERROR: Authentication required; EOF occurred in violation of protocol"),
            AnalysisErrorCode::AuthenticationRequired,
        ),
        (
            Step::Failure("ERROR: HTTP error 429 Too many requests; Connection reset by peer"),
            AnalysisErrorCode::RateLimited,
        ),
        (
            Step::Failure("ERROR: HTTP error 403 Forbidden; Connection reset by peer"),
            AnalysisErrorCode::AccessDenied,
        ),
        (
            Step::ProcessFailure(ProcessFailureKind::TimedOut),
            AnalysisErrorCode::AnalysisTimeout,
        ),
        (
            Step::ProcessFailure(ProcessFailureKind::LaunchFailed),
            AnalysisErrorCode::ProcessFailed,
        ),
        (Step::InvalidJson, AnalysisErrorCode::InvalidMetadata),
    ] {
        let context = context();
        let process = ScriptedProcess::new(vec![step, Step::Success], &context);
        assert!(
            matches!(analyze(&process, &hls_inventory(), &context), Err(error) if error == expected)
        );
        assert_eq!(
            process.requests.lock().unwrap().len(),
            1,
            "must not retry {expected:?}"
        );
    }
}

#[test]
fn x_metadata_retry_keeps_the_original_forty_five_second_extraction_deadline() {
    let context = context();
    let process = ScriptedProcess::new(
        vec![
            Step::DelayedFailure(TLS_EOF, Duration::from_millis(80)),
            Step::Success,
        ],
        &context,
    );
    assert!(analyze(&process, &inventory(), &context).is_ok());
    let requests = process.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].timeout.unwrap() <= requests[0].timeout.unwrap() - Duration::from_millis(50),
        "elapsed attempt time must reduce the original extraction budget"
    );
}

#[test]
fn x_metadata_expired_original_deadline_prevents_a_second_extraction() {
    let mut context = context();
    context.deadline = Instant::now() + Duration::from_millis(40);
    let process = ScriptedProcess::new(
        vec![
            Step::DelayedFailure(TLS_EOF, Duration::from_millis(60)),
            Step::Success,
        ],
        &context,
    );
    assert!(matches!(
        analyze(&process, &hls_inventory(), &context),
        Err(AnalysisErrorCode::AnalysisTimeout)
    ));
    assert_eq!(process.requests.lock().unwrap().len(), 1);
}

#[test]
fn x_metadata_cancellation_prevents_initial_and_retry_extraction() {
    let ctx = context();
    let process = ScriptedProcess::new(vec![Step::Success], &ctx);
    ctx.cancellation.cancel();
    assert!(matches!(
        analyze(&process, &inventory(), &ctx),
        Err(AnalysisErrorCode::ProcessFailed)
    ));
    assert!(process.requests.lock().unwrap().is_empty());

    let ctx = context();
    let process = ScriptedProcess::new(vec![Step::CancelFailure, Step::Success], &ctx);
    assert!(matches!(
        analyze(&process, &hls_inventory(), &ctx),
        Err(AnalysisErrorCode::ProcessFailed)
    ));
    assert_eq!(process.requests.lock().unwrap().len(), 1);
}
