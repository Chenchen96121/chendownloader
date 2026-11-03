//! Native-only, bounded local input for probing or copying a sealed X rendition.
use super::manifest::VerifiedHlsSnapshot;
use crate::{
    application::{
        network_profile::NetworkProfile, process::CancellationToken, x_source::XRequestContext,
    },
    domain::media::AnalysisErrorCode,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct HlsByteRange {
    pub offset: u64,
    pub length: u64,
}
#[derive(Clone, Copy, PartialEq)]
pub enum HlsReadMode {
    Probe,
    Complete,
}
#[derive(Debug)]
pub enum HlsTransferError {
    Media(AnalysisErrorCode),
    Io(std::io::ErrorKind),
}
impl HlsTransferError {
    pub fn analysis_code(&self) -> AnalysisErrorCode {
        match self {
            Self::Media(code) => *code,
            Self::Io(_) => AnalysisErrorCode::ProcessFailed,
        }
    }
}
impl From<AnalysisErrorCode> for HlsTransferError {
    fn from(code: AnalysisErrorCode) -> Self {
        Self::Media(code)
    }
}
impl From<std::io::Error> for HlsTransferError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(if matches!(error.raw_os_error(), Some(28 | 112)) {
            std::io::ErrorKind::StorageFull
        } else {
            error.kind()
        })
    }
}
pub trait HlsSegmentTransport: Send + Sync {
    fn fetch(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError>;
    fn fetch_probe(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError> {
        self.fetch(url, range, destination, maximum, context)
    }
}
pub struct HlsMaterializationRequest<'a> {
    pub snapshot: &'a VerifiedHlsSnapshot,
    pub parent: &'a Path,
    pub mode: HlsReadMode,
    pub network: Arc<NetworkProfile>,
    pub cancellation: CancellationToken,
    pub deadline: Option<Instant>,
}
pub struct LocalHlsInput {
    directory: tempfile::TempDir,
    input: PathBuf,
    complete: bool,
    sampled: bool,
    files: Vec<(PathBuf, u64, [u8; 32])>,
}
impl LocalHlsInput {
    pub fn input(&self) -> &Path {
        &self.input
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub(crate) fn has_probe_samples(&self) -> bool {
        self.sampled
    }
    pub fn revalidate(&self, cancel: &CancellationToken) -> Result<(), AnalysisErrorCode> {
        let root = self
            .directory
            .path()
            .canonicalize()
            .map_err(|_| invalid())?;
        for (path, length, expected) in &self.files {
            let meta = fs::symlink_metadata(path).map_err(|_| invalid())?;
            if !meta.is_file()
                || meta.len() != *length
                || meta.file_type().is_symlink()
                || reparse(&meta)
                || path.canonicalize().map_err(|_| invalid())?.parent() != Some(root.as_path())
                || digest(path, cancel)? != *expected
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
const PROBE_BYTES: u64 = 32 * 1024 * 1024;
const SEGMENT_BYTES: u64 = 512 * 1024 * 1024;
fn invalid() -> AnalysisErrorCode {
    AnalysisErrorCode::InvalidMetadata
}
fn check(cancel: &CancellationToken, deadline: Option<Instant>) -> Result<(), AnalysisErrorCode> {
    if cancel.is_canceled() {
        Err(AnalysisErrorCode::ProcessFailed)
    } else if deadline.is_some_and(|end| Instant::now() >= end) {
        Err(AnalysisErrorCode::AnalysisTimeout)
    } else {
        Ok(())
    }
}
fn reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}
fn digest(path: &Path, cancel: &CancellationToken) -> Result<[u8; 32], AnalysisErrorCode> {
    let mut file = File::open(path).map_err(|_| invalid())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        check(cancel, None)?;
        let n = file.read(&mut buffer).map_err(|_| invalid())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().into())
}
#[derive(Clone, Eq, PartialEq, Hash)]
struct Asset {
    url: url::Url,
    range: Option<HlsByteRange>,
}
enum Line {
    Text(String),
    Segment(usize),
    Map(usize),
}
fn byte_range(
    raw: &str,
    url: &url::Url,
    previous: Option<&(url::Url, u64)>,
) -> Result<HlsByteRange, AnalysisErrorCode> {
    let number = |raw: &str| -> Result<u64, AnalysisErrorCode> {
        if raw.is_empty() || raw.len() > 16 || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        raw.parse().map_err(|_| invalid())
    };
    let (length, offset) = if let Some((len, offset)) = raw.split_once('@') {
        (number(len)?, number(offset)?)
    } else {
        (
            number(raw)?,
            previous
                .filter(|(old, _)| old == url)
                .map(|(_, end)| *end)
                .ok_or(invalid())?,
        )
    };
    if length == 0
        || length > SEGMENT_BYTES
        || offset
            .checked_add(length)
            .is_none_or(|end| end > 9_007_199_254_740_991)
    {
        return Err(invalid());
    }
    Ok(HlsByteRange { offset, length })
}
fn map_attributes(raw: &str) -> Result<HashMap<&str, &str>, AnalysisErrorCode> {
    let mut values = HashMap::new();
    let mut rest = raw;
    while !rest.is_empty() {
        let (key, value) = rest.split_once('=').ok_or(invalid())?;
        if !matches!(key, "URI" | "BYTERANGE") {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        let value = value.strip_prefix('"').ok_or(invalid())?;
        let (value, tail) = value.split_once('"').ok_or(invalid())?;
        if value.is_empty() || values.insert(key, value).is_some() {
            return Err(invalid());
        }
        rest = if tail.is_empty() {
            ""
        } else {
            tail.strip_prefix(',')
                .filter(|tail| !tail.is_empty())
                .ok_or(invalid())?
        };
    }
    Ok(values)
}
fn plan(document: &str, mode: HlsReadMode) -> Result<(Vec<Asset>, Vec<Line>), AnalysisErrorCode> {
    let mut assets = vec![];
    let mut indices = HashMap::new();
    let mut lines = vec![];
    let mut previous = None;
    let mut range: Option<String> = None;
    let mut duration = 0.0;
    let mut pending_duration = None;
    let mut segments = 0;
    let mut add = |raw: &str, range: Option<HlsByteRange>| -> Result<usize, AnalysisErrorCode> {
        let url =
            super::url_policy::validate_x_asset_url(raw, super::url_policy::XAssetKind::Video)?;
        if !url
            .path()
            .rsplit_once('.')
            .is_some_and(|(_, ext)| matches!(ext, "ts" | "m4s" | "mp4" | "m4a" | "aac" | "vtt"))
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        let asset = Asset { url, range };
        if let Some(id) = indices.get(&asset) {
            return Ok(*id);
        }
        let id = assets.len();
        if id >= 10_000 {
            return Err(invalid());
        }
        indices.insert(asset.clone(), id);
        assets.push(asset);
        Ok(id)
    };
    for line in document.lines() {
        if mode == HlsReadMode::Probe && segments > 0 && (duration >= 3.0 || segments >= 3) {
            break;
        }
        if line == "#EXT-X-ENDLIST" {
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXT-X-MAP:") {
            let attrs = map_attributes(raw)?;
            let raw = attrs.get("URI").ok_or(invalid())?;
            let url =
                super::url_policy::validate_x_asset_url(raw, super::url_policy::XAssetKind::Video)?;
            let bytes = attrs
                .get("BYTERANGE")
                .map(|raw| byte_range(raw, &url, previous.as_ref()))
                .transpose()?;
            lines.push(Line::Map(add(raw, bytes)?));
        } else if let Some(raw) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            if range.replace(raw.to_owned()).is_some() {
                return Err(invalid());
            }
        } else if let Some(raw) = line.strip_prefix("#EXTINF:") {
            let value = raw
                .split(',')
                .next()
                .unwrap_or("")
                .parse::<f64>()
                .map_err(|_| invalid())?;
            if !value.is_finite()
                || value <= 0.0
                || value > 86_400.0
                || pending_duration.replace(value).is_some()
            {
                return Err(invalid());
            }
            lines.push(Line::Text(line.into()));
        } else if line == "#EXT-X-GAP" {
            return Err(AnalysisErrorCode::ContentUnavailable);
        } else if line.starts_with('#') || line.is_empty() {
            lines.push(Line::Text(line.into()));
        } else {
            let url = super::url_policy::validate_x_asset_url(
                line,
                super::url_policy::XAssetKind::Video,
            )?;
            let bytes = range
                .take()
                .map(|raw| byte_range(&raw, &url, previous.as_ref()))
                .transpose()?;
            previous = bytes.map(|range| (url, range.offset + range.length));
            lines.push(Line::Segment(add(line, bytes)?));
            duration += pending_duration.take().ok_or(invalid())?;
            segments += 1;
        }
    }
    if segments == 0 || range.is_some() || pending_duration.is_some() {
        return Err(invalid());
    }
    lines.push(Line::Text("#EXT-X-ENDLIST".into()));
    Ok((assets, lines))
}
pub fn materialize_hls(
    request: HlsMaterializationRequest<'_>,
    transport: &dyn HlsSegmentTransport,
) -> Result<LocalHlsInput, HlsTransferError> {
    check(&request.cancellation, request.deadline)?;
    request.snapshot.revalidate()?;
    if !request.snapshot.is_media_playlist() {
        return Err(AnalysisErrorCode::FormatUnavailable.into());
    }
    let document = fs::read_to_string(request.snapshot.input()).map_err(|_| invalid())?;
    let (assets, lines) = plan(&document, request.mode)?;
    // Maps are always complete, even if an asset also appears as media. Only
    // fMP4 segments with an active initialization map can use probe sampling.
    let mut probe_assets = vec![true; assets.len()];
    let mut map_assets = vec![false; assets.len()];
    let mut mapped = false;
    for line in &lines {
        match line {
            Line::Map(id) => {
                map_assets[*id] = true;
                mapped = true;
            }
            Line::Segment(id) => {
                probe_assets[*id] &= mapped
                    && assets[*id]
                        .url
                        .path()
                        .rsplit_once('.')
                        .is_some_and(|(_, ext)| matches!(ext, "m4s" | "mp4" | "m4a"));
            }
            _ => {}
        }
    }
    let meta =
        fs::symlink_metadata(request.parent).map_err(|_| AnalysisErrorCode::ProcessFailed)?;
    if !meta.is_dir() || meta.file_type().is_symlink() || reparse(&meta) {
        return Err(invalid().into());
    }
    let directory = tempfile::Builder::new()
        .prefix("x-hls-")
        .tempdir_in(request.parent)
        .map_err(HlsTransferError::from)?;
    let mut files = vec![];
    let mut names = vec![];
    let mut total = 0_u64;
    let mut sampled = false;
    for (id, asset) in assets.iter().enumerate() {
        check(&request.cancellation, request.deadline)?;
        let ext = asset.url.path().rsplit_once('.').ok_or(invalid())?.1;
        let path = directory.path().join(format!("asset-{id:05}.{ext}"));
        let maximum = if request.mode == HlsReadMode::Probe {
            PROBE_BYTES
                .checked_sub(total)
                .filter(|n| *n > 0)
                .ok_or(invalid())?
        } else {
            SEGMENT_BYTES
        };
        let context = XRequestContext::new(
            request.cancellation.clone(),
            request
                .deadline
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(120)),
            request.network.clone(),
        );
        let sample = request.mode == HlsReadMode::Probe && probe_assets[id] && !map_assets[id];
        sampled |= sample;
        let count = if sample {
            transport.fetch_probe(&asset.url, asset.range, &path, maximum, &context)?
        } else {
            transport.fetch(&asset.url, asset.range, &path, maximum, &context)?
        };
        let meta = fs::symlink_metadata(&path).map_err(|_| invalid())?;
        if count == 0
            || count > maximum
            || !meta.is_file()
            || meta.len() != count
            || meta.file_type().is_symlink()
            || reparse(&meta)
            || asset.range.is_some_and(|range| {
                if sample {
                    count > range.length
                } else {
                    range.length != count
                }
            })
        {
            return Err(invalid().into());
        }
        total = total
            .checked_add(count)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or(invalid())?;
        let path_text = path.to_str().ok_or(invalid())?;
        // Canonical Windows paths have a verbatim prefix which is not a valid
        // FFmpeg file-protocol URI. Keep the absolute drive/UNC path itself.
        let path_text = path_text.strip_prefix("\\\\?\\").unwrap_or(path_text);
        let path_text = path_text
            .strip_prefix("UNC\\")
            .map(|p| format!("//{p}"))
            .unwrap_or_else(|| path_text.to_owned());
        let uri = format!("file:{}", path_text.replace('\\', "/"));
        if uri.contains('"') || uri.chars().any(char::is_control) {
            return Err(invalid().into());
        }
        names.push(uri);
        let hash = digest(&path, &request.cancellation)?;
        files.push((path, count, hash));
    }
    let mut local = String::new();
    for line in lines {
        match line {
            Line::Text(text) => local.push_str(&text),
            Line::Segment(id) => local.push_str(&names[id]),
            Line::Map(id) => local.push_str(&format!("#EXT-X-MAP:URI=\"{}\"", names[id])),
        }
        local.push('\n');
    }
    let input = directory.path().join("input.m3u8");
    fs::write(&input, local.as_bytes()).map_err(HlsTransferError::from)?;
    files.push((
        input.clone(),
        local.len() as u64,
        Sha256::digest(local.as_bytes()).into(),
    ));
    let result = LocalHlsInput {
        directory,
        input,
        complete: request.mode == HlsReadMode::Complete,
        sampled,
        files,
    };
    check(&request.cancellation, request.deadline)?;
    result.revalidate(&request.cancellation)?;
    Ok(result)
}

pub struct HttpHlsSegmentTransport;
impl HlsSegmentTransport for HttpHlsSegmentTransport {
    fn fetch(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError> {
        self.transfer(url, range, destination, maximum, context, false)
    }
    fn fetch_probe(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError> {
        self.transfer(url, range, destination, maximum, context, true)
    }
}
impl HttpHlsSegmentTransport {
    fn transfer(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
        probe: bool,
    ) -> Result<u64, HlsTransferError> {
        super::url_policy::validate_x_asset_url(
            url.as_str(),
            super::url_policy::XAssetKind::Video,
        )?;
        check(&context.cancellation, Some(context.deadline))?;
        if maximum == 0
            || maximum > SEGMENT_BYTES
            || range.is_some_and(|range| {
                range.length == 0
                    || range.length > if probe { SEGMENT_BYTES } else { maximum }
                    || range.offset.checked_add(range.length).is_none()
            })
        {
            return Err(invalid().into());
        }
        let requested_range = if probe {
            Some(super::hls_probe::request_range(range, maximum)?)
        } else {
            range
        };
        retry_segment_with_fallback(
            destination,
            context,
            || {
                let maximum = ffmpeg_fallback_maximum(range, maximum, probe)?;
                super::ffmpeg_http::fetch(
                    url,
                    destination,
                    maximum,
                    probe,
                    Duration::from_secs(8),
                    context,
                )
            },
            |created| {
                let client = super::http_session::x_video_http_client(
                    &context.network,
                    super::http_session::XHttpPolicy::Segment,
                )?;
                tauri::async_runtime::block_on(async {
                    check(&context.cancellation, Some(context.deadline))?;
                    let remaining = context
                        .deadline
                        .checked_duration_since(Instant::now())
                        .filter(|duration| !duration.is_zero())
                        .ok_or(AnalysisErrorCode::AnalysisTimeout)?;
                    let mut request = client
                        .get(url.clone())
                        .header("Referer", "https://x.com/")
                        .header("User-Agent", "Mozilla/5.0")
                        .timeout(remaining);
                    if let Some(range) = requested_range {
                        let end = range
                            .offset
                            .checked_add(range.length)
                            .and_then(|n| n.checked_sub(1))
                            .ok_or(invalid())?;
                        request =
                            request.header("Range", format!("bytes={}-{}", range.offset, end));
                    }
                    let mut response = request.send().await.map_err(http_error)?;
                    if !response.status().is_success() {
                        return Err(SegmentAttemptError::from(HlsTransferError::from(
                            match response.status().as_u16() {
                                401 => AnalysisErrorCode::AuthenticationRequired,
                                403 => AnalysisErrorCode::AccessDenied,
                                404 => AnalysisErrorCode::ContentUnavailable,
                                429 => AnalysisErrorCode::RateLimited,
                                _ => AnalysisErrorCode::NetworkFailed,
                            },
                        )));
                    }
                    let length = response.content_length();
                    let (read_maximum, expected) = if probe {
                        super::hls_probe::read_limit(
                            range,
                            maximum,
                            response.status().as_u16(),
                            response
                                .headers()
                                .get("Content-Range")
                                .and_then(|v| v.to_str().ok()),
                            length,
                        )?
                    } else {
                        if let Some(range) = range {
                            let expected = format!(
                                "bytes {}-{}/",
                                range.offset,
                                range.offset + range.length - 1
                            );
                            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                                || !response
                                    .headers()
                                    .get("Content-Range")
                                    .and_then(|v| v.to_str().ok())
                                    .is_some_and(|v| v.starts_with(&expected))
                            {
                                return Err(invalid().into());
                            }
                        } else if response.status() != reqwest::StatusCode::OK {
                            return Err(invalid().into());
                        }
                        if length.is_some_and(|n| {
                            n == 0 || n > maximum || range.is_some_and(|range| range.length != n)
                        }) {
                            return Err(invalid().into());
                        }
                        (maximum, range.map(|r| r.length).or(length))
                    };
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(destination)
                        .map_err(HlsTransferError::from)?;
                    *created = true;
                    let mut total = 0_u64;
                    while let Some(chunk) = response.chunk().await.map_err(http_error)? {
                        check(&context.cancellation, Some(context.deadline))?;
                        total = total
                            .checked_add(chunk.len() as u64)
                            .filter(|n| *n <= read_maximum)
                            .ok_or(invalid())?;
                        file.write_all(&chunk).map_err(HlsTransferError::from)?;
                    }
                    check(&context.cancellation, Some(context.deadline))?;
                    if total == 0 || expected.is_some_and(|n| n != total) {
                        return Err(invalid().into());
                    }
                    file.sync_all().map_err(HlsTransferError::from)?;
                    Ok(total)
                })
            },
        )
    }
}
struct SegmentAttemptError {
    error: HlsTransferError,
    retryable: bool,
}
impl From<HlsTransferError> for SegmentAttemptError {
    fn from(error: HlsTransferError) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
}
impl From<AnalysisErrorCode> for SegmentAttemptError {
    fn from(error: AnalysisErrorCode) -> Self {
        HlsTransferError::from(error).into()
    }
}

// A second GET may recover a broken connection, but must not extend the probe
// deadline or turn invalid content/authentication/storage failures into retries.
#[cfg(test)]
fn retry_segment(
    destination: &Path,
    context: &XRequestContext,
    attempt: impl FnMut(&mut bool) -> Result<u64, SegmentAttemptError>,
) -> Result<u64, HlsTransferError> {
    retry_segment_with_fallback(destination, context, || None, attempt)
}

fn ffmpeg_fallback_maximum(range: Option<HlsByteRange>, maximum: u64, probe: bool) -> Option<u64> {
    // An explicit BYTERANGE needs Content-Range proof. Raw FFmpeg output cannot
    // supply that proof, including ranges whose offset happens to be zero.
    if range.is_some() {
        return None;
    }
    if probe {
        Some(super::hls_probe::request_range(None, maximum).ok()?.length)
    } else {
        Some(maximum)
    }
}

fn retry_segment_with_fallback(
    destination: &Path,
    context: &XRequestContext,
    mut fallback: impl FnMut() -> Option<Result<u64, HlsTransferError>>,
    mut attempt: impl FnMut(&mut bool) -> Result<u64, SegmentAttemptError>,
) -> Result<u64, HlsTransferError> {
    for index in 0..2 {
        check(&context.cancellation, Some(context.deadline))?;
        let mut created = false;
        match attempt(&mut created) {
            Ok(count) => return Ok(count),
            Err(failure) => {
                // The attempt's file handle has closed. Never delete a path
                // whose create_new did not succeed (it belongs to someone else).
                if created {
                    fs::remove_file(destination).map_err(HlsTransferError::from)?;
                }
                if index == 1 || !failure.retryable {
                    return Err(failure.error);
                }
                check(&context.cancellation, Some(context.deadline))?;
                if let Some(result) = fallback() {
                    return result;
                }
            }
        }
    }
    unreachable!("bounded segment attempts always return")
}
fn http_error(error: reqwest::Error) -> SegmentAttemptError {
    use std::{error::Error, io::ErrorKind};
    let timeout = error.is_timeout();
    let mut source: Option<&(dyn Error + 'static)> = Some(&error);
    let mut transient = timeout;
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            transient |= matches!(
                io.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
                    | ErrorKind::TimedOut
            );
        }
        source = cause.source();
    }
    SegmentAttemptError {
        error: HlsTransferError::from(if timeout {
            AnalysisErrorCode::AnalysisTimeout
        } else {
            AnalysisErrorCode::NetworkFailed
        }),
        retryable: transient,
    }
}
#[cfg(test)]
#[path = "hls_http_tests.rs"]
mod http_tests;

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use std::cell::Cell;

    fn context() -> XRequestContext {
        XRequestContext::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(3),
            Arc::new(crate::infrastructure::system_proxy::SystemProxies::default().snapshot()),
        )
    }

    #[test]
    fn first_disconnect_cleans_owned_partial_before_fallback_and_skips_second_native_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("segment");
        let calls = Cell::new(0);
        let result = retry_segment_with_fallback(
            &path,
            &context(),
            || {
                assert!(!path.exists());
                fs::write(&path, b"complete").unwrap();
                Some(Ok(8))
            },
            |created| {
                calls.set(calls.get() + 1);
                fs::write(&path, b"partial").unwrap();
                *created = true;
                Err(SegmentAttemptError {
                    error: AnalysisErrorCode::NetworkFailed.into(),
                    retryable: true,
                })
            },
        );
        assert_eq!(result.unwrap(), 8);
        assert_eq!(calls.get(), 1);
        assert_eq!(fs::read(path).unwrap(), b"complete");
    }

    #[test]
    fn native_success_and_permanent_failures_never_call_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("segment");
        assert_eq!(
            retry_segment_with_fallback(
                &path,
                &context(),
                || panic!("native success must win"),
                |_| Ok(17)
            )
            .unwrap(),
            17
        );
        for code in [
            AnalysisErrorCode::AuthenticationRequired,
            AnalysisErrorCode::AccessDenied,
            AnalysisErrorCode::ContentUnavailable,
            AnalysisErrorCode::RateLimited,
            AnalysisErrorCode::InvalidMetadata,
            AnalysisErrorCode::NetworkFailed,
        ] {
            let result = retry_segment_with_fallback(
                &path,
                &context(),
                || panic!("permanent failure must win"),
                |_| Err(code.into()),
            );
            assert_eq!(result.unwrap_err().analysis_code(), code);
        }
    }

    #[test]
    fn fallback_keeps_explicit_ranges_native_and_caps_only_original_zero_offset_probe_prefix() {
        for offset in [0, 99] {
            for probe in [false, true] {
                assert_eq!(
                    ffmpeg_fallback_maximum(Some(HlsByteRange { offset, length: 10 }), 100, probe),
                    None
                );
            }
        }
        assert_eq!(
            ffmpeg_fallback_maximum(None, 32 * 1024 * 1024, true),
            Some(1024 * 1024)
        );
        assert_eq!(ffmpeg_fallback_maximum(None, 100, true), Some(100));
        assert_eq!(ffmpeg_fallback_maximum(None, 100, false), Some(100));
    }

    #[test]
    fn cancellation_after_native_failure_prevents_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("segment");
        let context = context();
        let result = retry_segment_with_fallback(
            &path,
            &context,
            || panic!("canceled"),
            |_| {
                context.cancellation.cancel();
                Err(SegmentAttemptError {
                    error: AnalysisErrorCode::NetworkFailed.into(),
                    retryable: true,
                })
            },
        );
        assert_eq!(
            result.unwrap_err().analysis_code(),
            AnalysisErrorCode::ProcessFailed
        );
    }
}
