use std::{
    io::{Read, Write},
    mem::size_of,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::CREATE_NO_WINDOW,
    },
};

use crate::application::process::{
    CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
    ProcessRequest, ProcessStream, ProcessSupervisor,
};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const PIPE_CHANNEL_DEPTH: usize = 16;

pub struct WindowsProcessSupervisor;

fn configure_system_proxy_for_tool(
    command: &mut Command,
    program: &crate::application::process::ProcessProgram,
    proxies: &crate::infrastructure::system_proxy::SystemProxies,
) {
    if !program.system_proxy_allowed() {
        return;
    }
    let proxies = program
        .network_profile()
        .map(|profile| &profile.proxies)
        .unwrap_or(proxies);
    if let Some(host) = program.explicit_https_proxy_host() {
        // The pinned yt-dlp curl transport does not reliably honor proxy env
        // variables. This value comes only from the validated native profile.
        command.arg("--proxy").arg(
            proxies
                .https_for_host(host)
                .map(url::Url::as_str)
                .unwrap_or(""),
        );
    }
    if let Some(proxy) = &proxies.http {
        command.env("HTTP_PROXY", proxy.as_str());
    }
    if let Some(proxy) = &proxies.https {
        command.env("HTTPS_PROXY", proxy.as_str());
    }
    if (proxies.http.is_some() || proxies.https.is_some()) && proxies.bypass.is_some() {
        command.env("NO_PROXY", proxies.bypass.as_deref().unwrap_or_default());
    }
}

impl ProcessSupervisor for WindowsProcessSupervisor {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        if cancellation.is_canceled() {
            return Err(ProcessFailureKind::Canceled);
        }
        if request
            .stdin
            .as_ref()
            .is_some_and(|input| input.len() > 65_536)
        {
            return Err(ProcessFailureKind::IoFailed);
        }

        let job = Job::new()?;
        let mut command = Command::new(request.program.path());
        command
            .current_dir(&request.working_dir)
            .stdin(if request.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .creation_flags(CREATE_NO_WINDOW);
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        if let Some(directory) = request.program.private_temp_directory() {
            command.env("TEMP", directory).env("TMP", directory);
        }
        if request.program.system_proxy_allowed() {
            configure_system_proxy_for_tool(
                &mut command,
                &request.program,
                &crate::infrastructure::system_proxy::SystemProxies::current(),
            );
        }
        command.args(&request.args);

        let mut child = command
            .spawn()
            .map_err(|_| ProcessFailureKind::LaunchFailed)?;
        if job.assign(&child).is_err() {
            terminate_and_reap(&job, &mut child);
            return Err(ProcessFailureKind::JobControlFailed);
        }

        let stdout = child.stdout.take().ok_or(ProcessFailureKind::IoFailed)?;
        let stderr = child.stderr.take().ok_or(ProcessFailureKind::IoFailed)?;
        let (tx, rx) = mpsc::sync_channel(PIPE_CHANNEL_DEPTH);
        let stdout_reader = spawn_reader(ProcessStream::Stdout, stdout, tx.clone());
        let stderr_reader = spawn_reader(ProcessStream::Stderr, stderr, tx.clone());
        let input_writer = request.stdin.as_ref().map(|input| {
            let bytes = input.clone();
            let pipe = child.stdin.take();
            let sender = tx.clone();
            thread::spawn(move || {
                let success = pipe.is_some_and(|mut pipe| pipe.write_all(&bytes).is_ok());
                let _ = sender.send(PipeEvent::InputFinished(success));
            })
        });
        drop(tx);

        let result = supervise(&job, &mut child, &rx, &request, &cancellation, sink);
        drop(rx);
        join_reader(stdout_reader);
        join_reader(stderr_reader);
        if let Some(writer) = input_writer {
            join_reader(writer);
        }
        result
    }
}

fn supervise(
    job: &Job,
    child: &mut Child,
    receiver: &Receiver<PipeEvent>,
    request: &ProcessRequest,
    cancellation: &CancellationToken,
    sink: &dyn ProcessEventSink,
) -> Result<ProcessExit, ProcessFailureKind> {
    let started = Instant::now();
    let mut last_activity = started;
    let mut collector = OutputCollector::new(&request.output_mode);
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let mut status = None;
    let mut input_finished = request.stdin.is_none();

    loop {
        if status.is_none() {
            status = child.try_wait().map_err(|_| ProcessFailureKind::IoFailed)?;
        }
        if status.is_some() && stdout_eof && stderr_eof && input_finished {
            break;
        }
        // Descendants may keep inherited pipes open after the parent exits.
        // Cancellation and deadlines must cover draining those pipes as well.
        let failure = if cancellation.is_canceled() {
            Some(ProcessFailureKind::Canceled)
        } else if request
            .timeout
            .is_some_and(|timeout| started.elapsed() >= timeout)
        {
            Some(ProcessFailureKind::TimedOut)
        } else if request
            .inactivity_timeout
            .is_some_and(|timeout| last_activity.elapsed() >= timeout)
        {
            Some(ProcessFailureKind::InactivityTimedOut)
        } else {
            None
        };
        if let Some(failure) = failure {
            terminate_and_reap(job, child);
            return Err(failure);
        }

        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(PipeEvent::InputFinished(true)) => input_finished = true,
            Ok(PipeEvent::InputFinished(false)) => {
                terminate_and_reap(job, child);
                return Err(ProcessFailureKind::IoFailed);
            }
            Ok(PipeEvent::Data(stream, bytes)) => {
                last_activity = Instant::now();
                if let Err(failure) = collector.push(stream, &bytes, sink) {
                    terminate_and_reap(job, child);
                    return Err(failure);
                }
            }
            Ok(PipeEvent::Eof(stream)) => {
                if let Err(failure) = collector.finish_stream(stream, sink) {
                    terminate_and_reap(job, child);
                    return Err(failure);
                }
                match stream {
                    ProcessStream::Stdout => stdout_eof = true,
                    ProcessStream::Stderr => stderr_eof = true,
                }
            }
            Ok(PipeEvent::Failed) => {
                terminate_and_reap(job, child);
                return Err(ProcessFailureKind::IoFailed);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if !stdout_eof || !stderr_eof {
                    terminate_and_reap(job, child);
                    return Err(ProcessFailureKind::IoFailed);
                }
            }
        }
    }

    let status = status.expect("process exit was observed before completion");
    let code = status.code().unwrap_or(-1);
    let (stdout, stderr) = collector.finish();
    Ok(ProcessExit {
        code,
        stdout,
        stderr,
    })
}

enum PipeEvent {
    InputFinished(bool),
    Data(ProcessStream, Vec<u8>),
    Eof(ProcessStream),
    Failed,
}

fn spawn_reader(
    stream: ProcessStream,
    mut pipe: impl Read + Send + 'static,
    sender: SyncSender<PipeEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(PipeEvent::Eof(stream));
                    break;
                }
                Ok(count) => {
                    if sender
                        .send(PipeEvent::Data(stream, buffer[..count].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => {
                    let _ = sender.send(PipeEvent::Failed);
                    break;
                }
            }
        }
    })
}

fn join_reader(reader: JoinHandle<()>) {
    let _ = reader.join();
}

enum OutputCollector {
    Capture {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_limit: usize,
        stderr_limit: usize,
    },
    Lines {
        stdout_line: Vec<u8>,
        stderr_line: Vec<u8>,
        stderr: Vec<u8>,
        line_limit: usize,
        retained_stderr_limit: usize,
    },
}

impl OutputCollector {
    fn new(mode: &OutputMode) -> Self {
        match *mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => Self::Capture {
                stdout: Vec::new(),
                stderr: Vec::new(),
                stdout_limit,
                stderr_limit,
            },
            OutputMode::Lines {
                line_limit,
                retained_stderr_limit,
            } => Self::Lines {
                stdout_line: Vec::new(),
                stderr_line: Vec::new(),
                stderr: Vec::new(),
                line_limit,
                retained_stderr_limit,
            },
        }
    }

    fn push(
        &mut self,
        stream: ProcessStream,
        bytes: &[u8],
        sink: &dyn ProcessEventSink,
    ) -> Result<(), ProcessFailureKind> {
        match self {
            Self::Capture {
                stdout,
                stderr,
                stdout_limit,
                stderr_limit,
            } => match stream {
                ProcessStream::Stdout => append_bounded(
                    stdout,
                    bytes,
                    *stdout_limit,
                    ProcessFailureKind::StdoutLimitExceeded,
                ),
                ProcessStream::Stderr => append_bounded(
                    stderr,
                    bytes,
                    *stderr_limit,
                    ProcessFailureKind::StderrLimitExceeded,
                ),
            },
            Self::Lines {
                stdout_line,
                stderr_line,
                stderr,
                line_limit,
                retained_stderr_limit,
            } => {
                if stream == ProcessStream::Stderr {
                    append_bounded(
                        stderr,
                        bytes,
                        *retained_stderr_limit,
                        ProcessFailureKind::StderrLimitExceeded,
                    )?;
                }
                let line = match stream {
                    ProcessStream::Stdout => stdout_line,
                    ProcessStream::Stderr => stderr_line,
                };
                push_lines(line, bytes, *line_limit, stream, sink)
            }
        }
    }

    fn finish_stream(
        &mut self,
        stream: ProcessStream,
        sink: &dyn ProcessEventSink,
    ) -> Result<(), ProcessFailureKind> {
        if let Self::Lines {
            stdout_line,
            stderr_line,
            line_limit,
            ..
        } = self
        {
            let line = match stream {
                ProcessStream::Stdout => stdout_line,
                ProcessStream::Stderr => stderr_line,
            };
            emit_partial_line(line, *line_limit, stream, sink)?;
        }
        Ok(())
    }

    fn finish(self) -> (Vec<u8>, Vec<u8>) {
        match self {
            Self::Capture { stdout, stderr, .. } => (stdout, stderr),
            Self::Lines { stderr, .. } => (Vec::new(), stderr),
        }
    }
}

fn append_bounded(
    target: &mut Vec<u8>,
    bytes: &[u8],
    limit: usize,
    failure: ProcessFailureKind,
) -> Result<(), ProcessFailureKind> {
    if target
        .len()
        .checked_add(bytes.len())
        .is_none_or(|length| length > limit)
    {
        return Err(failure);
    }
    target.extend_from_slice(bytes);
    Ok(())
}

fn push_lines(
    pending: &mut Vec<u8>,
    bytes: &[u8],
    limit: usize,
    stream: ProcessStream,
    sink: &dyn ProcessEventSink,
) -> Result<(), ProcessFailureKind> {
    pending.extend_from_slice(bytes);
    while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
        let mut line = pending.drain(..=newline).collect::<Vec<_>>();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.len() > limit {
            return Err(ProcessFailureKind::LineLimitExceeded);
        }
        sink.on_line(stream, &line);
    }
    if pending.len() > limit {
        return Err(ProcessFailureKind::LineLimitExceeded);
    }
    Ok(())
}

fn emit_partial_line(
    pending: &mut Vec<u8>,
    limit: usize,
    stream: ProcessStream,
    sink: &dyn ProcessEventSink,
) -> Result<(), ProcessFailureKind> {
    if pending.len() > limit {
        return Err(ProcessFailureKind::LineLimitExceeded);
    }
    if !pending.is_empty() {
        if pending.last() == Some(&b'\r') {
            pending.pop();
        }
        sink.on_line(stream, pending);
        pending.clear();
    }
    Ok(())
}

struct Job(HANDLE);

impl Job {
    fn new() -> Result<Self, ProcessFailureKind> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(ProcessFailureKind::JobControlFailed);
            }
            let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const information).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if configured == 0 {
                CloseHandle(handle);
                return Err(ProcessFailureKind::JobControlFailed);
            }
            Ok(Self(handle))
        }
    }

    fn assign(&self, child: &Child) -> Result<(), ProcessFailureKind> {
        let process = child.as_raw_handle() as HANDLE;
        if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
            Err(ProcessFailureKind::JobControlFailed)
        } else {
            Ok(())
        }
    }

    fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn terminate_and_reap(job: &Job, child: &mut Child) {
    job.terminate();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod proxy_tests {
    use super::*;
    use crate::{
        application::{process::ProcessProgram, tool_resolver::ResolvedTool},
        domain::managed_tools::ToolName,
        infrastructure::system_proxy::SystemProxies,
    };
    use std::{collections::HashMap, path::PathBuf};

    #[test]
    fn social_proxy_argument_uses_snapshot_before_source_arguments_and_honors_bypass() {
        let captured = SystemProxies::from_windows_manual_settings(true, "127.0.0.1:7011");
        let changed = SystemProxies::from_windows_manual_settings(true, "127.0.0.1:7022");
        for (bypass, expected) in [
            (None, "http://127.0.0.1:7011/"),
            (Some(".instagram.com".into()), ""),
        ] {
            let mut captured = captured.clone();
            captured.bypass = bypass;
            let program = ProcessProgram::from_resolved_tool(ResolvedTool {
                name: ToolName::YtDlp,
                version: "fixture".into(),
                path: PathBuf::from("fixture.exe"),
            })
            .with_network_profile(std::sync::Arc::new(captured.snapshot()))
            .with_explicit_https_proxy_for_host("www.instagram.com");
            let mut command = Command::new(program.path());
            configure_system_proxy_for_tool(&mut command, &program, &changed);
            command.args([
                "--ignore-config",
                "--",
                "https://www.instagram.com/reel/Test/",
            ]);
            let args: Vec<_> = command
                .get_args()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            assert_eq!(&args[..3], ["--proxy", expected, "--ignore-config"]);
            assert_eq!(args[3], "--");
        }
    }

    #[test]
    fn explicit_network_snapshot_overrides_later_system_changes() {
        use std::sync::Arc;
        let captured = SystemProxies::from_windows_manual_settings(true, "127.0.0.1:7011");
        let changed = SystemProxies::from_windows_manual_settings(true, "127.0.0.1:7022");
        let profile = Arc::new(captured.snapshot());
        for name in [ToolName::GalleryDl, ToolName::YtDlp] {
            let program = ProcessProgram::from_resolved_tool(ResolvedTool {
                name,
                version: "fixture".into(),
                path: PathBuf::from("fixture.exe"),
            })
            .with_network_profile(Arc::clone(&profile));
            let mut command = Command::new(program.path());
            command.env_clear();
            configure_system_proxy_for_tool(&mut command, &program, &changed);
            let environment: HashMap<_, _> = command
                .get_envs()
                .filter_map(|(k, v)| {
                    v.map(|v| {
                        (
                            k.to_string_lossy().into_owned(),
                            v.to_string_lossy().into_owned(),
                        )
                    })
                })
                .collect();
            assert_eq!(
                environment.get("HTTPS_PROXY").map(String::as_str),
                Some("http://127.0.0.1:7011/")
            );
        }
        for name in [ToolName::Ffmpeg, ToolName::Ffprobe, ToolName::Deno] {
            let program = ProcessProgram::from_resolved_tool(ResolvedTool {
                name,
                version: "fixture".into(),
                path: PathBuf::from("fixture.exe"),
            })
            .with_network_profile(Arc::clone(&profile));
            let mut command = Command::new(program.path());
            command.env_clear();
            configure_system_proxy_for_tool(&mut command, &program, &changed);
            assert!(command.get_envs().next().is_none());
        }
        let bypass = SystemProxies {
            http: captured.http.clone(),
            https: captured.https.clone(),
            bypass: Some(".twimg.com".into()),
        }
        .snapshot();
        assert!(bypass.https_proxy_for_host("video.twimg.com").is_none());
        assert_eq!(
            bypass.https_proxy_for_host("x.com").map(url::Url::as_str),
            Some("http://127.0.0.1:7011/")
        );
    }

    #[test]
    fn only_verified_network_tools_receive_the_explicit_system_proxy() {
        for (name, expected) in [
            (ToolName::YtDlp, true),
            (ToolName::GalleryDl, true),
            (ToolName::Ffmpeg, false),
            (ToolName::Ffprobe, false),
            (ToolName::Deno, false),
        ] {
            let program = ProcessProgram::from_resolved_tool(ResolvedTool {
                name,
                version: "fixture".to_owned(),
                path: PathBuf::from("fixture.exe"),
            });
            let proxies = SystemProxies {
                http: Some(url::Url::parse("http://127.0.0.1:7897").unwrap()),
                https: Some(url::Url::parse("http://127.0.0.1:7897").unwrap()),
                bypass: None,
            };
            let mut command = Command::new(program.path());
            command.env_clear();
            configure_system_proxy_for_tool(&mut command, &program, &proxies);
            let environment: HashMap<_, _> = command
                .get_envs()
                .filter_map(|(name, value)| {
                    value.map(|value| {
                        (
                            name.to_string_lossy().into_owned(),
                            value.to_string_lossy().into_owned(),
                        )
                    })
                })
                .collect();
            assert_eq!(
                environment.get("HTTP_PROXY").map(String::as_str),
                expected.then_some("http://127.0.0.1:7897/")
            );
            assert_eq!(
                environment.get("HTTPS_PROXY").map(String::as_str),
                expected.then_some("http://127.0.0.1:7897/")
            );
        }
    }
}
