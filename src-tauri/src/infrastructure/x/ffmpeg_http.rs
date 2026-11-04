//! A scoped compatibility route for native TLS disconnects. FFmpeg only copies
//! raw HTTP bytes here; playlist interpretation stays in the native validators.
use super::{
    hls_local::HlsTransferError,
    url_policy::{validate_x_asset_url, XAssetKind},
};
use crate::{
    application::{
        portable_package::EMBEDDED_TOOL_MANIFEST,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram,
            ProcessRequest, ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ResolvedTool,
        x_source::XRequestContext,
    },
    domain::{
        managed_tools::{ManagedToolManifest, ToolName},
        media::AnalysisErrorCode,
    },
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct FfmpegHttpTool {
    path: PathBuf,
    version: String,
    digest: String,
}

impl FfmpegHttpTool {
    pub(super) fn from_resolved(tool: ResolvedTool) -> Option<Self> {
        let manifest: ManagedToolManifest = serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).ok()?;
        let pinned = manifest.tools.iter().find(|t| t.name == ToolName::Ffmpeg)?;
        (tool.name == ToolName::Ffmpeg && tool.version == pinned.version).then(|| Self {
            path: tool.path,
            version: tool.version,
            digest: pinned.sha256.clone(),
        })
    }

    fn revalidate(&self, context: &XRequestContext) -> Result<(), HlsTransferError> {
        let integrity = || HlsTransferError::from(AnalysisErrorCode::ToolIntegrityFailed);
        let meta = fs::symlink_metadata(&self.path).map_err(|_| integrity())?;
        if !regular(&meta) {
            return Err(integrity());
        }
        let mut file = File::open(&self.path).map_err(|_| integrity())?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 65536];
        loop {
            check_context(context)?;
            let count = file.read(&mut buffer).map_err(|_| integrity())?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if format!("{:x}", digest.finalize()) != self.digest {
            return Err(integrity());
        }
        Ok(())
    }
}

pub(super) fn fetch(
    url: &url::Url,
    destination: &Path,
    maximum: u64,
    probe: bool,
    io_timeout: Duration,
    context: &XRequestContext,
) -> Option<Result<u64, HlsTransferError>> {
    let tool = context.network.x_video_http.ffmpeg.get()?;
    #[cfg(windows)]
    {
        Some(fetch_with(
            tool,
            &crate::infrastructure::process::WindowsProcessSupervisor,
            url,
            destination,
            maximum,
            probe,
            io_timeout,
            context,
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = (tool, url, destination, maximum, probe, io_timeout);
        None
    }
}

struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _: ProcessStream, _: &[u8]) {}
}

fn check_context(context: &XRequestContext) -> Result<Duration, HlsTransferError> {
    if context.cancellation.is_canceled() {
        return Err(AnalysisErrorCode::ProcessFailed.into());
    }
    context
        .deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| AnalysisErrorCode::AnalysisTimeout.into())
}

fn regular(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    meta.is_file() && !meta.file_type().is_symlink() && !reparse
}

fn raw_args(
    url: &url::Url,
    proxy: Option<&url::Url>,
    output: &Path,
    maximum: u64,
    probe: bool,
    io_timeout: Duration,
) -> Result<Vec<OsString>, HlsTransferError> {
    validate_x_asset_url(url.as_str(), XAssetKind::Video)?;
    if maximum == 0 || maximum > 512 * 1024 * 1024 || io_timeout.is_zero() {
        return Err(AnalysisErrorCode::InvalidMetadata.into());
    }
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-xerror",
        "-tls_verify",
        "1",
        "-max_redirects",
        "0",
        "-seekable",
        "0",
        "-protocol_whitelist",
        "https,http,tcp,tls,httpproxy",
        "-referer",
        "https://x.com/",
        "-user_agent",
        "Mozilla/5.0",
    ]
    .map(Into::into)
    .to_vec();
    if let Some(proxy) = proxy {
        if !matches!(proxy.scheme(), "http" | "https")
            || proxy.host_str().is_none()
            || !proxy.username().is_empty()
            || proxy.password().is_some()
            || proxy.path() != "/"
            || proxy.query().is_some()
            || proxy.fragment().is_some()
        {
            return Err(AnalysisErrorCode::AnalysisUnavailable.into());
        }
        args.extend([
            OsString::from("-http_proxy"),
            OsString::from(proxy.as_str()),
        ]);
    }
    // Full reads request one sentinel byte beyond the limit, so an oversized
    // object cannot be silently mistaken for a complete segment. Probe reads
    // intentionally stop at a prefix and are never recorded as complete.
    let end = maximum
        .checked_add(u64::from(!probe))
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    args.extend([
        OsString::from("-rw_timeout"),
        OsString::from(io_timeout.as_micros().min(u64::MAX as u128).to_string()),
        OsString::from("-f"),
        OsString::from("data"),
        OsString::from("-raw_packet_size"),
        OsString::from("1024"),
        OsString::from("-i"),
        OsString::from(url.as_str()),
        OsString::from("-map"),
        OsString::from("0:0"),
        OsString::from("-c"),
        OsString::from("copy"),
        OsString::from("-fs"),
        OsString::from(end.to_string()),
        OsString::from("-f"),
        OsString::from("data"),
        output.as_os_str().to_owned(),
    ]);
    Ok(args)
}

fn validate_output(directory: &Path, output: &Path, maximum: u64) -> Result<u64, HlsTransferError> {
    let mut count = 0;
    for entry in fs::read_dir(directory).map_err(HlsTransferError::from)? {
        let entry = entry.map_err(HlsTransferError::from)?;
        count += 1;
        if count > 1 || entry.path() != output {
            return Err(AnalysisErrorCode::InvalidMetadata.into());
        }
    }
    let meta = fs::symlink_metadata(output).map_err(HlsTransferError::from)?;
    if !regular(&meta) || meta.len() == 0 || meta.len() > maximum {
        return Err(AnalysisErrorCode::InvalidMetadata.into());
    }
    Ok(meta.len())
}

fn watch_output(directory: &Path, output: &Path, maximum: u64) -> Result<(), HlsTransferError> {
    let mut count = 0;
    for entry in fs::read_dir(directory).map_err(HlsTransferError::from)? {
        let entry = entry.map_err(HlsTransferError::from)?;
        count += 1;
        let meta = fs::symlink_metadata(entry.path()).map_err(HlsTransferError::from)?;
        if count > 1 || entry.path() != output || !regular(&meta) || meta.len() > maximum {
            return Err(AnalysisErrorCode::InvalidMetadata.into());
        }
    }
    Ok(())
}

// Keep the verified tool/process dependencies separate from the immutable
// per-read policy; tests exercise the exact native transfer path.
#[allow(clippy::too_many_arguments)]
fn fetch_with(
    tool: &FfmpegHttpTool,
    process: &dyn ProcessSupervisor,
    url: &url::Url,
    destination: &Path,
    maximum: u64,
    probe: bool,
    io_timeout: Duration,
    context: &XRequestContext,
) -> Result<u64, HlsTransferError> {
    check_context(context)?;
    validate_x_asset_url(url.as_str(), XAssetKind::Video)?;
    let parent = destination
        .parent()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let directory = tempfile::Builder::new()
        .prefix("x-http-")
        .tempdir_in(parent)
        .map_err(HlsTransferError::from)?;
    let output = directory.path().join("raw.bin");
    let args = raw_args(
        url,
        context.network.https_proxy_for_host("video.twimg.com"),
        &output,
        maximum,
        probe,
        io_timeout,
    )?;
    tool.revalidate(context)?;
    let timeout = check_context(context)?;
    // FFmpeg stops at a packet boundary and may write one final raw packet
    // past -fs. Only probe files get this small staging allowance; publish at
    // most the requested prefix. Complete downloads retain the strict limit.
    let staging_maximum = maximum
        .checked_add(if probe { 1024 } else { 0 })
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let stop = AtomicBool::new(false);
    let cancellation = CancellationToken::new();
    let watcher_error = Mutex::new(None);
    let result = thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Acquire) {
                let result = check_context(context)
                    .map(|_| ())
                    .and_then(|_| watch_output(directory.path(), &output, staging_maximum));
                if let Err(error) = result {
                    *watcher_error.lock().unwrap() = Some(error);
                    cancellation.cancel();
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let result = process.run(
            ProcessRequest {
                stdin: None,
                program: ProcessProgram::from_resolved_tool(ResolvedTool {
                    name: ToolName::Ffmpeg,
                    version: tool.version.clone(),
                    path: tool.path.clone(),
                }),
                args,
                working_dir: directory.path().to_owned(),
                timeout: Some(timeout),
                inactivity_timeout: None,
                output_mode: OutputMode::Capture {
                    stdout_limit: 1024,
                    stderr_limit: 65536,
                },
            },
            cancellation.clone(),
            &Discard,
        );
        stop.store(true, Ordering::Release);
        result
    });
    check_context(context)?;
    if let Some(error) = watcher_error.into_inner().unwrap() {
        return Err(error);
    }
    let exit = result.map_err(|error| {
        HlsTransferError::from(match error {
            ProcessFailureKind::TimedOut | ProcessFailureKind::InactivityTimedOut => {
                AnalysisErrorCode::AnalysisTimeout
            }
            ProcessFailureKind::StdoutLimitExceeded
            | ProcessFailureKind::StderrLimitExceeded
            | ProcessFailureKind::LineLimitExceeded => AnalysisErrorCode::InvalidMetadata,
            _ => AnalysisErrorCode::ProcessFailed,
        })
    })?;
    if exit.code != 0 {
        return Err(exit_error(&exit.stderr));
    }
    let count = validate_output(directory.path(), &output, staging_maximum)?.min(maximum);
    let mut source = File::open(&output)
        .map_err(HlsTransferError::from)?
        .take(count);
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(HlsTransferError::from)?;
    let publish = (|| {
        let mut total = 0_u64;
        let mut buffer = [0_u8; 65536];
        loop {
            check_context(context)?;
            let n = source.read(&mut buffer).map_err(HlsTransferError::from)?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .filter(|n| *n <= count)
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            destination_file
                .write_all(&buffer[..n])
                .map_err(HlsTransferError::from)?;
        }
        if total != count {
            return Err(AnalysisErrorCode::InvalidMetadata.into());
        }
        destination_file
            .sync_all()
            .map_err(HlsTransferError::from)?;
        check_context(context)?;
        Ok(total)
    })();
    drop(destination_file);
    if publish.is_err() {
        fs::remove_file(destination).map_err(HlsTransferError::from)?;
    }
    publish
}

fn exit_error(stderr: &[u8]) -> HlsTransferError {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if ["no space left on device", "not enough space", "disk full"]
        .iter()
        .any(|s| text.contains(s))
    {
        return HlsTransferError::Io(std::io::ErrorKind::StorageFull);
    }
    let code = if text.contains("401 unauthorized") {
        AnalysisErrorCode::AuthenticationRequired
    } else if text.contains("403 forbidden") {
        AnalysisErrorCode::AccessDenied
    } else if text.contains("404 not found") {
        AnalysisErrorCode::ContentUnavailable
    } else if text.contains("429 too many requests") {
        AnalysisErrorCode::RateLimited
    } else {
        AnalysisErrorCode::NetworkFailed
    };
    code.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[cfg(windows)]
    #[test]
    #[ignore = "requires pinned tools and an explicit approved live X segment URL; real network transfer"]
    fn real_ffmpeg_raw_full_and_probe_preserve_identical_source_bytes() {
        let manifest: ManagedToolManifest = serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).unwrap();
        let pinned = manifest
            .tools
            .iter()
            .find(|t| t.name == ToolName::Ffmpeg)
            .unwrap();
        let tool_root = PathBuf::from(
            std::env::var_os("LMD_REAL_TOOL_PACKAGE").expect("explicit pinned tool root"),
        );
        let tool = FfmpegHttpTool::from_resolved(ResolvedTool {
            name: ToolName::Ffmpeg,
            version: pinned.version.clone(),
            path: tool_root
                .join(ToolName::Ffmpeg.artifact_name(crate::domain::managed_tools::BUILD_TARGET)),
        })
        .unwrap();
        let url = std::env::var("LMD_X_RAW_SAMPLE_URL")
            .expect("explicit approved public CDN sample")
            .parse()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let full = directory.path().join("full.bin");
        let prefix = directory.path().join("prefix.bin");
        let context = XRequestContext::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(120),
            Arc::new(crate::application::network_profile::NetworkProfile::current()),
        );
        let bytes = fetch_with(
            &tool,
            &crate::infrastructure::process::WindowsProcessSupervisor,
            &url,
            &full,
            32 * 1024 * 1024,
            false,
            Duration::from_secs(8),
            &context,
        )
        .unwrap();
        let sampled = fetch_with(
            &tool,
            &crate::infrastructure::process::WindowsProcessSupervisor,
            &url,
            &prefix,
            1024 * 1024,
            true,
            Duration::from_secs(8),
            &context,
        )
        .unwrap();
        assert!(bytes > 1024 * 1024, "sample must exercise a partial read");
        assert_eq!(sampled, 1024 * 1024);
        assert_eq!(
            fs::read(prefix).unwrap(),
            fs::read(full).unwrap()[..sampled as usize]
        );
        eprintln!("X raw transport verified: fullBytes={bytes}, prefixBytes={sampled}");
    }

    #[test]
    fn raw_command_enforces_tls_no_redirects_and_prefix_bounds() {
        let url = url::Url::parse("https://video.twimg.com/fixture/master.m3u8").unwrap();
        let args = raw_args(
            &url,
            None,
            Path::new("private.bin"),
            4096,
            false,
            Duration::from_secs(15),
        )
        .unwrap();
        let args: Vec<_> = args.iter().map(|s| s.to_str().unwrap()).collect();
        for pair in [
            ["-tls_verify", "1"],
            ["-max_redirects", "0"],
            ["-fs", "4097"],
            ["-seekable", "0"],
            ["-rw_timeout", "15000000"],
            ["-protocol_whitelist", "https,http,tcp,tls,httpproxy"],
        ] {
            assert!(args.windows(2).any(|v| v == pair));
        }
        assert_eq!(args.iter().filter(|s| **s == "data").count(), 2);
        let probe = raw_args(
            &url,
            None,
            Path::new("private.bin"),
            4096,
            true,
            Duration::from_secs(8),
        )
        .unwrap();
        assert!(probe.windows(2).any(|v| v[0] == "-fs" && v[1] == "4096"));
    }

    #[test]
    fn raw_command_rejects_unvalidated_proxy_routes_and_urls() {
        let url = url::Url::parse("https://video.twimg.com/fixture/master.m3u8").unwrap();
        for proxy in [
            "socks5://localhost:1234/",
            "http://user:password@localhost/",
            "http://localhost/path",
            "https://localhost/?q=1",
        ] {
            assert!(raw_args(
                &url,
                Some(&proxy.parse().unwrap()),
                Path::new("out"),
                1,
                false,
                Duration::from_secs(8)
            )
            .is_err());
        }
        let bad = url::Url::parse("https://video.twimg.com.evil.test/master.m3u8").unwrap();
        assert!(raw_args(
            &bad,
            None,
            Path::new("out"),
            1,
            false,
            Duration::from_secs(8)
        )
        .is_err());
        assert!(raw_args(
            &url,
            None,
            Path::new("out"),
            0,
            false,
            Duration::from_secs(8)
        )
        .is_err());
    }

    #[test]
    fn output_validation_rejects_overflow_empty_and_foreign_files() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("raw.bin");
        fs::write(&output, b"12345").unwrap();
        assert_eq!(validate_output(directory.path(), &output, 5).unwrap(), 5);
        assert_eq!(
            validate_output(directory.path(), &output, 4)
                .unwrap_err()
                .analysis_code(),
            AnalysisErrorCode::InvalidMetadata
        );
        fs::write(&output, b"").unwrap();
        assert!(validate_output(directory.path(), &output, 5).is_err());
        fs::write(directory.path().join("extra"), b"unexpected").unwrap();
        fs::write(&output, b"1").unwrap();
        assert!(validate_output(directory.path(), &output, 5).is_err());
    }

    #[test]
    fn canceled_and_expired_requests_fail_before_work() {
        let context = request_context();
        context.cancellation.cancel();
        assert_eq!(
            check_context(&context).unwrap_err().analysis_code(),
            AnalysisErrorCode::ProcessFailed
        );
        let mut expired = request_context();
        expired.deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            check_context(&expired).unwrap_err().analysis_code(),
            AnalysisErrorCode::AnalysisTimeout
        );
    }

    #[test]
    fn disk_full_and_denied_tool_failures_keep_meaningful_errors() {
        assert!(matches!(
            exit_error(b"No space left on device"),
            HlsTransferError::Io(std::io::ErrorKind::StorageFull)
        ));
        for (stderr, expected) in [
            (
                "HTTP error 401 Unauthorized",
                AnalysisErrorCode::AuthenticationRequired,
            ),
            ("HTTP error 403 Forbidden", AnalysisErrorCode::AccessDenied),
            (
                "HTTP error 404 Not Found",
                AnalysisErrorCode::ContentUnavailable,
            ),
            (
                "HTTP error 429 Too Many Requests",
                AnalysisErrorCode::RateLimited,
            ),
        ] {
            assert_eq!(exit_error(stderr.as_bytes()).analysis_code(), expected);
        }
    }

    struct SyntheticProcess {
        bytes: &'static [u8],
        wait_for_cancel: bool,
    }

    impl ProcessSupervisor for SyntheticProcess {
        fn run(
            &self,
            request: ProcessRequest,
            cancellation: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<crate::application::process::ProcessExit, ProcessFailureKind> {
            let output = Path::new(request.args.last().unwrap());
            fs::write(output, self.bytes).unwrap();
            if self.wait_for_cancel {
                let limit = Instant::now() + Duration::from_secs(2);
                while !cancellation.is_canceled() && Instant::now() < limit {
                    thread::sleep(Duration::from_millis(1));
                }
                assert!(
                    cancellation.is_canceled(),
                    "watcher must terminate the child on cancellation or overflow"
                );
                return Err(ProcessFailureKind::Canceled);
            }
            Ok(crate::application::process::ProcessExit {
                code: 0,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }

    fn synthetic_tool(directory: &Path) -> FfmpegHttpTool {
        let path = directory.join("fixture-tool");
        fs::write(&path, b"synthetic verified tool").unwrap();
        FfmpegHttpTool {
            path,
            version: "internal-test-only".into(),
            digest: format!("{:x}", Sha256::digest(b"synthetic verified tool")),
        }
    }

    #[test]
    fn scoped_transfer_publishes_verified_raw_bytes_and_keeps_preexisting_caller_file() {
        let directory = tempfile::tempdir().unwrap();
        let tool = synthetic_tool(directory.path());
        let destination = directory.path().join("asset");
        let url = "https://video.twimg.com/fixture/segment.m4s"
            .parse()
            .unwrap();
        let process = SyntheticProcess {
            bytes: b"raw bytes",
            wait_for_cancel: false,
        };
        let request = request_context();
        assert_eq!(
            fetch_with(
                &tool,
                &process,
                &url,
                &destination,
                32,
                false,
                Duration::from_secs(8),
                &request
            )
            .unwrap(),
            9
        );
        assert_eq!(fs::read(&destination).unwrap(), b"raw bytes");
        fs::write(&destination, b"caller-owned").unwrap();
        assert!(matches!(
            fetch_with(
                &tool,
                &process,
                &url,
                &destination,
                32,
                false,
                Duration::from_secs(8),
                &request
            ),
            Err(HlsTransferError::Io(std::io::ErrorKind::AlreadyExists))
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"caller-owned");
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            2,
            "private staging directories must be removed"
        );
    }

    #[test]
    fn packet_rounding_is_trimmed_only_for_probe_and_never_for_complete_transfer() {
        let directory = tempfile::tempdir().unwrap();
        let tool = synthetic_tool(directory.path());
        let destination = directory.path().join("prefix");
        let url = "https://video.twimg.com/fixture/segment.m4s"
            .parse()
            .unwrap();
        let process = SyntheticProcess {
            bytes: b"raw bytes",
            wait_for_cancel: false,
        };
        let count = fetch_with(
            &tool,
            &process,
            &url,
            &destination,
            5,
            true,
            Duration::from_secs(8),
            &request_context(),
        )
        .unwrap();
        assert_eq!(count, 5);
        assert_eq!(fs::read(&destination).unwrap(), b"raw b");
        let complete = directory.path().join("complete");
        assert_eq!(
            fetch_with(
                &tool,
                &process,
                &url,
                &complete,
                5,
                false,
                Duration::from_secs(8),
                &request_context()
            )
            .unwrap_err()
            .analysis_code(),
            AnalysisErrorCode::InvalidMetadata
        );
        assert!(!complete.exists());
    }

    #[test]
    fn watcher_bounds_ongoing_child_bytes_and_removes_staging_output() {
        let directory = tempfile::tempdir().unwrap();
        let tool = synthetic_tool(directory.path());
        let destination = directory.path().join("asset");
        let url = "https://video.twimg.com/fixture/segment.m4s"
            .parse()
            .unwrap();
        let process = SyntheticProcess {
            bytes: b"oversized",
            wait_for_cancel: true,
        };
        let result = fetch_with(
            &tool,
            &process,
            &url,
            &destination,
            2,
            false,
            Duration::from_secs(8),
            &request_context(),
        );
        assert_eq!(
            result.unwrap_err().analysis_code(),
            AnalysisErrorCode::InvalidMetadata
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn watcher_propagates_parent_cancellation_to_running_child() {
        let directory = tempfile::tempdir().unwrap();
        let tool = synthetic_tool(directory.path());
        let destination = directory.path().join("asset");
        let url = "https://video.twimg.com/fixture/segment.m4s"
            .parse()
            .unwrap();
        let process = SyntheticProcess {
            bytes: b"raw",
            wait_for_cancel: true,
        };
        let request = request_context();
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(20));
                request.cancellation.cancel();
            });
            let result = fetch_with(
                &tool,
                &process,
                &url,
                &destination,
                32,
                false,
                Duration::from_secs(8),
                &request,
            );
            assert_eq!(
                result.unwrap_err().analysis_code(),
                AnalysisErrorCode::ProcessFailed
            );
        });
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn tool_revalidation_rejects_changed_binary_before_launch() {
        let directory = tempfile::tempdir().unwrap();
        let tool = synthetic_tool(directory.path());
        fs::write(&tool.path, b"replaced tool").unwrap();
        assert_eq!(
            tool.revalidate(&request_context())
                .unwrap_err()
                .analysis_code(),
            AnalysisErrorCode::ToolIntegrityFailed
        );
    }

    fn request_context() -> XRequestContext {
        XRequestContext::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(1),
            Arc::new(crate::infrastructure::system_proxy::SystemProxies::default().snapshot()),
        )
    }
}
