//! Ephemeral anonymous inventory from the pinned extractor's response bytes.
//! Video URLs here identify media only; the existing video analyzer still
//! selects and verifies its full highest-quality formats separately.
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use serde_json::Value;

use crate::{
    application::{
        media_url::ValidatedMediaUrl,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessProgram, ProcessRequest, ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        instagram::{InstagramMediaKind, InstagramUrlKind},
        managed_tools::{ManagedToolErrorCode, ToolName},
        media::{AnalysisErrorCode, Platform},
    },
    infrastructure::{
        gallery_dl::{InstagramInventory, InstagramInventoryExtractor},
        instagram_web::parse_web_response,
    },
};

const FILE_LIMIT: u64 = 2 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 8 * 1024 * 1024;
const COUNT_LIMIT: usize = 8;
const STDOUT_LIMIT: usize = 2 * 1024 * 1024;
const STDERR_LIMIT: usize = 256 * 1024;

pub struct YtDlpInstagramInventoryExtractor<R, P> {
    resolver: R,
    process: P,
}

impl<R, P> YtDlpInstagramInventoryExtractor<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}

pub struct ToolFallbackInstagramInventoryExtractor<N, T> {
    native: N,
    tool: T,
}

impl<N, T> ToolFallbackInstagramInventoryExtractor<N, T> {
    pub fn new(native: N, tool: T) -> Self {
        Self { native, tool }
    }
}

impl<N: InstagramInventoryExtractor, T: InstagramInventoryExtractor> InstagramInventoryExtractor
    for ToolFallbackInstagramInventoryExtractor<N, T>
{
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        validate_source(source)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        match self.native.extract_post(source, cancellation) {
            Err(
                AnalysisErrorCode::NetworkFailed
                | AnalysisErrorCode::AnalysisTimeout
                | AnalysisErrorCode::ExtractorOutdated
                | AnalysisErrorCode::AuthenticationRequired,
            ) if !cancellation.is_canceled() => self.tool.extract_post(source, cancellation),
            result => result,
        }
    }
}

impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync>
    InstagramInventoryExtractor for YtDlpInstagramInventoryExtractor<R, P>
{
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        validate_source(source)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let tool =
            self.resolver
                .resolve_verified(ToolName::YtDlp)
                .map_err(|error| match error.code {
                    ManagedToolErrorCode::ToolMissing => AnalysisErrorCode::ToolMissing,
                    ManagedToolErrorCode::ToolIntegrityFailed
                    | ManagedToolErrorCode::InvalidToolManifest
                    | ManagedToolErrorCode::ToolVersionMismatch => {
                        AnalysisErrorCode::ToolIntegrityFailed
                    }
                    _ => AnalysisErrorCode::ProcessFailed,
                })?;
        if tool.version != "2026.08.19" {
            return Err(AnalysisErrorCode::ToolIntegrityFailed);
        }
        let working = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let mut args: Vec<OsString> = [
            "--ignore-config",
            "--no-remote-components",
            "--no-plugin-dirs",
            "--simulate",
            "--dump-single-json",
            "--write-pages",
            "--no-playlist",
            "--quiet",
            "--no-progress",
            "--color",
            "never",
            "--encoding",
            "utf-8",
            "--no-cookies",
            "--no-cookies-from-browser",
            "--no-update",
            "--no-cache-dir",
            "--no-check-formats",
            "--socket-timeout",
            "15",
            "--retries",
            "0",
            "--extractor-retries",
            "0",
            "--use-extractors",
            "Instagram$",
            "--impersonate",
            super::ytdlp::INSTAGRAM_BROWSER_PROFILE,
            "--",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(source.url().into());
        let exit = run_with_budget(working.path(), cancellation, |child_cancellation| {
            self.process
                .run(
                    ProcessRequest {
                        program: ProcessProgram::from_resolved_tool(tool)
                            .with_explicit_https_proxy_for_host("www.instagram.com"),
                        args,
                        working_dir: working.path().into(),
                        timeout: Some(Duration::from_secs(45)),
                        inactivity_timeout: None,
                        output_mode: OutputMode::Capture {
                            stdout_limit: STDOUT_LIMIT,
                            stderr_limit: STDERR_LIMIT,
                        },
                    },
                    child_cancellation,
                    &Discard,
                )
                .map_err(map_process_error)
        })?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        if exit.stdout.len() > STDOUT_LIMIT || exit.stderr.len() > STDERR_LIMIT {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        // A complete response cannot override an explicit denial or rate limit.
        if let Some(error) = terminal_failure(&exit.stderr) {
            return Err(error);
        }
        let inventory = parse_witness(source, working.path())?;
        validate_exit(source, &exit, &inventory)?;
        Ok(inventory)
    }
}

fn validate_source(source: &ValidatedMediaUrl) -> Result<(), AnalysisErrorCode> {
    if source.platform() == Platform::Instagram
        && source.instagram_kind() == Some(InstagramUrlKind::Post)
    {
        Ok(())
    } else {
        Err(AnalysisErrorCode::UnsupportedMedia)
    }
}

fn inventory_files(directory: &Path) -> Result<Vec<PathBuf>, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let mut paths = Vec::new();
    let mut total = 0_u64;
    for entry in fs::read_dir(directory).map_err(|_| invalid)? {
        let path = entry.map_err(|_| invalid)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| invalid)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || reparse(&metadata)
            || path.extension().and_then(|s| s.to_str()) != Some("dump")
            || metadata.len() > FILE_LIMIT
            || paths.len() == COUNT_LIMIT
        {
            return Err(invalid);
        }
        total = total.checked_add(metadata.len()).ok_or(invalid)?;
        if total > TOTAL_LIMIT {
            return Err(invalid);
        }
        paths.push(path);
    }
    Ok(paths)
}

fn run_with_budget(
    directory: &Path,
    cancellation: &CancellationToken,
    run: impl FnOnce(CancellationToken) -> Result<ProcessExit, AnalysisErrorCode>,
) -> Result<ProcessExit, AnalysisErrorCode> {
    let exceeded = AtomicBool::new(false);
    let child_cancellation = CancellationToken::new();
    if cancellation.is_canceled() {
        return Err(AnalysisErrorCode::ProcessFailed);
    }
    std::thread::scope(|scope| {
        let (stop, receiver) = mpsc::channel();
        let exceeded_ref = &exceeded;
        let child_cancel = child_cancellation.clone();
        let watcher = scope.spawn(move || {
            while receiver.recv_timeout(Duration::from_millis(50))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                if cancellation.is_canceled() {
                    child_cancel.cancel();
                    break;
                }
                if inventory_files(directory).is_err() {
                    exceeded_ref.store(true, Ordering::Release);
                    child_cancel.cancel();
                    break;
                }
            }
        });
        let result = run(child_cancellation);
        let _ = stop.send(());
        watcher
            .join()
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        if exceeded.load(Ordering::Acquire) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        inventory_files(directory)?;
        result
    })
}

fn parse_witness(
    source: &ValidatedMediaUrl,
    directory: &Path,
) -> Result<InstagramInventory, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let mut found = None;
    for path in inventory_files(directory)? {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(|_| invalid)?
            .take(FILE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid)?;
        if bytes.len() as u64 > FILE_LIMIT {
            return Err(invalid);
        }
        // Homepage and unrelated API dumps are not inventory witnesses. Select
        // by response shape, never by a URL-derived filesystem name.
        let Ok(root) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if root.pointer("/data/xig_polaris_media").is_none()
            && root.pointer("/data/xdt_shortcode_media").is_none()
            && root
                .pointer("/data/xdt_api__v1__media__shortcode__web_info")
                .is_none()
            && root.get("items").is_none()
        {
            continue;
        }
        if found.is_some() {
            return Err(invalid);
        }
        found = Some(parse_web_response(source, &bytes)?);
    }
    found.ok_or(AnalysisErrorCode::ExtractorOutdated)
}

fn validate_exit(
    source: &ValidatedMediaUrl,
    exit: &ProcessExit,
    inventory: &InstagramInventory,
) -> Result<(), AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if exit.code == 0 {
        validate_metadata(source, &exit.stdout, inventory)?;
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&exit.stderr).to_ascii_lowercase();
    let errors = stderr
        .lines()
        .filter(|line| line.trim_start().starts_with("error:"))
        .collect::<Vec<_>>();
    if exit.code != 1
        || errors.is_empty()
        || !errors.iter().all(|line| {
            line.contains("there is no video in this post")
                || line.contains("no video formats found")
        })
    {
        return Err(AnalysisErrorCode::ProcessFailed);
    }
    if inventory
        .items()
        .iter()
        .all(|item| item.public_item().kind == InstagramMediaKind::Image)
    {
        return Ok(());
    }
    // yt-dlp reports image carousel entries as no-video errors. The separately
    // validated full response owns the inventory; require successful metadata
    // for every video ordinal before accepting that specific mixed-post exit.
    let metadata = validate_metadata(source, &exit.stdout, inventory)?;
    let entries = metadata
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(invalid)?;
    for (item, entry) in inventory.items().iter().zip(entries) {
        if item.public_item().kind == InstagramMediaKind::Video
            && entry
                .get("formats")
                .and_then(Value::as_array)
                .is_none_or(|rows| rows.is_empty())
        {
            return Err(invalid);
        }
    }
    Ok(())
}

fn validate_metadata(
    source: &ValidatedMediaUrl,
    stdout: &[u8],
    inventory: &InstagramInventory,
) -> Result<Value, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let metadata: Value = serde_json::from_slice(stdout).map_err(|_| invalid)?;
    if metadata.get("extractor_key").and_then(Value::as_str) != Some("Instagram") {
        return Err(invalid);
    }
    if metadata.get("webpage_url").and_then(Value::as_str) != Some(source.url()) {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    if inventory.items().len() > 1 && metadata.get("entries").and_then(Value::as_array).is_none() {
        return Err(invalid);
    }
    if metadata
        .get("playlist_count")
        .is_some_and(|n| n.as_u64() != Some(inventory.items().len() as u64))
        || metadata.get("entries").is_some_and(|entries| {
            entries
                .as_array()
                .is_none_or(|entries| entries.len() != inventory.items().len())
        })
    {
        return Err(invalid);
    }
    Ok(metadata)
}

fn terminal_failure(stderr: &[u8]) -> Option<AnalysisErrorCode> {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if has(&["http error 429", "too many requests", "rate limit"]) {
        Some(AnalysisErrorCode::RateLimited)
    } else if has(&["http error 403", "forbidden", "access denied"]) {
        Some(AnalysisErrorCode::AccessDenied)
    } else if has(&[
        "has been removed",
        "content is unavailable",
        "video unavailable",
        "video is unavailable",
        "deleted video",
        "private video",
        "video is private",
    ]) {
        Some(AnalysisErrorCode::ContentUnavailable)
    } else if has(&[
        "login required",
        "authentication required",
        "only available for registered users",
        "redirected to the login page",
        "use --cookies",
    ]) {
        Some(AnalysisErrorCode::AuthenticationRequired)
    } else {
        None
    }
}

fn map_process_error(error: ProcessFailureKind) -> AnalysisErrorCode {
    match error {
        ProcessFailureKind::TimedOut | ProcessFailureKind::InactivityTimedOut => {
            AnalysisErrorCode::AnalysisTimeout
        }
        ProcessFailureKind::StdoutLimitExceeded
        | ProcessFailureKind::StderrLimitExceeded
        | ProcessFailureKind::LineLimitExceeded => AnalysisErrorCode::InvalidMetadata,
        _ => AnalysisErrorCode::ProcessFailed,
    }
}

#[cfg(windows)]
fn reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}
#[cfg(not(windows))]
fn reparse(_: &fs::Metadata) -> bool {
    false
}

struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _: ProcessStream, _: &[u8]) {}
}

#[cfg(test)]
#[path = "instagram_tool_tests.rs"]
mod tests;
