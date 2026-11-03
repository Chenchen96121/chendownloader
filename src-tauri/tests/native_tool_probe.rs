use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use local_media_downloader_lib::{
    application::{
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_bootstrap::{ProbeOutput, ToolVersionProbe},
    },
    domain::managed_tools::{ManagedToolErrorCode, ToolName},
    infrastructure::tools::NativeToolVersionProbe,
};

#[derive(Debug, Eq, PartialEq)]
struct Invocation {
    program: PathBuf,
    args: Vec<OsString>,
    working_dir: PathBuf,
    timeout: Option<Duration>,
    inactivity_timeout: Option<Duration>,
    stdout_limit: usize,
    stderr_limit: usize,
    initially_canceled: bool,
}

struct FakeProcess {
    calls: Mutex<Vec<Invocation>>,
    response: Mutex<Option<Result<ProcessExit, ProcessFailureKind>>>,
    cancel_during_run: bool,
}

impl FakeProcess {
    fn successful() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            response: Mutex::new(Some(Ok(ProcessExit {
                code: 7,
                stdout: b"version stdout".to_vec(),
                stderr: b"version stderr".to_vec(),
            }))),
            cancel_during_run: false,
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
        let initially_canceled = cancellation.is_canceled();
        if self.cancel_during_run {
            cancellation.cancel();
        }
        let (stdout_limit, stderr_limit) = match request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => (stdout_limit, stderr_limit),
            OutputMode::Lines { .. } => panic!("version probes must use capture mode"),
        };
        self.calls.lock().unwrap().push(Invocation {
            program: request.program.path().to_owned(),
            args: request.args,
            working_dir: request.working_dir,
            timeout: request.timeout,
            inactivity_timeout: request.inactivity_timeout,
            stdout_limit,
            stderr_limit,
            initially_canceled,
        });
        self.response.lock().unwrap().take().unwrap_or_else(|| {
            Ok(ProcessExit {
                code: 0,
                stdout: b"second".to_vec(),
                stderr: Vec::new(),
            })
        })
    }
}

#[test]
fn probes_each_digest_verified_candidate_with_the_exact_bounded_request() {
    for tool in ToolName::required() {
        let expected_version_arg = match tool {
            ToolName::YtDlp | ToolName::Deno | ToolName::GalleryDl => "--version",
            ToolName::Ffmpeg | ToolName::Ffprobe => "-version",
        };
        let process = FakeProcess::successful();
        let probe = NativeToolVersionProbe::new(&process);
        let candidate = PathBuf::from(r"C:\staged").join(tool.artifact_name("target"));
        let output = probe.probe(tool, &candidate).unwrap();
        assert_eq!(
            output,
            ProbeOutput {
                exit_code: 7,
                stdout: "version stdout".to_owned(),
                stderr: "version stderr".to_owned(),
            }
        );
        assert_eq!(
            *process.calls.lock().unwrap(),
            [Invocation {
                program: candidate,
                args: vec![OsString::from(expected_version_arg)],
                working_dir: PathBuf::from(r"C:\staged"),
                timeout: Some(Duration::from_secs(10)),
                inactivity_timeout: None,
                stdout_limit: 4096,
                stderr_limit: 4096,
                initially_canceled: false,
            }]
        );
    }
}

#[test]
fn every_probe_gets_an_isolated_cancellation_token() {
    let process = FakeProcess {
        calls: Mutex::new(Vec::new()),
        response: Mutex::new(None),
        cancel_during_run: true,
    };
    let probe = NativeToolVersionProbe::new(&process);
    let candidate = Path::new(r"C:\staged\deno.exe");
    probe.probe(ToolName::Deno, candidate).unwrap();
    probe.probe(ToolName::Deno, candidate).unwrap();
    assert_eq!(
        process
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.initially_canceled)
            .collect::<Vec<_>>(),
        [false, false]
    );
}

#[test]
fn process_failures_become_redacted_probe_errors() {
    let process = FakeProcess {
        calls: Mutex::new(Vec::new()),
        response: Mutex::new(Some(Err(ProcessFailureKind::LaunchFailed))),
        cancel_during_run: false,
    };
    let probe = NativeToolVersionProbe::new(&process);
    let error = probe
        .probe(ToolName::YtDlp, Path::new(r"C:\private\yt-dlp.exe"))
        .unwrap_err();
    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
    assert!(!error.detail.contains("private"));
}

#[test]
fn invalid_utf8_is_projected_lossily_for_the_existing_strict_parser() {
    let process = FakeProcess {
        calls: Mutex::new(Vec::new()),
        response: Mutex::new(Some(Ok(ProcessExit {
            code: 0,
            stdout: vec![b'2', 0xff],
            stderr: vec![0xfe],
        }))),
        cancel_during_run: false,
    };
    let output = NativeToolVersionProbe::new(&process)
        .probe(ToolName::Deno, Path::new(r"C:\staged\deno.exe"))
        .unwrap();
    assert!(output.stdout.contains('\u{fffd}'));
    assert!(output.stderr.contains('\u{fffd}'));
}
