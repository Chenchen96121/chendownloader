//! Bilibili-only transfer authority; never accepts frontend source metadata.
use crate::{
    application::{
        bilibili_download::VerifiedBilibiliDownloadSource,
        download_executor::DownloadExecutionRequest,
        local_processing::{LocalProcessingRequest, ProcessingEvidence},
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{download::DownloadErrorCode, media::Platform},
    infrastructure::{
        ffmpeg::{canonical_job_directory, validate_sources, FfmpegMediaProcessor, SourceGuards},
        ffprobe::stream_properties::{
            frame_rate, rates_match, refine_video_properties, AudioStreamProperties,
            VideoStreamProperties,
        },
    },
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn validate_bilibili_transfer(
    request: &DownloadExecutionRequest,
) -> Result<(), DownloadErrorCode> {
    match (request.source.platform(), &request.bilibili_source) {
        (Platform::Bilibili, Some(source))
            if request.x_source.is_none()
                && request.playlist_item.is_none()
                && request.instagram_selector.is_none()
                && source.referer() == request.source.url() =>
        {
            source.validate_request(
                &request.plan,
                request.video_source.as_ref(),
                request.audio_source.as_ref(),
            )
        }
        (Platform::Bilibili, _) | (_, Some(_)) => Err(DownloadErrorCode::InvalidRequest),
        _ => Ok(()),
    }
}

pub(super) fn transfer_args(
    source: &VerifiedBilibiliDownloadSource,
    id: &str,
    info: &Path,
) -> Result<Vec<OsString>, DownloadErrorCode> {
    let url = source.format_url(id)?;
    let network = source.network();
    let guards = input_guards(
        &network,
        url.host_str().ok_or(DownloadErrorCode::InvalidRequest)?,
    )?;
    Ok(vec![
        "--downloader".into(),
        "ffmpeg".into(),
        "--downloader-args".into(),
        guards.into(),
        "--no-cookies".into(),
        "--no-cookies-from-browser".into(),
        "--no-update".into(),
        "--no-cache-dir".into(),
        "--no-check-formats".into(),
        "--referer".into(),
        source.referer().into(),
        "--user-agent".into(),
        "Mozilla/5.0".into(),
        "--load-info-json".into(),
        info.as_os_str().to_owned(),
    ])
}

fn input_guards(
    network: &crate::application::network_profile::NetworkProfile,
    host: &str,
) -> Result<String, DownloadErrorCode> {
    let proxy = network.https_proxy_for_host(host);
    if proxy.is_some_and(|p| p.scheme() != "http") {
        return Err(DownloadErrorCode::AnalysisUnavailable);
    }
    // *_i is applied by the pinned downloader before every actual input.
    Ok(format!("ffmpeg_i:-max_redirects 0 -protocol_whitelist https,http,tcp,tls,httpproxy -format_whitelist mov,matroska,webm,flac,aac,mp3,ogg -rw_timeout 8000000 -http_proxy '{}'",proxy.map(|p|p.as_str()).unwrap_or("")))
}

pub(super) fn classify_transfer_error(stderr: &[u8]) -> DownloadErrorCode {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if [
        "no space left on device",
        "not enough space on the disk",
        "disk full",
        "errno 28",
        "winerror 112",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        DownloadErrorCode::DiskFull
    } else {
        super::download::map_nonzero_exit(stderr)
    }
}

// Frame refinement is shared, but these Bili inputs are already local and must
// never cause another remote read (including a nested format/protocol).
struct LocalFrameProcess<'a, P>(&'a P);
impl<P: ProcessSupervisor> ProcessSupervisor for LocalFrameProcess<'_, P> {
    fn run(
        &self,
        mut request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let separator = request
            .args
            .iter()
            .position(|a| a == "--")
            .ok_or(ProcessFailureKind::IoFailed)?;
        request.args.splice(
            separator..separator,
            [
                "-protocol_whitelist",
                "file",
                "-format_whitelist",
                "mov,matroska,webm,flac,aac,mp3,ogg",
            ]
            .map(Into::into),
        );
        self.0.run(request, cancellation, sink)
    }
}

pub(super) struct SealedInfoFile {
    // Drop read guard before TempPath attempts to delete the file.
    _guard: File,
    path: tempfile::TempPath,
    parent: PathBuf,
    bytes: Vec<u8>,
    digest: [u8; 32],
}
impl SealedInfoFile {
    pub(super) fn create(parent: &Path, bytes: Vec<u8>) -> Result<Self, DownloadErrorCode> {
        let mut file =
            tempfile::NamedTempFile::new_in(parent).map_err(super::download::map_workspace_io)?;
        file.write_all(&bytes)
            .map_err(super::download::map_workspace_io)?;
        file.as_file()
            .sync_all()
            .map_err(super::download::map_workspace_io)?;
        let path = file.into_temp_path();
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options
                .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
                .custom_flags(
                    windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
                );
        }
        let guard = options
            .open(&path)
            .map_err(|_| DownloadErrorCode::OutputInvalid)?;
        let sealed = Self {
            _guard: guard,
            path,
            parent: parent.to_owned(),
            digest: Sha256::digest(&bytes).into(),
            bytes,
        };
        sealed.verify()?;
        Ok(sealed)
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn verify(&self) -> Result<(), DownloadErrorCode> {
        let invalid = DownloadErrorCode::OutputInvalid;
        let metadata = fs::symlink_metadata(&self.path).map_err(|_| invalid)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return Err(invalid);
            }
        }
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != self.bytes.len() as u64
            || self.path.canonicalize().map_err(|_| invalid)?.parent()
                != Some(self.parent.as_path())
        {
            return Err(invalid);
        }
        let actual = fs::read(&self.path).map_err(|_| invalid)?;
        if actual != self.bytes || <[u8; 32]>::from(Sha256::digest(&actual)) != self.digest {
            return Err(invalid);
        }
        Ok(())
    }
}

pub(super) struct BilibiliLocalEvidence {
    // The full source files stay pinned until processing and evidence binding finish.
    _guards: SourceGuards,
    video: Option<VideoStreamProperties>,
    audio: Option<AudioStreamProperties>,
}

// A missing summary field is not evidence of absence. It may only be filled by
// a bounded probe of the same pinned original file, never by processor claims.
fn known_video_matches(known: &VideoStreamProperties, actual: &VideoStreamProperties) -> bool {
    let optional = |old: &Option<String>, new: &Option<String>| {
        old.as_ref().is_none_or(|value| new.as_ref() == Some(value))
    };
    known.codec == actual.codec
        && known.width == actual.width
        && known.height == actual.height
        && frame_rate(&actual.frame_rate)
            .zip(frame_rate(&known.frame_rate))
            .is_some_and(|(new, old)| rates_match(new, old))
        && optional(&known.profile, &actual.profile)
        && optional(&known.pixel_format, &actual.pixel_format)
        && optional(&known.color_range, &actual.color_range)
        && optional(&known.color_space, &actual.color_space)
        && optional(&known.color_transfer, &actual.color_transfer)
        && optional(&known.color_primaries, &actual.color_primaries)
        && (known.hdr_side_data.is_empty() || known.hdr_side_data == actual.hdr_side_data)
}

pub(super) fn verify_downloaded_sources<
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
>(
    source: &VerifiedBilibiliDownloadSource,
    request: &LocalProcessingRequest,
    resolver: &R,
    process: &P,
    cancellation: &CancellationToken,
) -> Result<BilibiliLocalEvidence, DownloadErrorCode> {
    let paths = validate_sources(request)?;
    let staging = canonical_job_directory(request)?;
    let guards = SourceGuards::acquire(request, &staging, &paths)?;
    let probe = FfmpegMediaProcessor::new(resolver, process);
    let mut local = BilibiliLocalEvidence {
        _guards: guards,
        video: None,
        audio: None,
    };
    for (input, path) in request.sources.iter().zip(&paths) {
        let result = probe.probe_complete_source(path, &staging, cancellation, true)?;
        let duration = result
            .format
            .get("duration")
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        let full = source.duration_seconds();
        if (duration - full).abs() > 2.0_f64.max(full * 0.01) {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let (original_video, original_audio) = source.original_properties(&input.format_id)?;
        if result.streams.len()
            != usize::from(original_video.is_some()) + usize::from(original_audio.is_some())
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        if let Some(original) = original_video {
            let mut actual = result
                .streams
                .iter()
                .find(|s| s.codec_type == "video")
                .ok_or(DownloadErrorCode::OutputInvalid)?
                .video_properties()?;
            refine_video_properties(
                resolver,
                &LocalFrameProcess(process),
                path,
                &staging,
                cancellation,
                &mut actual,
                request.plan.compatibility_mode,
            )?;
            if !known_video_matches(original, &actual) {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            let declared = request
                .video_source
                .iter()
                .chain(request.audio_source.iter())
                .find(|format| format.id == input.format_id)
                .ok_or(DownloadErrorCode::OutputInvalid)?;
            let transfer = actual.color_transfer.as_deref();
            let hdr = matches!(transfer, Some("smpte2084" | "arib-std-b67"));
            if match declared.dynamic_range.as_deref() {
                Some("SDR") => hdr,
                Some("HDR") => !hdr,
                Some("HDR10") => transfer != Some("smpte2084"),
                Some("HLG") => transfer != Some("arib-std-b67"),
                Some("DV" | "HDR10+" | "HDR12") => true,
                _ => false,
            } {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            if request.plan.video_format_id.as_deref() == Some(input.format_id.as_str()) {
                local.video = Some(actual);
            }
        }
        if let Some(original) = original_audio {
            let actual = result
                .streams
                .iter()
                .find(|s| s.codec_type == "audio")
                .ok_or(DownloadErrorCode::OutputInvalid)?
                .audio_properties()?;
            if !actual.unchanged_from(original) {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            if request.plan.audio_format_id.as_deref() == Some(input.format_id.as_str()) {
                local.audio = Some(actual);
            }
        }
    }
    Ok(local)
}

pub(super) fn verify_processing_evidence(
    local: &BilibiliLocalEvidence,
    request: &DownloadExecutionRequest,
    evidence: &mut ProcessingEvidence,
) -> Result<(), DownloadErrorCode> {
    let invalid = DownloadErrorCode::OutputInvalid;
    if !match (local.video.as_ref(), evidence.source_video.as_ref()) {
        (Some(proven), Some(summary)) => known_video_matches(summary, proven),
        (None, None) => true,
        _ => false,
    } || !match (local.audio.as_ref(), evidence.source_audio.as_ref()) {
        (Some(old), Some(new)) => new.unchanged_from(old),
        (None, None) => true,
        _ => false,
    } {
        return Err(invalid);
    }
    let (video_processing, audio_processing) = if request.plan.compatibility_mode {
        let decision = crate::infrastructure::ffmpeg::compatibility::decide_compatibility(
            local.video.as_ref(),
            local.audio.as_ref(),
        )?;
        (decision.video_processing, decision.audio_processing)
    } else {
        (request.plan.video_processing, request.plan.audio_processing)
    };
    if evidence.video_processing != video_processing
        || evidence.audio_processing != audio_processing
    {
        return Err(invalid);
    }
    // Carry the independently proven original facts to final output verification.
    evidence.source_video = local.video.clone();
    evidence.source_audio = local.audio.clone();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::system_proxy::SystemProxies;

    #[test]
    fn actual_input_host_uses_frozen_proxy_and_bypass() {
        let mut proxies = SystemProxies::from_windows_manual_settings(true, "127.0.0.1:7897");
        proxies.bypass = Some("direct.bilivideo.com".into());
        let network = proxies.snapshot();
        assert!(input_guards(&network, "direct.bilivideo.com")
            .unwrap()
            .ends_with("-http_proxy ''"));
        assert!(input_guards(&network, "other.bilivideo.cn")
            .unwrap()
            .ends_with("-http_proxy 'http://127.0.0.1:7897/'"));
    }

    #[test]
    fn sealed_info_rechecks_length_contents_digest_and_private_parent() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().canonicalize().unwrap();
        let mut info = SealedInfoFile::create(&parent, b"safe fixture".to_vec()).unwrap();
        assert!(info.verify().is_ok());
        info.digest[0] ^= 1;
        assert_eq!(info.verify(), Err(DownloadErrorCode::OutputInvalid));
        info.digest[0] ^= 1;
        info.bytes[0] ^= 1;
        assert_eq!(info.verify(), Err(DownloadErrorCode::OutputInvalid));
        info.bytes.push(0);
        assert_eq!(info.verify(), Err(DownloadErrorCode::OutputInvalid));
        info.bytes = b"safe fixture".to_vec();
        info.parent = parent.join("not-the-parent");
        assert_eq!(info.verify(), Err(DownloadErrorCode::OutputInvalid));
        #[cfg(windows)]
        {
            assert!(fs::write(info.path(), b"replacement").is_err());
            assert!(fs::remove_file(info.path()).is_err());
        }
        let path = info.path().to_owned();
        drop(info);
        assert!(!path.exists());
    }
}
