use std::{ffi::OsString, time::Duration};

use crate::{
    application::{
        analyze_media::MetadataAnalyzer,
        media_url::ValidatedMediaUrl,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest,
            ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        managed_tools::ToolName,
        media::{AnalysisErrorCode, MediaInfo, Platform},
    },
};

use super::{errors, normalize_metadata};

const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(45);
const STDOUT_LIMIT: usize = super::metadata::MAX_METADATA_BYTES;
const STDERR_LIMIT: usize = 65_536;

pub struct YtDlpMetadataAnalyzer<R, P> {
    resolver: R,
    process: P,
}

impl<R, P> YtDlpMetadataAnalyzer<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}

impl<R, P> MetadataAnalyzer for YtDlpMetadataAnalyzer<R, P>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    fn analyze(&self, source: &ValidatedMediaUrl) -> Result<MediaInfo, AnalysisErrorCode> {
        if source.platform() != Platform::Youtube {
            return Err(AnalysisErrorCode::UnsupportedPlatform);
        }
        let yt_dlp = self
            .resolver
            .resolve_verified(ToolName::YtDlp)
            .map_err(errors::map_tool_error)?;
        let deno = self
            .resolver
            .resolve_verified(ToolName::Deno)
            .map_err(errors::map_tool_error)?;
        let working_dir = yt_dlp
            .path
            .parent()
            .ok_or(AnalysisErrorCode::ProcessFailed)?
            .to_owned();
        let mut deno_runtime = OsString::from("deno:");
        deno_runtime.push(&deno.path);
        let args = vec![
            "--ignore-config".into(),
            "--no-remote-components".into(),
            "--no-plugin-dirs".into(),
            "--js-runtimes".into(),
            deno_runtime,
            "--simulate".into(),
            "--dump-single-json".into(),
            "--no-playlist".into(),
            "--no-warnings".into(),
            "--".into(),
            source.url().into(),
        ];
        let exit = self
            .process
            .run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_resolved_tool(yt_dlp),
                    args,
                    working_dir,
                    timeout: Some(ANALYSIS_TIMEOUT),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: STDOUT_LIMIT,
                        stderr_limit: STDERR_LIMIT,
                    },
                },
                CancellationToken::new(),
                &NoopSink,
            )
            .map_err(errors::map_process_error)?;
        if exit.code != 0 {
            return Err(errors::classify_nonzero_exit(&exit.stderr));
        }
        let stdout =
            std::str::from_utf8(&exit.stdout).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        normalize_metadata(source.platform(), stdout)
    }
}

struct NoopSink;

impl ProcessEventSink for NoopSink {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
}
