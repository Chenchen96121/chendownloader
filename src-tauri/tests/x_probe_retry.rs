use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
        x_source::{XInventory, XRequestContext, XVideoCandidate},
        x_video::{ResolvedXVideo, XVideoAnalyzer},
    },
    domain::{
        managed_tools::{ManagedToolError, ToolName},
        media::AnalysisErrorCode,
    },
    infrastructure::{x::inventory::parse_x_inventory, ytdlp::x::YtDlpXVideoAnalyzer},
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const HIGH_INPUT: &str = "https://video.twimg.com/ext_tw_video/fixtureB/high.mp4";

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

#[derive(Clone, Copy, Debug)]
enum ProbeStep {
    Success,
    Timeout,
    NetworkFailure,
    TlsProxyPullFailure,
    TlsProxyAccessDenied,
    CancelDuringTimeout,
    CanceledProcess,
    AccessDenied,
    AuthenticationRequired,
    InvalidJson,
    ChangedDimensions,
    ExpireDeadline,
}

struct ScriptedProcess {
    metadata: Value,
    steps: Vec<ProbeStep>,
    probes: Mutex<Vec<ProcessRequest>>,
    extractions: AtomicUsize,
    deadline: Option<Instant>,
}
impl ScriptedProcess {
    fn new(steps: Vec<ProbeStep>) -> Self {
        let mut metadata: Value =
            serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json")).unwrap();
        metadata["entries"][1]["formats"][1]["vcodec"] = Value::Null;
        Self {
            metadata,
            steps,
            probes: Mutex::new(vec![]),
            extractions: AtomicUsize::new(0),
            deadline: None,
        }
    }
}
fn success(value: Value) -> Result<ProcessExit, ProcessFailureKind> {
    Ok(ProcessExit {
        code: 0,
        stdout: serde_json::to_vec(&value).unwrap(),
        stderr: vec![],
    })
}
fn probe_value() -> Value {
    json!({"programs":[],"stream_groups":[],"streams":[
        {"codec_type":"video","codec_name":"vp9","width":1920,"height":1080,
            "avg_frame_rate":"60/1","color_transfer":"smpte2084","bit_rate":"8000000"},
        {"codec_type":"audio","codec_name":"opus","bit_rate":"160000",
            "sample_rate":"48000","channels":2}],
        "format":{"duration":"12","size":"12240000"}})
}
impl ProcessSupervisor for ScriptedProcess {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        if request.program.path().ends_with("yt-dlp.exe") {
            self.extractions.fetch_add(1, Ordering::SeqCst);
            return success(self.metadata.clone());
        }
        assert!(request.program.path().ends_with("ffprobe.exe"));
        let index = {
            let mut probes = self.probes.lock().unwrap();
            let index = probes.len();
            probes.push(request);
            index
        };
        let failure = |stderr: &str| {
            Ok(ProcessExit {
                code: 1,
                stdout: vec![],
                stderr: stderr.as_bytes().to_vec(),
            })
        };
        match self.steps.get(index).copied().unwrap_or(ProbeStep::Success) {
            ProbeStep::Success => success(probe_value()),
            ProbeStep::Timeout => Err(ProcessFailureKind::TimedOut),
            ProbeStep::NetworkFailure => failure("Connection refused"),
            ProbeStep::TlsProxyPullFailure => failure("[httpproxy @ 0000000000000000] Stream ends prematurely at 0, should be 18446744073709551615\n[tls @ 0000000000000000] Error in the pull function.\n[tls @ 0000000000000000] IO error: I/O error\nhttps://video.twimg.com/ext_tw_video/fixtureB/high.mp4: I/O error"),
            ProbeStep::TlsProxyAccessDenied => failure("HTTP error 403 Forbidden\n[tls @ 0000000000000000] Error in the pull function.\n[tls @ 0000000000000000] IO error: I/O error"),
            ProbeStep::CancelDuringTimeout => {
                cancellation.cancel();
                Err(ProcessFailureKind::TimedOut)
            }
            ProbeStep::CanceledProcess => Err(ProcessFailureKind::Canceled),
            ProbeStep::AccessDenied => failure("HTTP error 403 Forbidden"),
            ProbeStep::AuthenticationRequired => failure("Authentication required"),
            ProbeStep::InvalidJson => Ok(ProcessExit {
                code: 0,
                stdout: b"{".to_vec(),
                stderr: vec![],
            }),
            ProbeStep::ChangedDimensions => {
                let mut value = probe_value();
                value["streams"][0]["width"] = json!(1280);
                success(value)
            }
            ProbeStep::ExpireDeadline => {
                let remaining = self
                    .deadline
                    .unwrap()
                    .checked_duration_since(Instant::now())
                    .unwrap_or_default();
                std::thread::sleep(remaining + Duration::from_millis(1));
                Err(ProcessFailureKind::TimedOut)
            }
        }
    }
}
fn inventory() -> XInventory {
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), &source).unwrap()
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
fn x_progressive_transient_probe_retries_the_same_guarded_input_without_downgrading() {
    for transient in [
        ProbeStep::Timeout,
        ProbeStep::NetworkFailure,
        ProbeStep::TlsProxyPullFailure,
    ] {
        let process = ScriptedProcess::new(vec![transient, ProbeStep::Success]);
        let ctx = context();
        let resolved = analyze(&process, &inventory(), &ctx)
            .unwrap_or_else(|error| panic!("{transient:?}: {error:?}"));
        assert_eq!(resolved.plan().video_format_id.as_deref(), Some("high"));
        assert_eq!(resolved.plan().audio_format_id.as_deref(), Some("high"));
        assert_eq!(resolved.media().formats.len(), 2);
        let selected = resolved
            .media()
            .formats
            .iter()
            .find(|row| row.id == "high")
            .unwrap();
        assert_eq!(
            (selected.width, selected.height, selected.fps),
            (Some(1920), Some(1080), Some(60.0))
        );
        let probes = process.probes.lock().unwrap();
        assert_eq!(probes.len(), 2, "one retry for {transient:?}");
        assert_eq!(
            probes[0].args, probes[1].args,
            "retry must preserve input and guards"
        );
        assert_eq!(probes[0].program.path(), probes[1].program.path());
        assert_eq!(probes[0].working_dir, probes[1].working_dir);
        for probe in probes.iter() {
            let args = probe
                .args
                .iter()
                .map(|arg| arg.to_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(args.last(), Some(&HIGH_INPUT));
            for pair in [
                ["-protocol_whitelist", "https,http,tcp,tls,httpproxy"],
                ["-format_whitelist", "mov,matroska,webm"],
                ["-max_redirects", "0"],
                ["-rw_timeout", "8000000"],
                ["-referer", "https://x.com/"],
            ] {
                assert!(
                    args.windows(2).any(|actual| actual == pair),
                    "missing guard {pair:?}"
                );
            }
            assert!(probe
                .timeout
                .is_some_and(|timeout| !timeout.is_zero() && timeout <= Duration::from_secs(15)));
        }
        assert_eq!(process.extractions.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn x_progressive_probe_stops_after_one_retry_and_returns_the_transient_error() {
    for (step, expected) in [
        (ProbeStep::Timeout, AnalysisErrorCode::AnalysisTimeout),
        (ProbeStep::NetworkFailure, AnalysisErrorCode::NetworkFailed),
        (
            ProbeStep::TlsProxyPullFailure,
            AnalysisErrorCode::NetworkFailed,
        ),
    ] {
        let process = ScriptedProcess::new(vec![step, step, ProbeStep::Success]);
        assert!(
            matches!(analyze(&process, &inventory(), &context()), Err(error) if error == expected)
        );
        let probes = process.probes.lock().unwrap();
        assert_eq!(
            probes.len(),
            2,
            "must not make a third attempt after {step:?}"
        );
        assert_eq!(probes[0].args, probes[1].args);
    }
}

#[test]
fn x_progressive_probe_does_not_retry_authentication_metadata_or_content_failures() {
    for (step, expected) in [
        (ProbeStep::AccessDenied, AnalysisErrorCode::AccessDenied),
        (
            ProbeStep::TlsProxyAccessDenied,
            AnalysisErrorCode::AccessDenied,
        ),
        (
            ProbeStep::AuthenticationRequired,
            AnalysisErrorCode::AuthenticationRequired,
        ),
        (ProbeStep::InvalidJson, AnalysisErrorCode::InvalidMetadata),
        (
            ProbeStep::ChangedDimensions,
            AnalysisErrorCode::ContentChanged,
        ),
        (ProbeStep::CanceledProcess, AnalysisErrorCode::ProcessFailed),
    ] {
        let process = ScriptedProcess::new(vec![step, ProbeStep::Success]);
        assert!(
            matches!(analyze(&process, &inventory(), &context()), Err(error) if error == expected),
            "{step:?}"
        );
        assert_eq!(
            process.probes.lock().unwrap().len(),
            1,
            "must not retry {step:?}"
        );
    }
}

#[test]
fn x_progressive_probe_cancellation_prevents_initial_and_retry_attempts() {
    let process = ScriptedProcess::new(vec![ProbeStep::Success]);
    let ctx = context();
    ctx.cancellation.cancel();
    assert!(matches!(
        analyze(&process, &inventory(), &ctx),
        Err(AnalysisErrorCode::ProcessFailed)
    ));
    assert_eq!(process.extractions.load(Ordering::SeqCst), 0);
    assert!(process.probes.lock().unwrap().is_empty());

    let process = ScriptedProcess::new(vec![ProbeStep::CancelDuringTimeout, ProbeStep::Success]);
    let ctx = context();
    assert!(matches!(
        analyze(&process, &inventory(), &ctx),
        Err(AnalysisErrorCode::ProcessFailed)
    ));
    assert!(ctx.cancellation.is_canceled());
    assert_eq!(process.probes.lock().unwrap().len(), 1);
}

#[test]
fn x_progressive_probe_retry_does_not_restart_the_request_deadline() {
    let mut ctx = context();
    ctx.deadline = Instant::now() + Duration::from_millis(250);
    let mut process = ScriptedProcess::new(vec![ProbeStep::ExpireDeadline, ProbeStep::Success]);
    process.deadline = Some(ctx.deadline);
    assert!(matches!(
        analyze(&process, &inventory(), &ctx),
        Err(AnalysisErrorCode::AnalysisTimeout)
    ));
    let probes = process.probes.lock().unwrap();
    assert_eq!(
        probes.len(),
        1,
        "an expired original deadline cannot grant a fresh retry"
    );
    assert!(probes[0]
        .timeout
        .is_some_and(|timeout| timeout <= Duration::from_millis(250)));
}

#[test]
fn x_progressive_probe_retry_consumes_the_shared_sixteen_attempt_budget() {
    let mut inventory = inventory();
    let mut process = ScriptedProcess::new(vec![ProbeStep::Timeout, ProbeStep::Success]);
    let template = process.metadata["entries"][1]["formats"][1].clone();
    let formats = (0..16)
        .map(|index| {
            let mut format = template.clone();
            format["format_id"] = json!(format!("candidate-{index}"));
            format["url"] = json!(format!(
                "https://video.twimg.com/ext_tw_video/fixtureB/budget-{index}.mp4"
            ));
            format
        })
        .collect::<Vec<_>>();
    inventory.items[1].video_candidates = formats
        .iter()
        .map(|format| XVideoCandidate {
            url: format["url"].as_str().unwrap().parse().unwrap(),
            content_type: "video/mp4".into(),
            bitrate: None,
        })
        .collect();
    process.metadata["entries"][1]["formats"] = json!(formats);
    assert!(matches!(
        analyze(&process, &inventory, &context()),
        Err(AnalysisErrorCode::FormatUnavailable)
    ));
    let probes = process.probes.lock().unwrap();
    assert_eq!(
        probes.len(),
        16,
        "retry is part of the cap, never a seventeenth attempt"
    );
    assert_eq!(probes[0].args, probes[1].args);
    let inputs = probes
        .iter()
        .map(|request| request.args.last().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(
        inputs.len(),
        15,
        "no lower-quality success can hide the unproven final candidate"
    );
}
