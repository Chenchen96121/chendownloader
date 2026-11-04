//! Separately trusted optional platform component; never part of core readiness.
use crate::{
    application::process::{
        CancellationToken, OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest,
        ProcessStream, ProcessSupervisor,
    },
    domain::{download::DownloadErrorCode, managed_tools::ToolState, media::AnalysisErrorCode},
    infrastructure::process::WindowsProcessSupervisor,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fs, io::Read, path::PathBuf, sync::Mutex, time::Duration};

pub const TRUSTED_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/tiktok-core/manifest.json"
));

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u8,
    protocol_version: u8,
    target: String,
    version: String,
    artifact: String,
    sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokRuntimeStatus {
    pub state: ToolState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

pub struct TikTokToolResolver {
    root: PathBuf,
    manifest: Manifest,
}
enum CoreFailure {
    Analysis(AnalysisErrorCode),
    Download(DownloadErrorCode),
}
impl From<AnalysisErrorCode> for CoreFailure {
    fn from(error: AnalysisErrorCode) -> Self {
        Self::Analysis(error)
    }
}
impl CoreFailure {
    fn analysis(self) -> AnalysisErrorCode {
        match self {
            Self::Analysis(error) => error,
            Self::Download(DownloadErrorCode::DownloadTimeout) => {
                AnalysisErrorCode::AnalysisTimeout
            }
            Self::Download(_) => AnalysisErrorCode::InvalidMetadata,
        }
    }
    fn download(self) -> DownloadErrorCode {
        match self {
            Self::Analysis(error) => crate::application::tiktok_analysis::map_error(error),
            Self::Download(error) => error,
        }
    }
}
impl TikTokToolResolver {
    pub fn new(root: PathBuf, bytes: &[u8]) -> Result<Self, AnalysisErrorCode> {
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|_| AnalysisErrorCode::ToolIntegrityFailed)?;
        if manifest.schema_version != 1
            || manifest.protocol_version != 1
            || manifest.target != "x86_64-pc-windows-msvc"
            || manifest.version != "1.1.0"
            || manifest.artifact != "tiktok-core-x86_64-pc-windows-msvc.exe"
            || manifest.sha256.len() != 64
            || !manifest
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(AnalysisErrorCode::ToolIntegrityFailed);
        }
        Ok(Self { root, manifest })
    }
    pub fn from_executable() -> Result<Self, AnalysisErrorCode> {
        super::temporary::recover_once();
        let exe = std::env::current_exe().map_err(|_| AnalysisErrorCode::ToolMissing)?;
        let parent = exe.parent().ok_or(AnalysisErrorCode::ToolMissing)?;
        Self::new(parent.join("resources/platform-tools"), TRUSTED_MANIFEST)
    }
    pub fn resolve_verified(&self) -> Result<ProcessProgram, AnalysisErrorCode> {
        let path = self.root.join(&self.manifest.artifact);
        for component in [&self.root, path.as_path()] {
            let metadata =
                fs::symlink_metadata(component).map_err(|_| AnalysisErrorCode::ToolMissing)?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(AnalysisErrorCode::ToolIntegrityFailed);
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(AnalysisErrorCode::ToolIntegrityFailed);
            }
        }
        let mut file = fs::File::open(&path).map_err(|_| AnalysisErrorCode::ToolMissing)?;
        let metadata = file
            .metadata()
            .map_err(|_| AnalysisErrorCode::ToolIntegrityFailed)?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 * 1024 {
            return Err(AnalysisErrorCode::ToolIntegrityFailed);
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let n = file
                .read(&mut buffer)
                .map_err(|_| AnalysisErrorCode::ToolIntegrityFailed)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        if format!("{:x}", hash.finalize()) != self.manifest.sha256 {
            return Err(AnalysisErrorCode::ToolIntegrityFailed);
        }
        Ok(ProcessProgram::from_verified_staged_candidate(path))
    }
    pub fn status(&self) -> TikTokRuntimeStatus {
        match self.resolve_verified() {
            Ok(_) => TikTokRuntimeStatus {
                state: ToolState::Ready,
                version: Some(self.manifest.version.clone()),
            },
            Err(AnalysisErrorCode::ToolMissing) => TikTokRuntimeStatus {
                state: ToolState::Missing,
                version: None,
            },
            Err(_) => TikTokRuntimeStatus {
                state: ToolState::IntegrityFailed,
                version: None,
            },
        }
    }
    pub fn run(
        &self,
        value: &serde_json::Value,
        seconds: u64,
        cancel: CancellationToken,
        sink: Option<&dyn ProcessEventSink>,
    ) -> Result<serde_json::Value, AnalysisErrorCode> {
        self.run_inner(value, seconds, cancel, sink)
            .map_err(CoreFailure::analysis)
    }
    pub fn run_download(
        &self,
        value: &serde_json::Value,
        seconds: u64,
        cancel: CancellationToken,
        sink: Option<&dyn ProcessEventSink>,
    ) -> Result<serde_json::Value, DownloadErrorCode> {
        self.run_inner(value, seconds, cancel, sink)
            .map_err(CoreFailure::download)
    }
    fn run_inner(
        &self,
        value: &serde_json::Value,
        seconds: u64,
        cancel: CancellationToken,
        sink: Option<&dyn ProcessEventSink>,
    ) -> Result<serde_json::Value, CoreFailure> {
        let input = serde_json::to_vec(value).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if input.len() > 65536 {
            return Err(AnalysisErrorCode::InvalidMetadata.into());
        }
        let temporary = super::temporary::OwnedTemporary::new()?;
        let program = self
            .resolve_verified()?
            .with_private_temp_directory(temporary.path().to_owned());
        let events = JsonSink {
            result: Mutex::new(None),
            error: Mutex::new(None),
            sink,
            cancellation: cancel.clone(),
        };
        let request = ProcessRequest {
            stdin: Some(input),
            program,
            args: Vec::<OsString>::new(),
            working_dir: temporary.path().to_owned(),
            timeout: Some(Duration::from_secs(seconds)),
            inactivity_timeout: if value.get("operation").and_then(|v| v.as_str())
                == Some("analyze")
            {
                None
            } else {
                Some(Duration::from_secs(60))
            },
            output_mode: OutputMode::Lines {
                line_limit: 4 * 1024 * 1024,
                retained_stderr_limit: 16384,
            },
        };
        let exit = WindowsProcessSupervisor
            .run(request, cancel, &events)
            .map_err(|e| match e {
                crate::application::process::ProcessFailureKind::TimedOut
                | crate::application::process::ProcessFailureKind::InactivityTimedOut => {
                    AnalysisErrorCode::AnalysisTimeout
                }
                _ => AnalysisErrorCode::ProcessFailed,
            });
        if let Some(error) = events
            .error
            .into_inner()
            .map_err(|_| AnalysisErrorCode::Unknown)?
        {
            return Err(error);
        }
        let exit = exit?;
        if exit.code != 0 {
            return Err(AnalysisErrorCode::ProcessFailed.into());
        }
        events
            .result
            .into_inner()
            .map_err(|_| AnalysisErrorCode::Unknown)?
            .ok_or(AnalysisErrorCode::InvalidMetadata.into())
    }
}

struct JsonSink<'a> {
    result: Mutex<Option<serde_json::Value>>,
    error: Mutex<Option<CoreFailure>>,
    sink: Option<&'a dyn ProcessEventSink>,
    cancellation: CancellationToken,
}
impl ProcessEventSink for JsonSink<'_> {
    fn on_line(&self, stream: ProcessStream, line: &[u8]) {
        if stream != ProcessStream::Stdout {
            return;
        }
        let parsed = serde_json::from_slice::<serde_json::Value>(line);
        let Ok(value) = parsed else {
            if let Ok(mut error) = self.error.lock() {
                *error = Some(AnalysisErrorCode::InvalidMetadata.into());
            }
            self.cancellation.cancel();
            return;
        };
        if value.get("protocolVersion").and_then(|x| x.as_u64()) != Some(1)
            || !value.as_object().is_some_and(|m| {
                m.len() == 3
                    && m.keys()
                        .all(|k| matches!(k.as_str(), "protocolVersion" | "type" | "data"))
            })
            || !value.get("data").is_some_and(|v| v.is_object())
            || self.result.lock().is_ok_and(|result| result.is_some())
            || self.error.lock().is_ok_and(|error| error.is_some())
        {
            if let Ok(mut error) = self.error.lock() {
                *error = Some(AnalysisErrorCode::InvalidMetadata.into());
            }
            self.cancellation.cancel();
            return;
        }
        match value.get("type").and_then(|x| x.as_str()) {
            Some("result") => {
                if let Ok(mut result) = self.result.lock() {
                    if result.is_some() {
                        if let Ok(mut error) = self.error.lock() {
                            *error = Some(AnalysisErrorCode::InvalidMetadata.into());
                        }
                    } else {
                        *result = value.get("data").cloned();
                    }
                }
            }
            Some("progress") => {
                let data = &value["data"];
                let done = data.get("downloadedBytes").and_then(|v| v.as_u64());
                let total = data.get("totalBytes").and_then(|v| v.as_u64());
                if !data.as_object().is_some_and(|m| {
                    m.len() == 2
                        && m.contains_key("downloadedBytes")
                        && m.contains_key("totalBytes")
                }) || done.is_none_or(|n| n > 9_007_199_254_740_991)
                    || !(data["totalBytes"].is_null()
                        || total.is_some_and(|n| {
                            n > 0 && n <= 9_007_199_254_740_991 && done.is_none_or(|d| d <= n)
                        }))
                {
                    if let Ok(mut error) = self.error.lock() {
                        *error = Some(AnalysisErrorCode::InvalidMetadata.into());
                    }
                    self.cancellation.cancel();
                    return;
                }
                if let Some(sink) = self.sink {
                    sink.on_line(stream, line);
                }
            }
            Some("error") => {
                let code = value
                    .pointer("/data/code")
                    .cloned()
                    .and_then(|code| {
                        serde_json::from_value::<AnalysisErrorCode>(code.clone())
                            .ok()
                            .map(CoreFailure::Analysis)
                            .or_else(|| {
                                serde_json::from_value::<DownloadErrorCode>(code)
                                    .ok()
                                    .map(CoreFailure::Download)
                            })
                    })
                    .unwrap_or(CoreFailure::Analysis(AnalysisErrorCode::InvalidMetadata));
                if let Ok(mut error) = self.error.lock() {
                    *error = Some(code);
                }
            }
            _ => {
                if let Ok(mut error) = self.error.lock() {
                    *error = Some(AnalysisErrorCode::InvalidMetadata.into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sink() -> JsonSink<'static> {
        JsonSink {
            result: Mutex::new(None),
            error: Mutex::new(None),
            sink: None,
            cancellation: CancellationToken::new(),
        }
    }
    #[test]
    fn protocol_rejects_unknown_envelope_fields_invalid_progress_and_empty_result() {
        for value in [
            serde_json::json!({"protocolVersion":1,"type":"result","data":{},"cookie":"secret"}),
            serde_json::json!({"protocolVersion":1,"type":"result"}),
            serde_json::json!({"protocolVersion":1,"type":"progress","data":{"downloadedBytes":-1,"totalBytes":20}}),
        ] {
            let sink = sink();
            sink.on_line(ProcessStream::Stdout, &serde_json::to_vec(&value).unwrap());
            assert!(sink.error.lock().unwrap().is_some());
        }
    }
    #[test]
    fn disk_full_and_download_timeout_keep_their_download_error() {
        for (code, expected) in [
            ("disk_full", DownloadErrorCode::DiskFull),
            ("download_timeout", DownloadErrorCode::DownloadTimeout),
        ] {
            let sink = sink();
            sink.on_line(
                ProcessStream::Stdout,
                &serde_json::to_vec(
                    &serde_json::json!({"protocolVersion":1,"type":"error","data":{"code":code}}),
                )
                .unwrap(),
            );
            assert_eq!(
                sink.error.into_inner().unwrap().unwrap().download(),
                expected
            );
        }
    }
}
