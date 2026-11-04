use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use crate::{
    application::{
        download_executor::{
            DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink, StagedDownload,
        },
        local_processing::{DownloadedSource, LocalMediaProcessor, LocalProcessingRequest},
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram,
            ProcessRequest, ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        download::{
            valid_opaque_id, DownloadErrorCode, DownloadMessageKey, DownloadProgress,
            DownloadProgressMetrics, DownloadState,
        },
        format_selection::{SelectionMode, SelectionOperation},
        instagram::InstagramUrlKind,
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
        media::{AnalysisErrorCode, Platform},
    },
};

use super::progress::ProgressParser;
use crate::infrastructure::x::hls_local::{
    materialize_hls, HlsMaterializationRequest, HlsReadMode, HlsSegmentTransport,
    HttpHlsSegmentTransport,
};

const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
const LINE_LIMIT: usize = 4096;
const STDERR_LIMIT: usize = 65_536;

pub struct YtDlpDownloadExecutor<R, P, C, T = HttpHlsSegmentTransport> {
    resolver: R,
    process: P,
    processor: C,
    segments: T,
}

impl<R, P, C> YtDlpDownloadExecutor<R, P, C> {
    pub fn new(resolver: R, process: P, processor: C) -> Self {
        Self {
            resolver,
            process,
            processor,
            segments: HttpHlsSegmentTransport,
        }
    }
}
impl<R, P, C, T> YtDlpDownloadExecutor<R, P, C, T> {
    pub fn with_hls_transport(resolver: R, process: P, processor: C, segments: T) -> Self {
        Self {
            resolver,
            process,
            processor,
            segments,
        }
    }
}

impl<R, P, C, T> DownloadExecutor for YtDlpDownloadExecutor<R, P, C, T>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
    C: LocalMediaProcessor + Send + Sync,
    T: HlsSegmentTransport,
{
    fn download(
        &self,
        request: DownloadExecutionRequest,
        cancellation: CancellationToken,
        progress: &dyn DownloadProgressSink,
    ) -> Result<StagedDownload, DownloadErrorCode> {
        validate_request(&request)?;
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        let mut workspace = SourceWorkspace::create(&request.job_id, &request.staging_dir)?;
        let mut yt_dlp = Some(
            self.resolver
                .resolve_verified(ToolName::YtDlp)
                .map_err(map_tool_error)?,
        );
        let deno = self
            .resolver
            .resolve_verified(ToolName::Deno)
            .map_err(map_tool_error)?;
        let ffmpeg = self
            .resolver
            .resolve_verified(ToolName::Ffmpeg)
            .map_err(map_tool_error)?;
        let _ffprobe = self
            .resolver
            .resolve_verified(ToolName::Ffprobe)
            .map_err(map_tool_error)?;

        let sink = StructuredProgressSink::new(
            progress,
            cancellation.clone(),
            request.plan.source_format_ids.len(),
        );
        let mut sources = Vec::new();
        for (slot, format_id) in request.plan.source_format_ids.iter().enumerate() {
            if slot > 0 {
                sink.parser
                    .lock()
                    .map_err(|_| DownloadErrorCode::OutputInvalid)?
                    .begin_source()?;
            }
            if cancellation.is_canceled() {
                return Err(DownloadErrorCode::ProcessFailed);
            }
            workspace.revalidate()?;
            let working_dir = workspace.source_dir.join(format!("input-{slot}"));
            fs::create_dir(&working_dir).map_err(map_workspace_io)?;
            let hls = request
                .x_source
                .as_ref()
                .map(|source| source.hls_input(format_id))
                .transpose()?
                .flatten();
            let bilibili_info = request
                .bilibili_source
                .as_ref()
                .map(|source| {
                    super::bilibili_download::SealedInfoFile::create(
                        &workspace.source_dir,
                        source.format_info_json(format_id)?,
                    )
                })
                .transpose()?;
            let info = if let Some(source) = request.x_source.as_ref().filter(|_| hls.is_none()) {
                let bytes = source.format_info_json(&request.plan, format_id)?;
                let mut file = tempfile::NamedTempFile::new_in(&workspace.source_dir)
                    .map_err(map_workspace_io)?;
                file.write_all(&bytes).map_err(map_workspace_io)?;
                file.as_file().sync_all().map_err(map_workspace_io)?;
                verify_info_file(file.path(), &workspace.source_dir, &bytes)?;
                Some(file)
            } else {
                None
            };
            let local_hls = if let Some(snapshot) = &hls {
                snapshot
                    .revalidate()
                    .map_err(|_| DownloadErrorCode::OutputInvalid)?;
                progress.on_progress(DownloadProgress::new(
                    &request.job_id,
                    DownloadState::Downloading,
                    DownloadProgressMetrics::default(),
                    DownloadMessageKey::Downloading,
                )?);
                let local = materialize_hls(
                    HlsMaterializationRequest {
                        snapshot,
                        parent: &working_dir,
                        mode: HlsReadMode::Complete,
                        network: request
                            .x_source
                            .as_ref()
                            .ok_or(DownloadErrorCode::InvalidRequest)?
                            .network(),
                        cancellation: cancellation.clone(),
                        deadline: None,
                    },
                    &self.segments,
                )
                .map_err(map_hls_error)?;
                if !local.is_complete() {
                    return Err(DownloadErrorCode::OutputInvalid);
                }
                local
                    .revalidate(&cancellation)
                    .map_err(map_analysis_error)?;
                Some(local)
            } else {
                None
            };
            let (program, args) = if let Some(local) = &local_hls {
                (
                    ProcessProgram::from_resolved_tool(
                        self.resolver
                            .resolve_verified(ToolName::Ffmpeg)
                            .map_err(map_tool_error)?,
                    ),
                    build_hls_args(&request, format_id, local.input())?,
                )
            } else {
                let args = build_args(
                    &request,
                    &deno.path,
                    &ffmpeg.path,
                    "source.%(ext)s",
                    format_id,
                    info.as_ref()
                        .map(|file| file.path())
                        .or_else(|| bilibili_info.as_ref().map(|file| file.path())),
                )?;
                let mut program = ProcessProgram::from_resolved_tool(match yt_dlp.take() {
                    Some(tool) => tool,
                    None => self
                        .resolver
                        .resolve_verified(ToolName::YtDlp)
                        .map_err(map_tool_error)?,
                });
                if let Some(source) = &request.x_source {
                    program = program.with_network_profile(source.network());
                }
                if let Some(source) = &request.bilibili_source {
                    program = program.with_network_profile(source.network());
                }
                if request.source.platform() == Platform::Instagram {
                    program = program.with_explicit_https_proxy_for_host("www.instagram.com");
                }
                (program, args)
            };
            if let Some(info) = &bilibili_info {
                info.verify()?;
            }
            let result = self.process.run(
                ProcessRequest {
                    stdin: None,
                    program,
                    args,
                    working_dir,
                    timeout: None,
                    inactivity_timeout: Some(INACTIVITY_TIMEOUT),
                    output_mode: OutputMode::Lines {
                        line_limit: LINE_LIMIT,
                        retained_stderr_limit: STDERR_LIMIT,
                    },
                },
                cancellation.clone(),
                &sink,
            );
            // Release fragment storage before the existing final copy/remux.
            drop(local_hls);
            if let Some(error) = sink.error() {
                return Err(error);
            }
            let exit = result.map_err(map_process_error)?;
            if exit.code != 0 {
                if request.bilibili_source.is_some() {
                    return Err(super::bilibili_download::classify_transfer_error(
                        &exit.stderr,
                    ));
                }
                if request.playlist_item.is_some() && is_changed_post_video(&exit.stderr) {
                    return Err(DownloadErrorCode::ContentChanged);
                }
                return Err(map_nonzero_exit(&exit.stderr));
            }
            if cancellation.is_canceled() {
                return Err(DownloadErrorCode::ProcessFailed);
            }
            sources.push(DownloadedSource {
                format_id: format_id.clone(),
                slot,
            });
        }
        workspace.revalidate()?;
        progress.on_progress(DownloadProgress::new(
            &request.job_id,
            DownloadState::Processing,
            DownloadProgressMetrics::default(),
            DownloadMessageKey::Processing,
        )?);
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        let processing_request = LocalProcessingRequest {
            job_id: request.job_id.clone(),
            staging_dir: workspace.staging_dir.clone(),
            source_dir: workspace.source_dir.clone(),
            plan: request.plan.clone(),
            video_source: request.video_source.clone(),
            audio_source: request.audio_source.clone(),
            sources,
        };
        let bilibili_guards = request
            .bilibili_source
            .as_ref()
            .map(|source| {
                super::bilibili_download::verify_downloaded_sources(
                    source,
                    &processing_request,
                    &self.resolver,
                    &self.process,
                    &cancellation,
                )
            })
            .transpose()?;
        let mut processed = self
            .processor
            .process(processing_request, cancellation.clone())?;
        if let Some(local) = &bilibili_guards {
            super::bilibili_download::verify_processing_evidence(
                local,
                &request,
                &mut processed.evidence,
            )?;
        }
        drop(bilibili_guards);
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        workspace.cleanup()?;

        Ok(StagedDownload {
            job_id: request.job_id,
            staging_dir: workspace.staging_dir.clone(),
            expected_operation: request.plan.operation,
            expected_container: Some(processed.evidence.actual_container.as_str().into()),
            requested_clip: request.plan.clip,
            evidence: processed.evidence,
        })
    }
}

fn verify_info_file(path: &Path, parent: &Path, expected: &[u8]) -> Result<(), DownloadErrorCode> {
    let metadata = fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || is_reparse_point(&metadata)
        || metadata.len() != expected.len() as u64
        || path
            .canonicalize()
            .map_err(|_| DownloadErrorCode::OutputInvalid)?
            .parent()
            != Some(parent)
        || fs::read(path).map_err(|_| DownloadErrorCode::OutputInvalid)? != expected
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(())
}

fn build_hls_args(
    request: &DownloadExecutionRequest,
    format_id: &str,
    input: &Path,
) -> Result<Vec<OsString>, DownloadErrorCode> {
    let source = request
        .x_source
        .as_ref()
        .ok_or(DownloadErrorCode::InvalidRequest)?;
    let format = source.format(format_id)?;
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-v",
        "error",
        "-nostdin",
        "-nostats",
        "-n",
        "-progress",
        "pipe:1",
        "-protocol_whitelist",
        "file",
        "-format_whitelist",
        "hls,mpegts,mov,aac,webvtt",
        "-probesize",
        "8388608",
        "-analyzeduration",
        "3000000",
    ]
    .map(Into::into)
    .to_vec();
    args.extend(["-i".into(), input.as_os_str().to_owned()]);
    if format.kind != crate::domain::media::StreamKind::AudioOnly {
        args.extend(["-map".into(), "0:v:0".into()]);
    }
    if format.kind != crate::domain::media::StreamKind::VideoOnly {
        args.extend(["-map".into(), "0:a:0".into()]);
    }
    args.extend([
        "-c".into(),
        "copy".into(),
        "-f".into(),
        "matroska".into(),
        "source.mkv".into(),
    ]);
    Ok(args)
}

struct SourceWorkspace {
    staging_dir: PathBuf,
    source_dir: PathBuf,
    armed: bool,
}

impl SourceWorkspace {
    fn create(job_id: &str, requested_staging: &Path) -> Result<Self, DownloadErrorCode> {
        let staging_dir = canonical_job_directory(job_id, requested_staging)?;
        let source_dir = staging_dir.join("source");
        if path_too_long(&source_dir) {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        match fs::symlink_metadata(&source_dir) {
            Ok(_) => return Err(DownloadErrorCode::OutputInvalid),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(DownloadErrorCode::OutputInvalid),
        }
        fs::create_dir(&source_dir).map_err(map_workspace_io)?;
        let workspace = Self {
            staging_dir,
            source_dir,
            armed: true,
        };
        workspace.revalidate()?;
        Ok(workspace)
    }

    fn revalidate(&self) -> Result<(), DownloadErrorCode> {
        let metadata =
            fs::symlink_metadata(&self.source_dir).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let canonical =
            fs::canonicalize(&self.source_dir).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if canonical != self.source_dir || canonical.parent() != Some(self.staging_dir.as_path()) {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), DownloadErrorCode> {
        self.revalidate()?;
        fs::remove_dir_all(&self.source_dir).map_err(map_workspace_io)?;
        match fs::symlink_metadata(&self.source_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.armed = false;
                Ok(())
            }
            _ => Err(DownloadErrorCode::OutputInvalid),
        }
    }
}

impl Drop for SourceWorkspace {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.cleanup();
        }
    }
}

fn canonical_job_directory(job_id: &str, requested: &Path) -> Result<PathBuf, DownloadErrorCode> {
    if !valid_opaque_id(job_id)
        || requested.file_name().and_then(|name| name.to_str()) != Some(job_id)
        || path_too_long(requested)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let metadata = fs::symlink_metadata(requested).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical = fs::canonicalize(requested).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if canonical.file_name().and_then(|name| name.to_str()) != Some(job_id)
        || path_too_long(&canonical)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(canonical)
}

pub(super) fn map_workspace_io(error: io::Error) -> DownloadErrorCode {
    if matches!(error.raw_os_error(), Some(28 | 112)) {
        DownloadErrorCode::DiskFull
    } else {
        DownloadErrorCode::OutputInvalid
    }
}

fn validate_request(request: &DownloadExecutionRequest) -> Result<(), DownloadErrorCode> {
    super::bilibili_download::validate_bilibili_transfer(request)?;
    match (request.source.platform(), &request.x_source) {
        (Platform::X, Some(source))
            if source.identity().post_id == request.source.media_id()
                && request.playlist_item.is_none()
                && request.instagram_selector.is_none()
                && request.plan.clip.is_none() =>
        {
            source.validate_request(
                &request.plan,
                request.video_source.as_ref(),
                request.audio_source.as_ref(),
            )?;
        }
        (Platform::X, _) | (_, Some(_)) => return Err(DownloadErrorCode::InvalidRequest),
        _ => {}
    }
    if !request.plan.compatibility_mode
        && (request.plan.audio_processing
            == Some(crate::domain::output_policy::StreamProcessing::Transcode)
            || request.plan.video_processing
                == Some(crate::domain::output_policy::StreamProcessing::Transcode)
            || request.plan.container_selection
                == crate::domain::output_policy::ContainerSelection::Compatibility)
    {
        return Err(DownloadErrorCode::InvalidRequest);
    }
    let source_selector_valid = match (
        request.source.platform(),
        request.source.instagram_kind(),
        request.playlist_item,
        request.instagram_selector.as_ref(),
    ) {
        (Platform::Youtube, None, None, None) => true,
        (Platform::X, None, None, None) => request.x_source.is_some(),
        (Platform::Bilibili, None, None, None) => request.bilibili_source.is_some(),
        (
            Platform::Instagram,
            Some(InstagramUrlKind::Reel),
            None,
            Some(crate::application::instagram_video::InstagramVideoSelector::Reel),
        ) => true,
        (
            Platform::Instagram,
            Some(InstagramUrlKind::Post),
            Some(playlist_item),
            Some(crate::application::instagram_video::InstagramVideoSelector::PostItem {
                ordinal,
                expected_media_id,
            }),
        ) => {
            playlist_item == *ordinal
                && crate::application::instagram_video::InstagramVideoSelector::post_item(
                    *ordinal,
                    expected_media_id,
                )
                .is_ok()
        }
        _ => false,
    };
    let instagram_plan_valid = request.source.platform() != Platform::Instagram
        || (request.plan.clip.is_none()
            && (request.plan.mode == SelectionMode::BestAvailable
                || (request.plan.mode == SelectionMode::VideoOnly
                    && request.plan.audio_format_id.is_none()
                    && request.audio_source.is_none()
                    && request.video_source.as_ref().is_some_and(|format| {
                        format.kind == crate::domain::media::StreamKind::VideoOnly
                    }))));
    if !valid_opaque_id(&request.job_id)
        || !source_selector_valid
        || !instagram_plan_valid
        || request.staging_dir.as_os_str().is_empty()
        || request.plan.source_format_ids.is_empty()
        || request
            .plan
            .source_format_ids
            .iter()
            .any(|id| !safe_token(id))
        || request
            .plan
            .output_container
            .as_deref()
            .is_some_and(|container| !safe_container(container))
    {
        return Err(DownloadErrorCode::InvalidRequest);
    }
    match request.plan.operation {
        SelectionOperation::Keep => {
            let [source] = request.plan.source_format_ids.as_slice() else {
                return Err(DownloadErrorCode::InvalidRequest);
            };
            let video_only = request.plan.mode == SelectionMode::VideoOnly;
            if (video_only
                && (request.plan.audio_format_id.is_some()
                    || request.plan.video_format_id.as_deref() != Some(source)))
                || (!video_only && request.plan.audio_format_id.as_deref() != Some(source))
                || request
                    .plan
                    .video_format_id
                    .as_deref()
                    .is_some_and(|video| video != source)
            {
                return Err(DownloadErrorCode::InvalidRequest);
            }
        }
        SelectionOperation::Merge => {
            let [video, audio] = request.plan.source_format_ids.as_slice() else {
                return Err(DownloadErrorCode::InvalidRequest);
            };
            if video == audio
                || request.plan.video_format_id.as_deref() != Some(video)
                || request.plan.audio_format_id.as_deref() != Some(audio)
            {
                return Err(DownloadErrorCode::InvalidRequest);
            }
        }
        SelectionOperation::ExtractAudio => {
            let [audio] = request.plan.source_format_ids.as_slice() else {
                return Err(DownloadErrorCode::InvalidRequest);
            };
            if request.plan.video_format_id.is_some()
                || request.plan.audio_format_id.as_deref() != Some(audio)
                || request.plan.clip.is_some()
                || request.plan.output_container.is_none()
            {
                return Err(DownloadErrorCode::InvalidRequest);
            }
        }
        SelectionOperation::ExtractVideo => {
            let [video] = request.plan.source_format_ids.as_slice() else {
                return Err(DownloadErrorCode::InvalidRequest);
            };
            if request.plan.video_format_id.as_deref() != Some(video)
                || request.plan.mode != SelectionMode::VideoOnly
                || request.plan.audio_format_id.is_some()
                || request.plan.output_container.is_none()
            {
                return Err(DownloadErrorCode::InvalidRequest);
            }
        }
    }
    Ok(())
}

fn build_args(
    request: &DownloadExecutionRequest,
    deno_path: &Path,
    ffmpeg_path: &Path,
    output_template: &str,
    selector: &str,
    x_info: Option<&Path>,
) -> Result<Vec<OsString>, DownloadErrorCode> {
    let mut deno_runtime = OsString::from("deno:");
    deno_runtime.push(deno_path);
    let progress_template = format!(
        "download:LMD_PROGRESS\tjob={}\tdownloaded=%(progress.downloaded_bytes)s\ttotal=%(progress.total_bytes)s\testimate=%(progress.total_bytes_estimate)s\tspeed=%(progress.speed)s\teta=%(progress.eta)s\tstatus=%(progress.status)s",
        request.job_id
    );
    let mut args = vec![
        "--ignore-config".into(),
        "--no-remote-components".into(),
        "--no-plugin-dirs".into(),
        "--js-runtimes".into(),
        deno_runtime,
        "--ffmpeg-location".into(),
        ffmpeg_path.as_os_str().to_owned(),
    ];
    if request.source.platform() == Platform::Instagram {
        args.extend([
            "--impersonate".into(),
            super::INSTAGRAM_BROWSER_PROFILE.into(),
        ]);
    }
    if let Some(ordinal) = request.playlist_item {
        args.extend([
            OsString::from("--yes-playlist"),
            OsString::from("--playlist-items"),
            OsString::from(ordinal.to_string()),
        ]);
    } else {
        args.push(OsString::from("--no-playlist"));
    }
    args.extend([
        "--no-warnings".into(),
        "--no-continue".into(),
        "--no-overwrites".into(),
        "--newline".into(),
        "--progress".into(),
        "--progress-delta".into(),
        "0.5".into(),
        "--progress-template".into(),
        progress_template.into(),
        "--output".into(),
        output_template.into(),
        "--format".into(),
        selector.into(),
    ]);
    if let (Some(info), Some(source)) = (x_info, &request.bilibili_source) {
        args.extend(super::bilibili_download::transfer_args(
            source, selector, info,
        )?);
    } else if let Some(info) = x_info {
        let network = request
            .x_source
            .as_ref()
            .ok_or(DownloadErrorCode::InvalidRequest)?
            .network();
        let proxy = network.https_proxy_for_host("video.twimg.com");
        if proxy.is_some_and(|url| url.scheme() != "http") {
            return Err(DownloadErrorCode::AnalysisUnavailable);
        }
        // The pinned FFmpeg downloader applies *_i arguments before every -i
        // and uses stream copy. Do not let yt-dlp's default HTTP downloader
        // follow an approved CDN URL to an unapproved redirect target.
        let input_guards = format!("ffmpeg_i:-max_redirects 0 -protocol_whitelist https,http,tcp,tls,httpproxy -format_whitelist mov,matroska,webm -rw_timeout 8000000 -http_proxy '{}'", proxy.map(|url|url.as_str()).unwrap_or(""));
        args.extend([
            "--downloader".into(),
            "ffmpeg".into(),
            "--downloader-args".into(),
            input_guards.into(),
            "--no-cookies".into(),
            "--no-cookies-from-browser".into(),
            "--no-update".into(),
            "--no-cache-dir".into(),
            "--no-check-formats".into(),
            "--referer".into(),
            "https://x.com/".into(),
            "--user-agent".into(),
            "Mozilla/5.0".into(),
            "--load-info-json".into(),
            info.as_os_str().to_owned(),
        ]);
    } else {
        args.push("--".into());
        args.push(request.source.url().into());
    }
    Ok(args)
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn safe_container(value: &str) -> bool {
    matches!(value, "mp4" | "webm" | "mkv" | "m4a" | "mp3" | "flac")
}

struct StructuredProgressSink<'a> {
    parser: Mutex<ProgressParser>,
    error: Mutex<Option<DownloadErrorCode>>,
    output: &'a dyn DownloadProgressSink,
    cancellation: CancellationToken,
    source_count: usize,
}

impl<'a> StructuredProgressSink<'a> {
    fn new(
        output: &'a dyn DownloadProgressSink,
        cancellation: CancellationToken,
        source_count: usize,
    ) -> Self {
        Self {
            parser: Mutex::new(ProgressParser::new()),
            error: Mutex::new(None),
            output,
            cancellation,
            source_count,
        }
    }

    fn error(&self) -> Option<DownloadErrorCode> {
        self.error.lock().ok().and_then(|error| *error)
    }

    fn record_error(&self, error: DownloadErrorCode) {
        if let Ok(mut stored) = self.error.lock() {
            stored.get_or_insert(error);
        }
    }
}

impl ProcessEventSink for StructuredProgressSink<'_> {
    fn on_line(&self, stream: ProcessStream, line: &[u8]) {
        if self.cancellation.is_canceled()
            || stream != ProcessStream::Stdout
            || self.error().is_some()
        {
            return;
        }
        let parsed = match self.parser.lock() {
            Ok(mut parser) => parser.parse(line),
            Err(_) => Err(DownloadErrorCode::OutputInvalid),
        };
        match parsed {
            Ok(Some(mut progress)) => {
                if self.source_count > 1 {
                    progress.total_bytes = None;
                    progress.percent = None;
                    progress.eta_seconds = None;
                }
                self.output.on_progress(progress);
            }
            Ok(None) => {}
            Err(error) => self.record_error(error),
        }
    }
}

fn map_tool_error(error: ManagedToolError) -> DownloadErrorCode {
    match error.code {
        ManagedToolErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        ManagedToolErrorCode::ToolIntegrityFailed
        | ManagedToolErrorCode::InvalidToolManifest
        | ManagedToolErrorCode::ToolVersionMismatch => DownloadErrorCode::ToolIntegrityFailed,
        ManagedToolErrorCode::UnsupportedTarget
        | ManagedToolErrorCode::ToolProbeFailed
        | ManagedToolErrorCode::ToolActivationFailed => DownloadErrorCode::ProcessFailed,
    }
}

fn map_process_error(error: ProcessFailureKind) -> DownloadErrorCode {
    match error {
        ProcessFailureKind::TimedOut | ProcessFailureKind::InactivityTimedOut => {
            DownloadErrorCode::DownloadTimeout
        }
        ProcessFailureKind::StdoutLimitExceeded
        | ProcessFailureKind::StderrLimitExceeded
        | ProcessFailureKind::LineLimitExceeded => DownloadErrorCode::OutputInvalid,
        ProcessFailureKind::LaunchFailed
        | ProcessFailureKind::JobControlFailed
        | ProcessFailureKind::IoFailed
        | ProcessFailureKind::Canceled => DownloadErrorCode::ProcessFailed,
    }
}

pub(super) fn map_nonzero_exit(stderr: &[u8]) -> DownloadErrorCode {
    map_analysis_error(super::errors::classify_nonzero_exit(stderr))
}
fn map_hls_error(
    error: crate::infrastructure::x::hls_local::HlsTransferError,
) -> DownloadErrorCode {
    if matches!(
        error,
        crate::infrastructure::x::hls_local::HlsTransferError::Io(io::ErrorKind::StorageFull)
    ) {
        DownloadErrorCode::DiskFull
    } else {
        map_analysis_error(error.analysis_code())
    }
}
fn map_analysis_error(error: AnalysisErrorCode) -> DownloadErrorCode {
    match error {
        AnalysisErrorCode::AuthenticationRequired => DownloadErrorCode::AuthenticationRequired,
        AnalysisErrorCode::AccessDenied => DownloadErrorCode::AccessDenied,
        AnalysisErrorCode::ContentUnavailable => DownloadErrorCode::ContentUnavailable,
        AnalysisErrorCode::ContentChanged => DownloadErrorCode::ContentChanged,
        AnalysisErrorCode::RateLimited => DownloadErrorCode::RateLimited,
        AnalysisErrorCode::ExtractorOutdated => DownloadErrorCode::ExtractorOutdated,
        AnalysisErrorCode::NetworkFailed => DownloadErrorCode::NetworkFailed,
        AnalysisErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        AnalysisErrorCode::ToolIntegrityFailed => DownloadErrorCode::ToolIntegrityFailed,
        AnalysisErrorCode::AnalysisTimeout => DownloadErrorCode::DownloadTimeout,
        AnalysisErrorCode::InvalidMetadata => DownloadErrorCode::OutputInvalid,
        AnalysisErrorCode::ProcessFailed => DownloadErrorCode::ProcessFailed,
        AnalysisErrorCode::InvalidUrl
        | AnalysisErrorCode::UnsupportedPlatform
        | AnalysisErrorCode::UnsupportedMedia
        | AnalysisErrorCode::FormatUnavailable
        | AnalysisErrorCode::AnalysisUnavailable
        | AnalysisErrorCode::AnalysisExpired => DownloadErrorCode::InvalidRequest,
        AnalysisErrorCode::Unknown => DownloadErrorCode::Unknown,
    }
}

fn is_changed_post_video(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    text.contains("there is no video") || text.contains("no video formats found")
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn path_too_long(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().count() > 32_767
}

#[cfg(not(windows))]
fn path_too_long(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len() > 4_096
}

#[cfg(test)]
mod hls_error_tests {
    use super::*;
    use crate::infrastructure::x::hls_local::HlsTransferError;
    #[test]
    fn local_hls_storage_full_preserves_folder_recovery() {
        assert_eq!(
            map_hls_error(HlsTransferError::Io(io::ErrorKind::StorageFull)),
            DownloadErrorCode::DiskFull
        );
        for code in [28, 112] {
            assert_eq!(
                map_hls_error(io::Error::from_raw_os_error(code).into()),
                DownloadErrorCode::DiskFull
            );
        }
        assert_eq!(
            map_hls_error(HlsTransferError::Io(io::ErrorKind::PermissionDenied)),
            DownloadErrorCode::ProcessFailed
        );
        assert_eq!(
            map_hls_error(AnalysisErrorCode::NetworkFailed.into()),
            DownloadErrorCode::NetworkFailed
        );
    }
}
