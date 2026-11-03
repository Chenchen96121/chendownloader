use super::url_policy::{validate_x_asset_url, XAssetKind};
use crate::{application::x_source::XRequestContext, domain::media::AnalysisErrorCode};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
pub trait HlsManifestTransport: Send + Sync {
    fn read(&self, url: &url::Url, context: &XRequestContext)
        -> Result<Vec<u8>, AnalysisErrorCode>;
}
const MAX_FILE: usize = 512 * 1024;
pub struct VerifiedHlsSnapshot {
    directory: Arc<tempfile::TempDir>,
    input: PathBuf,
    files: Vec<(PathBuf, [u8; 32])>,
    media_playlist: bool,
}
impl VerifiedHlsSnapshot {
    pub fn is_media_playlist(&self) -> bool {
        self.media_playlist
    }
    pub fn input(&self) -> &Path {
        &self.input
    }
    pub fn revalidate(&self) -> Result<(), AnalysisErrorCode> {
        let invalid = AnalysisErrorCode::InvalidMetadata;
        let root = self.directory.path().canonicalize().map_err(|_| invalid)?;
        for (path, digest) in &self.files {
            let meta = fs::symlink_metadata(path).map_err(|_| invalid)?;
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || reparse
                || meta.len() > MAX_FILE as u64
                || path.canonicalize().map_err(|_| invalid)?.parent() != Some(root.as_path())
            {
                return Err(invalid);
            }
            let bytes = fs::read(path).map_err(|_| invalid)?;
            let actual: [u8; 32] = Sha256::digest(&bytes).into();
            if &actual != digest {
                return Err(invalid);
            }
        }
        Ok(())
    }
}
pub fn build_hls_snapshot(
    url: &url::Url,
    transport: &dyn HlsManifestTransport,
    context: &XRequestContext,
) -> Result<VerifiedHlsSnapshot, AnalysisErrorCode> {
    validate_x_asset_url(url.as_str(), XAssetKind::Video)?;
    let directory = Arc::new(tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?);
    let mut builder = Builder {
        transport,
        context,
        directory: directory.clone(),
        cached: HashMap::new(),
        visiting: HashSet::new(),
        files: vec![],
        bytes: 0,
        uris: 0,
        root_is_media: false,
    };
    let input = builder.playlist(url, 0)?;
    let result = VerifiedHlsSnapshot {
        directory,
        input,
        files: builder.files,
        media_playlist: builder.root_is_media,
    };
    result.revalidate()?;
    Ok(result)
}
struct Builder<'a> {
    transport: &'a dyn HlsManifestTransport,
    context: &'a XRequestContext,
    directory: Arc<tempfile::TempDir>,
    cached: HashMap<String, PathBuf>,
    visiting: HashSet<String>,
    files: Vec<(PathBuf, [u8; 32])>,
    bytes: usize,
    uris: usize,
    root_is_media: bool,
}
impl Builder<'_> {
    fn check(&self) -> Result<(), AnalysisErrorCode> {
        if self.context.cancellation.is_canceled() {
            Err(AnalysisErrorCode::ProcessFailed)
        } else if Instant::now() >= self.context.deadline {
            Err(AnalysisErrorCode::AnalysisTimeout)
        } else {
            Ok(())
        }
    }
    fn playlist(&mut self, url: &url::Url, depth: usize) -> Result<PathBuf, AnalysisErrorCode> {
        self.check()?;
        let invalid = AnalysisErrorCode::InvalidMetadata;
        if depth > 4 || self.visiting.contains(url.as_str()) {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        if let Some(path) = self.cached.get(url.as_str()) {
            return Ok(path.clone());
        }
        if self.cached.len() >= 32 {
            return Err(invalid);
        }
        validate_x_asset_url(url.as_str(), XAssetKind::Video)?;
        let bytes = self.transport.read(url, self.context)?;
        self.bytes += bytes.len();
        if bytes.len() > MAX_FILE || self.bytes > 2 * 1024 * 1024 {
            return Err(invalid);
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| invalid)?
            .trim_start_matches('\u{feff}');
        if !text.starts_with("#EXTM3U") || text.contains("{$") || text.contains('\0') {
            return Err(invalid);
        }
        let path = self
            .directory
            .path()
            .join(format!("playlist-{}.m3u8", self.cached.len()));
        self.cached.insert(url.as_str().into(), path.clone());
        self.visiting.insert(url.as_str().into());
        let mut result = String::new();
        let mut next_playlist = false;
        let mut master = false;
        let mut media = false;
        let mut end = false;
        let mut playable = false;
        for line in text.lines() {
            self.check()?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.chars().any(char::is_control) {
                return Err(invalid);
            }
            if line.starts_with('#') {
                let tag = line.split(':').next().unwrap_or("");
                if tag == "#EXT-X-KEY" {
                    if line != "#EXT-X-KEY:METHOD=NONE" {
                        return Err(AnalysisErrorCode::UnsupportedMedia);
                    }
                } else if tag.starts_with("#EXT")
                    && !matches!(
                        tag,
                        "#EXTM3U"
                            | "#EXTINF"
                            | "#EXT-X-VERSION"
                            | "#EXT-X-TARGETDURATION"
                            | "#EXT-X-MEDIA-SEQUENCE"
                            | "#EXT-X-DISCONTINUITY-SEQUENCE"
                            | "#EXT-X-PLAYLIST-TYPE"
                            | "#EXT-X-INDEPENDENT-SEGMENTS"
                            | "#EXT-X-ENDLIST"
                            | "#EXT-X-BYTERANGE"
                            | "#EXT-X-MAP"
                            | "#EXT-X-DISCONTINUITY"
                            | "#EXT-X-PROGRAM-DATE-TIME"
                            | "#EXT-X-STREAM-INF"
                            | "#EXT-X-MEDIA"
                            | "#EXT-X-I-FRAME-STREAM-INF"
                            | "#EXT-X-ALLOW-CACHE"
                            | "#EXT-X-START"
                            | "#EXT-X-GAP"
                    )
                {
                    return Err(AnalysisErrorCode::UnsupportedMedia);
                }
                if tag == "#EXT-X-STREAM-INF" {
                    next_playlist = true;
                    master = true;
                }
                if tag == "#EXT-X-ENDLIST" {
                    end = true;
                }
                let playlist_attr = matches!(tag, "#EXT-X-MEDIA" | "#EXT-X-I-FRAME-STREAM-INF");
                if playlist_attr {
                    master = true;
                }
                let mut remaining = line;
                while let Some(index) = remaining.find("URI=") {
                    if !playlist_attr && tag != "#EXT-X-MAP" {
                        return Err(invalid);
                    }
                    result.push_str(&remaining[..index + 4]);
                    let value = &remaining[index + 4..];
                    let value = value.strip_prefix('"').ok_or(invalid)?;
                    let close = value.find('"').ok_or(invalid)?;
                    let reference = self.reference(url, &value[..close], playlist_attr, depth)?;
                    if playlist_attr {
                        playable = true;
                    }
                    result.push('"');
                    result.push_str(&reference);
                    result.push('"');
                    remaining = &value[close + 1..];
                }
                result.push_str(remaining);
                result.push('\n');
            } else {
                let reference = self.reference(url, line, next_playlist, depth)?;
                playable = true;
                if !next_playlist {
                    media = true;
                }
                next_playlist = false;
                result.push_str(&reference);
                result.push('\n');
            }
        }
        if next_playlist || !playable || (master && media) || (!master && !media) || (media && !end)
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        if result.len() > MAX_FILE {
            return Err(invalid);
        }
        if depth == 0 {
            self.root_is_media = !master && media && end;
        }
        fs::write(&path, result.as_bytes()).map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        self.files
            .push((path.clone(), Sha256::digest(result.as_bytes()).into()));
        self.visiting.remove(url.as_str());
        Ok(path)
    }
    fn reference(
        &mut self,
        base: &url::Url,
        raw: &str,
        playlist: bool,
        depth: usize,
    ) -> Result<String, AnalysisErrorCode> {
        self.uris += 1;
        if self.uris > 10_000 || raw.contains('\\') || raw.chars().any(char::is_control) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let url = base
            .join(raw)
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        validate_x_asset_url(url.as_str(), XAssetKind::Video)?;
        if !playlist && url.path().ends_with(".m3u8") {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        if playlist {
            let path = self.playlist(&url, depth + 1)?;
            // FFmpeg's file protocol does not URL-decode paths and Windows
            // does not open browser-style file:///C:/ URLs. This is a native
            // generated path, never a remote-supplied local file reference.
            let path = path.to_str().ok_or(AnalysisErrorCode::InvalidMetadata)?;
            if path.chars().any(char::is_control) || path.contains('"') {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            return Ok(format!("file:{}", path.replace('\\', "/")));
        }
        if !url
            .path()
            .rsplit_once('.')
            .is_some_and(|(_, ext)| matches!(ext, "ts" | "m4s" | "mp4" | "m4a" | "aac" | "vtt"))
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        Ok(url.to_string())
    }
}
pub struct HttpXManifestTransport;
impl HlsManifestTransport for HttpXManifestTransport {
    fn read(
        &self,
        url: &url::Url,
        context: &XRequestContext,
    ) -> Result<Vec<u8>, AnalysisErrorCode> {
        validate_x_asset_url(url.as_str(), XAssetKind::Video)?;
        if context.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let deadline = context
            .deadline
            .min(Instant::now() + Duration::from_secs(15));
        for attempt in 0..2 {
            if context.cancellation.is_canceled() {
                return Err(AnalysisErrorCode::ProcessFailed);
            }
            deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or(AnalysisErrorCode::AnalysisTimeout)?;
            let client = super::http_session::x_video_http_client(
                &context.network,
                super::http_session::XHttpPolicy::Manifest,
            )?;
            let result: Result<Vec<u8>, ManifestAttemptError> =
                tauri::async_runtime::block_on(async {
                    if context.cancellation.is_canceled() {
                        return Err(AnalysisErrorCode::ProcessFailed.into());
                    }
                    let timeout = deadline
                        .checked_duration_since(Instant::now())
                        .filter(|d| !d.is_zero())
                        .ok_or(AnalysisErrorCode::AnalysisTimeout)?;
                    let mut response = client
                        .get(url.clone())
                        .header("Referer", "https://x.com/")
                        .header("User-Agent", "Mozilla/5.0")
                        .timeout(timeout)
                        .send()
                        .await
                        .map_err(|error| manifest_remote_error(&error))?;
                    if !response.status().is_success() {
                        return Err(ManifestAttemptError::from(
                            match response.status().as_u16() {
                                401 => AnalysisErrorCode::AuthenticationRequired,
                                403 => AnalysisErrorCode::AccessDenied,
                                404 => AnalysisErrorCode::ContentUnavailable,
                                429 => AnalysisErrorCode::RateLimited,
                                _ => AnalysisErrorCode::NetworkFailed,
                            },
                        ));
                    }
                    if response
                        .content_length()
                        .is_some_and(|n| n > MAX_FILE as u64)
                    {
                        return Err(AnalysisErrorCode::InvalidMetadata.into());
                    }
                    // Each attempt owns its response buffer. A failed body is
                    // discarded before a retry can collect any replacement bytes.
                    let mut bytes = Vec::new();
                    loop {
                        if context.cancellation.is_canceled() {
                            return Err(AnalysisErrorCode::ProcessFailed.into());
                        }
                        if Instant::now() >= deadline {
                            return Err(AnalysisErrorCode::AnalysisTimeout.into());
                        }
                        let chunk = response
                            .chunk()
                            .await
                            .map_err(|error| manifest_remote_error(&error))?;
                        let Some(chunk) = chunk else {
                            break;
                        };
                        if bytes
                            .len()
                            .checked_add(chunk.len())
                            .is_none_or(|length| length > MAX_FILE)
                        {
                            return Err(AnalysisErrorCode::InvalidMetadata.into());
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    Ok(bytes)
                });
            if context.cancellation.is_canceled() {
                return Err(AnalysisErrorCode::ProcessFailed);
            }
            if Instant::now() >= deadline {
                return Err(AnalysisErrorCode::AnalysisTimeout);
            }
            match result {
                Ok(bytes) => return Ok(bytes),
                Err(failure) if attempt == 0 && failure.retryable => {
                    if context.network.x_video_http.ffmpeg.get().is_some() {
                        let directory =
                            tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
                        let output = directory.path().join("manifest.bin");
                        let mut fallback_context = context.clone();
                        fallback_context.deadline = deadline;
                        if let Some(result) = super::ffmpeg_http::fetch(
                            url,
                            &output,
                            MAX_FILE as u64,
                            false,
                            Duration::from_secs(15),
                            &fallback_context,
                        ) {
                            result.map_err(|error| error.analysis_code())?;
                            return fs::read(&output).map_err(|_| AnalysisErrorCode::ProcessFailed);
                        }
                    }
                }
                Err(failure) if attempt == 1 || !failure.retryable => return Err(failure.error),
                Err(_) => {}
            }
        }
        unreachable!("bounded manifest attempts always return")
    }
}

struct ManifestAttemptError {
    error: AnalysisErrorCode,
    retryable: bool,
}
impl From<AnalysisErrorCode> for ManifestAttemptError {
    fn from(error: AnalysisErrorCode) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
}

fn manifest_remote_error(error: &(dyn std::error::Error + 'static)) -> ManifestAttemptError {
    use std::{error::Error, io::ErrorKind};
    let mut source: Option<&(dyn Error + 'static)> = Some(error);
    let mut timeout = false;
    let mut transient = false;
    while let Some(cause) = source {
        if let Some(request) = cause.downcast_ref::<reqwest::Error>() {
            timeout |= request.is_timeout();
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            timeout |= io.kind() == ErrorKind::TimedOut;
            transient |= matches!(
                io.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
                    | ErrorKind::TimedOut
            );
            if let Some(inner) = io.get_ref() {
                source = Some(inner);
                continue;
            }
        }
        source = cause.source();
    }
    ManifestAttemptError {
        error: if timeout {
            AnalysisErrorCode::AnalysisTimeout
        } else {
            AnalysisErrorCode::NetworkFailed
        },
        retryable: timeout || transient,
    }
}

#[cfg(test)]
#[path = "manifest_http_tests.rs"]
mod http_tests;
