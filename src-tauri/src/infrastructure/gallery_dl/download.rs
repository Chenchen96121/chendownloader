use std::{ffi::OsString, fs, path::Path, time::Duration};

use crate::{
    application::{
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest,
            ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        download::DownloadErrorCode, instagram::InstagramMediaKind, managed_tools::ToolName,
        media::AnalysisErrorCode,
    },
};

use super::{
    errors, metadata::verify_acquired_item_identity, GalleryDlInstagramAdapter,
    ResolvedInstagramItem,
};

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const STDOUT_LIMIT: usize = 32 * 1024;
const STDERR_LIMIT: usize = 256 * 1024;
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;

use crate::infrastructure::image_verification::{
    ImageStaging, VerifiedImage as VerifiedInstagramImage,
};

pub(super) fn fetch_preview<R, P>(
    adapter: &GalleryDlInstagramAdapter<R, P>,
    item: &ResolvedInstagramItem,
    protected_directory: &Path,
    cancellation: &CancellationToken,
) -> Result<Option<VerifiedInstagramImage>, AnalysisErrorCode>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    if !item.public.preview_available {
        return Ok(None);
    }
    let result = acquire(adapter, item, protected_directory, cancellation, true)
        .map_err(AcquireFailure::analysis)?;
    if result.byte_length() > MAX_PREVIEW_BYTES || result.mime_type().is_none() {
        let _ = fs::remove_file(result.path());
        return Ok(None);
    }
    Ok(Some(result))
}

pub(super) fn download_image<R, P>(
    adapter: &GalleryDlInstagramAdapter<R, P>,
    item: &ResolvedInstagramItem,
    protected_directory: &Path,
    cancellation: &CancellationToken,
) -> Result<VerifiedInstagramImage, DownloadErrorCode>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    if item.public.kind != InstagramMediaKind::Image {
        return Err(DownloadErrorCode::InvalidRequest);
    }
    acquire(adapter, item, protected_directory, cancellation, false)
        .map_err(AcquireFailure::download)
}

pub(crate) fn acquire_direct_image<R, P, F>(
    resolver: &R,
    process: &P,
    item: &ResolvedInstagramItem,
    protected_directory: &Path,
    cancellation: &CancellationToken,
    preview: bool,
    fetch: F,
) -> Result<VerifiedInstagramImage, AcquireFailure>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
    F: FnOnce(&Path, u64) -> Result<(), AnalysisErrorCode>,
{
    if cancellation.is_canceled() {
        return Err(AcquireFailure::Process);
    }
    if (!preview && item.public.kind != InstagramMediaKind::Image)
        || item.media_id.is_empty()
        || item.display_url.scheme() != "https"
        || item.display_url.host_str().is_none()
    {
        return Err(AcquireFailure::InvalidOutput);
    }
    let staging = ImageStaging::new(protected_directory).map_err(AcquireFailure::Verification)?;
    let working_dir = staging.directory().to_owned();
    let staged = working_dir.join("stage.download");
    let maximum_bytes = if preview {
        MAX_PREVIEW_BYTES
    } else {
        MAX_IMAGE_BYTES
    };
    fetch(&staged, maximum_bytes).map_err(AcquireFailure::Analysis)?;
    if cancellation.is_canceled() {
        return Err(AcquireFailure::Process);
    }
    staging
        .verify(
            resolver,
            process,
            if item.public.kind == InstagramMediaKind::Image {
                item.public.width
            } else {
                None
            },
            if item.public.kind == InstagramMediaKind::Image {
                item.public.height
            } else {
                None
            },
            maximum_bytes,
            cancellation,
        )
        .map_err(AcquireFailure::Verification)
}

fn acquire<R, P>(
    adapter: &GalleryDlInstagramAdapter<R, P>,
    item: &ResolvedInstagramItem,
    protected_directory: &Path,
    cancellation: &CancellationToken,
    preview: bool,
) -> Result<VerifiedInstagramImage, AcquireFailure>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    if cancellation.is_canceled() {
        return Err(AcquireFailure::Process);
    }
    if item.media_id.is_empty()
        || item.display_url.scheme() != "https"
        || item.display_url.host_str().is_none()
    {
        return Err(AcquireFailure::InvalidOutput);
    }
    let staging = ImageStaging::new(protected_directory).map_err(AcquireFailure::Verification)?;
    let working_dir = staging.directory().to_owned();
    let gallery_dl = adapter
        .resolver
        .resolve_verified(ToolName::GalleryDl)
        .map_err(AcquireFailure::Tool)?;
    let ordinal = item.public.ordinal.to_string();
    let mut args = vec![
        OsString::from("--ignore-config"),
        OsString::from("--no-skip"),
        OsString::from("--quiet"),
        OsString::from("--no-input"),
        OsString::from("--no-mtime"),
        OsString::from("--no-part"),
        OsString::from("--range"),
        OsString::from(ordinal),
        OsString::from("--directory"),
        OsString::from("."),
        OsString::from("--filename"),
        OsString::from("stage.{extension}"),
        OsString::from("--postprocessor"),
        OsString::from("metadata"),
        OsString::from("--postprocessor-option"),
        OsString::from("mode=jsonl"),
        OsString::from("--postprocessor-option"),
        OsString::from("event=prepare"),
        OsString::from("--postprocessor-option"),
        OsString::from("filename=-"),
    ];
    if preview {
        args.extend([
            OsString::from("--option"),
            OsString::from("extractor.instagram.videos=false"),
        ]);
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(&item.source_page_url));
    let exit = adapter
        .process
        .run(
            ProcessRequest {
                program: ProcessProgram::from_resolved_tool(gallery_dl),
                args,
                working_dir: working_dir.clone(),
                timeout: Some(DOWNLOAD_TIMEOUT),
                inactivity_timeout: Some(Duration::from_secs(30)),
                output_mode: OutputMode::Capture {
                    stdout_limit: STDOUT_LIMIT,
                    stderr_limit: STDERR_LIMIT,
                },
            },
            cancellation.clone(),
            &NoopSink,
        )
        .map_err(AcquireFailure::ProcessKind)?;
    if cancellation.is_canceled() {
        return Err(AcquireFailure::Process);
    }
    if exit.code != 0 {
        return Err(AcquireFailure::Classified(exit.stderr));
    }
    verify_acquired_item_identity(item, &exit.stdout).map_err(|error| match error {
        AnalysisErrorCode::ContentChanged => AcquireFailure::ContentChanged,
        _ => AcquireFailure::InvalidOutput,
    })?;

    staging
        .verify(
            &adapter.resolver,
            &adapter.process,
            if item.public.kind == InstagramMediaKind::Image {
                item.public.width
            } else {
                None
            },
            if item.public.kind == InstagramMediaKind::Image {
                item.public.height
            } else {
                None
            },
            MAX_IMAGE_BYTES,
            cancellation,
        )
        .map_err(AcquireFailure::Verification)
}

pub(crate) enum AcquireFailure {
    Tool(crate::domain::managed_tools::ManagedToolError),
    ProcessKind(crate::application::process::ProcessFailureKind),
    Classified(Vec<u8>),
    Process,
    Verification(DownloadErrorCode),
    ContentChanged,
    InvalidOutput,
    Analysis(AnalysisErrorCode),
}

impl AcquireFailure {
    pub(crate) fn analysis(self) -> AnalysisErrorCode {
        match self {
            Self::Verification(error) => match error {
                DownloadErrorCode::ToolMissing => AnalysisErrorCode::ToolMissing,
                DownloadErrorCode::ToolIntegrityFailed => AnalysisErrorCode::ToolIntegrityFailed,
                DownloadErrorCode::DownloadTimeout => AnalysisErrorCode::AnalysisTimeout,
                DownloadErrorCode::ProcessFailed => AnalysisErrorCode::ProcessFailed,
                _ => AnalysisErrorCode::InvalidMetadata,
            },
            Self::Tool(error) => errors::analysis_tool(error),
            Self::ProcessKind(error) => errors::analysis_process(error),
            Self::Classified(stderr) => errors::classify_analysis(&stderr),
            Self::Process => AnalysisErrorCode::ProcessFailed,
            Self::ContentChanged => AnalysisErrorCode::ContentChanged,
            Self::InvalidOutput => AnalysisErrorCode::InvalidMetadata,
            Self::Analysis(error) => error,
        }
    }
    pub(crate) fn download(self) -> DownloadErrorCode {
        match self {
            Self::Verification(error) => error,
            Self::Tool(error) => errors::download_tool(error),
            Self::ProcessKind(error) => errors::download_process(error),
            Self::Classified(stderr) => errors::classify_download(&stderr),
            Self::Process => DownloadErrorCode::ProcessFailed,
            Self::ContentChanged => DownloadErrorCode::ContentChanged,
            Self::InvalidOutput => DownloadErrorCode::OutputInvalid,
            Self::Analysis(error) => match error {
                AnalysisErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
                AnalysisErrorCode::ToolIntegrityFailed => DownloadErrorCode::ToolIntegrityFailed,
                AnalysisErrorCode::AuthenticationRequired => {
                    DownloadErrorCode::AuthenticationRequired
                }
                AnalysisErrorCode::AccessDenied => DownloadErrorCode::AccessDenied,
                AnalysisErrorCode::ContentUnavailable => DownloadErrorCode::ContentUnavailable,
                AnalysisErrorCode::ContentChanged => DownloadErrorCode::ContentChanged,
                AnalysisErrorCode::RateLimited => DownloadErrorCode::RateLimited,
                AnalysisErrorCode::ExtractorOutdated => DownloadErrorCode::ExtractorOutdated,
                AnalysisErrorCode::NetworkFailed => DownloadErrorCode::NetworkFailed,
                AnalysisErrorCode::AnalysisTimeout => DownloadErrorCode::DownloadTimeout,
                AnalysisErrorCode::FormatUnavailable => DownloadErrorCode::FormatUnavailable,
                AnalysisErrorCode::ProcessFailed => DownloadErrorCode::ProcessFailed,
                AnalysisErrorCode::InvalidMetadata => DownloadErrorCode::OutputInvalid,
                AnalysisErrorCode::AnalysisUnavailable | AnalysisErrorCode::AnalysisExpired => {
                    DownloadErrorCode::AnalysisUnavailable
                }
                _ => DownloadErrorCode::Unknown,
            },
        }
    }
}

struct NoopSink;
impl ProcessEventSink for NoopSink {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
}
