#![cfg(windows)]

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
    thread,
    time::{Duration, Instant},
};

use local_media_downloader_lib::{
    application::process::{
        CancellationToken, OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram,
        ProcessRequest, ProcessStream, ProcessSupervisor,
    },
    infrastructure::process::WindowsProcessSupervisor,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, STILL_ACTIVE},
    System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
};

struct NoopSink;

impl ProcessEventSink for NoopSink {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
}

#[derive(Default)]
struct RecordingSink {
    lines: Mutex<Vec<(ProcessStream, Vec<u8>)>>,
}

impl ProcessEventSink for RecordingSink {
    fn on_line(&self, stream: ProcessStream, line: &[u8]) {
        self.lines.lock().unwrap().push((stream, line.to_vec()));
    }
}

fn fixture_program() -> ProcessProgram {
    ProcessProgram::for_test_fixture(PathBuf::from(env!("CARGO_BIN_EXE_process-fixture")))
}

fn capture_request(working_dir: &Path, args: &[&str]) -> ProcessRequest {
    ProcessRequest {
        program: fixture_program(),
        args: args.iter().map(OsString::from).collect(),
        working_dir: working_dir.to_owned(),
        timeout: Some(Duration::from_secs(5)),
        inactivity_timeout: None,
        output_mode: OutputMode::Capture {
            stdout_limit: 4096,
            stderr_limit: 4096,
        },
    }
}

fn run_capture(args: &[&str]) -> local_media_downloader_lib::application::process::ProcessExit {
    let working_dir = tempfile::tempdir().unwrap();
    WindowsProcessSupervisor
        .run(
            capture_request(working_dir.path(), args),
            CancellationToken::new(),
            &NoopSink,
        )
        .unwrap()
}

#[test]
fn preserves_unicode_arguments_without_a_shell() {
    let exit = run_capture(&["echo-args", "路径 🚀", "space kept", "\"quoted\""]);
    let actual: Vec<String> = serde_json::from_slice(&exit.stdout).unwrap();
    assert_eq!(actual, ["路径 🚀", "space kept", "\"quoted\""]);
    assert_eq!(exit.code, 0);
    assert!(exit.stderr.is_empty());
}

#[test]
fn clears_stdin_and_inherits_only_the_environment_allowlist() {
    let stdin = run_capture(&["stdin-length"]);
    assert_eq!(stdin.stdout, b"0");

    let environment = run_capture(&["environment", "CARGO_MANIFEST_DIR", "SystemRoot"]);
    assert_eq!(
        String::from_utf8(environment.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["CARGO_MANIFEST_DIR=0", "SystemRoot=1"]
    );
}

#[test]
fn captures_both_streams_and_preserves_invalid_utf8_as_bytes() {
    let captured = run_capture(&["emit", "4", "3"]);
    assert_eq!(captured.stdout, b"OOOO");
    assert_eq!(captured.stderr, b"EEE");

    let invalid = run_capture(&["invalid-utf8"]);
    assert_eq!(invalid.stdout, [0xff, 0x00, 0xfe]);
    assert_eq!(invalid.stderr, [0x80, b'E']);
}

#[test]
fn streams_complete_lines_and_retains_bounded_stderr() {
    let working_dir = tempfile::tempdir().unwrap();
    let sink = RecordingSink::default();
    let exit = WindowsProcessSupervisor
        .run(
            ProcessRequest {
                program: fixture_program(),
                args: ["lines"].into_iter().map(OsString::from).collect(),
                working_dir: working_dir.path().to_owned(),
                timeout: Some(Duration::from_secs(5)),
                inactivity_timeout: Some(Duration::from_secs(1)),
                output_mode: OutputMode::Lines {
                    line_limit: 64,
                    retained_stderr_limit: 64,
                },
            },
            CancellationToken::new(),
            &sink,
        )
        .unwrap();

    let lines = sink.lines.into_inner().unwrap();
    assert!(lines.contains(&(ProcessStream::Stdout, b"alpha".to_vec())));
    assert!(lines.contains(&(ProcessStream::Stdout, b"beta".to_vec())));
    assert!(lines.contains(&(ProcessStream::Stderr, b"warning".to_vec())));
    assert!(exit.stdout.is_empty());
    assert_eq!(exit.stderr, b"warning\n");
}

#[test]
fn reports_launch_failure_but_returns_nonzero_exit_output() {
    let working_dir = tempfile::tempdir().unwrap();
    let mut missing = capture_request(working_dir.path(), &["echo-args"]);
    missing.program = ProcessProgram::for_test_fixture(working_dir.path().join("missing.exe"));
    let failure = WindowsProcessSupervisor
        .run(missing, CancellationToken::new(), &NoopSink)
        .expect_err("a missing executable must fail to launch");
    assert_eq!(failure, ProcessFailureKind::LaunchFailed);

    let nonzero = run_capture(&["exit", "7"]);
    assert_eq!(nonzero.code, 7);
}

#[test]
fn rejects_stdout_stderr_and_line_overflow() {
    let working_dir = tempfile::tempdir().unwrap();
    let mut stdout = capture_request(working_dir.path(), &["emit", "9", "0"]);
    stdout.output_mode = OutputMode::Capture {
        stdout_limit: 8,
        stderr_limit: 8,
    };
    assert_eq!(
        WindowsProcessSupervisor.run(stdout, CancellationToken::new(), &NoopSink),
        Err(ProcessFailureKind::StdoutLimitExceeded)
    );

    let mut stderr = capture_request(working_dir.path(), &["emit", "0", "9"]);
    stderr.output_mode = OutputMode::Capture {
        stdout_limit: 8,
        stderr_limit: 8,
    };
    assert_eq!(
        WindowsProcessSupervisor.run(stderr, CancellationToken::new(), &NoopSink),
        Err(ProcessFailureKind::StderrLimitExceeded)
    );

    let line = ProcessRequest {
        program: fixture_program(),
        args: ["long-line", "9"].into_iter().map(OsString::from).collect(),
        working_dir: working_dir.path().to_owned(),
        timeout: Some(Duration::from_secs(5)),
        inactivity_timeout: None,
        output_mode: OutputMode::Lines {
            line_limit: 8,
            retained_stderr_limit: 8,
        },
    };
    assert_eq!(
        WindowsProcessSupervisor.run(line, CancellationToken::new(), &NoopSink),
        Err(ProcessFailureKind::LineLimitExceeded)
    );
}

#[test]
fn distinguishes_wall_and_inactivity_timeouts() {
    let working_dir = tempfile::tempdir().unwrap();
    let mut wall = capture_request(working_dir.path(), &["sleep", "1000"]);
    wall.timeout = Some(Duration::from_millis(100));
    assert_eq!(
        WindowsProcessSupervisor.run(wall, CancellationToken::new(), &NoopSink),
        Err(ProcessFailureKind::TimedOut)
    );

    let mut inactive = capture_request(working_dir.path(), &["sleep", "1000"]);
    inactive.timeout = Some(Duration::from_secs(2));
    inactive.inactivity_timeout = Some(Duration::from_millis(100));
    assert_eq!(
        WindowsProcessSupervisor.run(inactive, CancellationToken::new(), &NoopSink),
        Err(ProcessFailureKind::InactivityTimedOut)
    );
}

#[test]
fn pre_cancel_does_not_launch_and_active_cancel_returns_once() {
    let working_dir = tempfile::tempdir().unwrap();
    let marker = working_dir.path().join("started.pid");
    let marker_text = marker.to_string_lossy().into_owned();
    let request = capture_request(working_dir.path(), &["mark-sleep", &marker_text, "5000"]);
    let canceled = CancellationToken::new();
    canceled.cancel();
    assert_eq!(
        WindowsProcessSupervisor.run(request, canceled, &NoopSink),
        Err(ProcessFailureKind::Canceled)
    );
    assert!(!marker.exists());

    let request = capture_request(working_dir.path(), &["mark-sleep", &marker_text, "5000"]);
    let cancellation = CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let (result_tx, result_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        result_tx
            .send(WindowsProcessSupervisor.run(request, worker_cancellation, &NoopSink))
            .unwrap();
    });
    wait_until(Duration::from_secs(2), || marker.exists());
    cancellation.cancel();
    assert_eq!(
        result_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(ProcessFailureKind::Canceled)
    );
    worker.join().unwrap();
    assert!(matches!(
        result_rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn timeout_terminates_and_reaps_the_entire_descendant_tree() {
    let working_dir = tempfile::tempdir().unwrap();
    let pid_file = working_dir.path().join("tree.pids");
    let pid_file_text = pid_file.to_string_lossy().into_owned();
    let mut request = capture_request(working_dir.path(), &["spawn-tree", &pid_file_text, "5000"]);
    request.timeout = Some(Duration::from_millis(700));

    assert_eq!(
        WindowsProcessSupervisor.run(request, CancellationToken::new(), &NoopSink),
        Err(ProcessFailureKind::TimedOut)
    );
    let pids = fs::read_to_string(pid_file)
        .unwrap()
        .lines()
        .map(|line| line.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(pids.len(), 2);
    wait_until(Duration::from_secs(2), || {
        pids.iter().all(|pid| !process_is_running(*pid))
    });
    assert!(pids.iter().all(|pid| !process_is_running(*pid)));
}

#[test]
fn exited_parent_does_not_disable_cancel_or_deadlines_while_descendants_hold_pipes() {
    for mode in ["cancel", "wall", "inactivity"] {
        let root = tempfile::tempdir().unwrap();
        let pid_file = root.path().join("inherited-pipes.pids");
        let pid_text = pid_file.to_string_lossy().into_owned();
        let mut request = capture_request(
            root.path(),
            &["spawn-inherited-pipes-and-exit", &pid_text, "2000"],
        );
        request.timeout = (mode == "wall").then_some(Duration::from_millis(700));
        request.inactivity_timeout = (mode == "inactivity").then_some(Duration::from_millis(700));
        let cancellation = CancellationToken::new();
        let worker_token = cancellation.clone();
        let worker =
            thread::spawn(move || WindowsProcessSupervisor.run(request, worker_token, &NoopSink));
        wait_until(Duration::from_secs(2), || pid_file.exists());
        let pids = fs::read_to_string(&pid_file)
            .unwrap()
            .lines()
            .map(|line| line.parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        wait_until(Duration::from_secs(2), || !process_is_running(pids[0]));
        assert!(
            process_is_running(pids[1]),
            "descendant must still own inherited pipes"
        );
        if mode == "cancel" {
            cancellation.cancel();
        }
        let result = worker.join().unwrap();
        let expected = match mode {
            "cancel" => ProcessFailureKind::Canceled,
            "wall" => ProcessFailureKind::TimedOut,
            _ => ProcessFailureKind::InactivityTimedOut,
        };
        assert_eq!(result, Err(expected), "termination mode {mode}");
        wait_until(Duration::from_secs(2), || !process_is_running(pids[1]));
    }
}

fn wait_until(timeout: Duration, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(condition(), "condition did not become true before timeout");
}

fn process_is_running(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut exit_code = 0;
        let queried = GetExitCodeProcess(process, &mut exit_code);
        CloseHandle(process);
        queried != 0 && exit_code == STILL_ACTIVE as u32
    }
}
