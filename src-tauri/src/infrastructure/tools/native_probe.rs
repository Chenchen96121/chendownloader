use std::{ffi::OsString, path::Path, time::Duration};

use crate::{
    application::{
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram,
            ProcessRequest, ProcessStream, ProcessSupervisor,
        },
        tool_bootstrap::{ProbeOutput, ToolVersionProbe},
    },
    domain::managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_OUTPUT_LIMIT: usize = 4096;

pub struct NativeToolVersionProbe<P> {
    process: P,
}

impl<P> NativeToolVersionProbe<P> {
    pub fn new(process: P) -> Self {
        Self { process }
    }
}

impl<P: ProcessSupervisor> ToolVersionProbe for NativeToolVersionProbe<P> {
    fn probe(&self, tool: ToolName, candidate: &Path) -> Result<ProbeOutput, ManagedToolError> {
        if !candidate.is_absolute() {
            return Err(probe_failed());
        }
        let working_dir = candidate.parent().ok_or_else(probe_failed)?.to_owned();
        let exit = self
            .process
            .run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_verified_staged_candidate(candidate.to_owned()),
                    args: vec![OsString::from(match tool {
                        ToolName::YtDlp | ToolName::Deno | ToolName::GalleryDl => "--version",
                        ToolName::Ffmpeg | ToolName::Ffprobe => "-version",
                    })],
                    working_dir,
                    timeout: Some(PROBE_TIMEOUT),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: PROBE_OUTPUT_LIMIT,
                        stderr_limit: PROBE_OUTPUT_LIMIT,
                    },
                },
                CancellationToken::new(),
                &NoopSink,
            )
            .map_err(map_process_failure)?;
        Ok(ProbeOutput {
            exit_code: exit.code,
            stdout: String::from_utf8_lossy(&exit.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&exit.stderr).into_owned(),
        })
    }
}

fn map_process_failure(_failure: ProcessFailureKind) -> ManagedToolError {
    probe_failed()
}

fn probe_failed() -> ManagedToolError {
    ManagedToolError::new(
        ManagedToolErrorCode::ToolProbeFailed,
        "native version probe failed",
    )
}

struct NoopSink;

impl ProcessEventSink for NoopSink {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
}
