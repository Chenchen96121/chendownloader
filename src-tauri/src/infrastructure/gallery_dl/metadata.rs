use std::{
    ffi::OsString,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::Deserialize;
use url::{Host, Url};

use crate::{
    application::{
        media_url::ValidatedMediaUrl,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest,
            ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        instagram::{
            InstagramItemAvailability, InstagramMediaItem, InstagramMediaKind, InstagramUrlKind,
        },
        managed_tools::ToolName,
        media::{AnalysisErrorCode, FileSize, Platform},
    },
};

use super::{errors, GalleryDlInstagramAdapter};

const MAX_JSONL_BYTES: usize = 2_097_152;
const MAX_STDERR_BYTES: usize = 262_144;
const MAX_LINE_BYTES: usize = 262_144;
const MAX_ITEMS: usize = 32;
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
static NEXT_ITEM_ID: AtomicU64 = AtomicU64::new(1);

/// Native-only inventory. It deliberately has no `Serialize` or `Debug` implementation.
pub struct InstagramInventory {
    shortcode: String,
    title: String,
    creator: Option<String>,
    items: Vec<ResolvedInstagramItem>,
}

impl InstagramInventory {
    pub fn shortcode(&self) -> &str {
        &self.shortcode
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn creator(&self) -> Option<&str> {
        self.creator.as_deref()
    }
    pub fn items(&self) -> &[ResolvedInstagramItem] {
        &self.items
    }

    pub(crate) fn mark_web_assets(&mut self) {
        for item in &mut self.items {
            item.prefer_direct_asset = true;
        }
    }
}

/// A resolved item keeps volatile URLs inside Rust and cannot be serialized.
#[derive(Clone)]
pub struct ResolvedInstagramItem {
    pub(super) public: InstagramMediaItem,
    pub(super) media_id: String,
    pub(super) display_url: Url,
    pub(super) video_url: Option<Url>,
    pub(super) source_page_url: String,
    pub(super) prefer_direct_asset: bool,
}

impl ResolvedInstagramItem {
    pub fn public_item(&self) -> &InstagramMediaItem {
        &self.public
    }
    pub fn has_video_source(&self) -> bool {
        self.video_url.is_some()
    }

    pub(crate) fn media_id(&self) -> &str {
        &self.media_id
    }

    pub(crate) const fn prefers_direct_asset(&self) -> bool {
        self.prefer_direct_asset
    }

    pub(crate) fn display_url(&self) -> &Url {
        &self.display_url
    }
}

#[derive(Deserialize)]
struct RawItem {
    category: String,
    post_shortcode: String,
    post_url: String,
    #[serde(rename = "type")]
    item_type: String,
    description: Option<String>,
    username: Option<String>,
    num: u16,
    media_id: String,
    display_url: String,
    video_url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    extension: String,
    filesize: Option<u64>,
}

pub fn normalize_instagram_inventory(
    expected_shortcode: &str,
    jsonl: &[u8],
) -> Result<InstagramInventory, AnalysisErrorCode> {
    if !token(expected_shortcode, 64) || jsonl.is_empty() || jsonl.len() > MAX_JSONL_BYTES {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let text = std::str::from_utf8(jsonl).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let mut resolved = Vec::new();
    let mut shared_title: Option<String> = None;
    let mut shared_creator: Option<Option<String>> = None;

    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line.len() > MAX_LINE_BYTES || index >= MAX_ITEMS {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let raw: RawItem =
            serde_json::from_str(line).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if raw.post_shortcode != expected_shortcode {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        let expected_ordinal =
            u16::try_from(index + 1).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if raw.category != "instagram"
            || raw.item_type != "post"
            || raw.num != expected_ordinal
            || !token(&raw.media_id, 128)
            || !valid_post_url(&raw.post_url, expected_shortcode)
            || !dimensions(raw.width, raw.height)
            || raw
                .filesize
                .is_some_and(|size| size == 0 || size > 9_007_199_254_740_991)
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }

        let title = normalize_text(raw.description.as_deref(), 512)
            .unwrap_or_else(|| format!("Instagram {expected_shortcode}"));
        let creator = normalize_text(raw.username.as_deref(), 256);
        if shared_title.as_ref().is_some_and(|value| value != &title)
            || shared_creator
                .as_ref()
                .is_some_and(|value| value != &creator)
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        shared_title.get_or_insert_with(|| title.clone());
        shared_creator.get_or_insert_with(|| creator.clone());

        let display_url = private_asset_url(&raw.display_url)?;
        let video_url = raw
            .video_url
            .as_deref()
            .map(private_asset_url)
            .transpose()?;
        let extension = normalize_extension(&raw.extension)?;
        let kind = if video_url.is_some() {
            InstagramMediaKind::Video
        } else {
            if matches!(extension.as_str(), "mp4" | "webm" | "mkv" | "mov") {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            InstagramMediaKind::Image
        };
        let preview_available = private_path_extension(&display_url)
            .is_some_and(|ext| matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp"))
            && (video_url.is_some() || raw.filesize.is_none_or(|size| size <= MAX_PREVIEW_BYTES));
        let public = InstagramMediaItem {
            item_id: next_item_id()?,
            ordinal: raw.num,
            kind,
            width: raw.width,
            height: raw.height,
            container: extension,
            file_size: raw.filesize.map(|bytes| FileSize {
                bytes,
                is_approximate: false,
            }),
            fps: None,
            video_bitrate_kbps: None,
            audio_bitrate_kbps: None,
            video_codec: None,
            audio_codec: None,
            dynamic_range: None,
            preview_available,
            availability: InstagramItemAvailability::Ready,
            unavailable_reason: None,
        };
        public.validate()?;
        resolved.push(ResolvedInstagramItem {
            public,
            media_id: raw.media_id,
            display_url,
            video_url,
            source_page_url: format!("https://www.instagram.com/p/{expected_shortcode}/"),
            prefer_direct_asset: false,
        });
    }
    if resolved.is_empty() {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(InstagramInventory {
        shortcode: expected_shortcode.to_owned(),
        title: shared_title.ok_or(AnalysisErrorCode::InvalidMetadata)?,
        creator: shared_creator.flatten(),
        items: resolved,
    })
}

pub(super) fn extract_post<R, P>(
    adapter: &GalleryDlInstagramAdapter<R, P>,
    source: &ValidatedMediaUrl,
    cancellation: &CancellationToken,
) -> Result<InstagramInventory, AnalysisErrorCode>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    if source.platform() != Platform::Instagram
        || source.instagram_kind() != Some(InstagramUrlKind::Post)
    {
        return Err(AnalysisErrorCode::UnsupportedMedia);
    }
    if cancellation.is_canceled() {
        return Err(AnalysisErrorCode::ProcessFailed);
    }
    let gallery_dl = adapter
        .resolver
        .resolve_verified(ToolName::GalleryDl)
        .map_err(errors::analysis_tool)?;
    let working = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
    let args = [
        "--ignore-config",
        "--no-skip",
        "--no-download",
        "--quiet",
        "--no-input",
        "--postprocessor",
        "metadata",
        "--postprocessor-option",
        "mode=jsonl",
        "--postprocessor-option",
        "event=prepare",
        "--postprocessor-option",
        "filename=-",
        "--",
        source.url(),
    ]
    .map(OsString::from)
    .into();
    let exit = adapter
        .process
        .run(
            ProcessRequest {
                stdin: None,
                program: ProcessProgram::from_resolved_tool(gallery_dl),
                args,
                working_dir: working.path().to_owned(),
                timeout: Some(ANALYSIS_TIMEOUT),
                inactivity_timeout: None,
                output_mode: OutputMode::Capture {
                    stdout_limit: MAX_JSONL_BYTES,
                    stderr_limit: MAX_STDERR_BYTES,
                },
            },
            cancellation.clone(),
            &NoopSink,
        )
        .map_err(errors::analysis_process)?;
    if cancellation.is_canceled() {
        return Err(AnalysisErrorCode::ProcessFailed);
    }
    if exit.code != 0 {
        return Err(errors::classify_analysis(&exit.stderr));
    }
    normalize_instagram_inventory(source.media_id(), &exit.stdout)
}

fn valid_post_url(value: &str, shortcode: &str) -> bool {
    value == format!("https://www.instagram.com/p/{shortcode}/")
}

pub(super) fn verify_acquired_item_identity(
    expected: &ResolvedInstagramItem,
    jsonl: &[u8],
) -> Result<(), AnalysisErrorCode> {
    if jsonl.is_empty() || jsonl.len() > MAX_LINE_BYTES {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let text = std::str::from_utf8(jsonl).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let mut lines = text.lines();
    let line = lines.next().ok_or(AnalysisErrorCode::InvalidMetadata)?;
    if line.is_empty() || lines.next().is_some() {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let raw: RawItem =
        serde_json::from_str(line).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    if raw.category != "instagram"
        || raw.item_type != "post"
        || !token(&raw.post_shortcode, 64)
        || !token(&raw.media_id, 128)
        || !valid_post_url(&raw.post_url, &raw.post_shortcode)
        || !dimensions(raw.width, raw.height)
        || raw
            .filesize
            .is_some_and(|size| size == 0 || size > 9_007_199_254_740_991)
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let _display_url = private_asset_url(&raw.display_url)?;
    let video_url = raw
        .video_url
        .as_deref()
        .map(private_asset_url)
        .transpose()?;
    let extension = normalize_extension(&raw.extension)?;
    let kind = if video_url.is_some() {
        InstagramMediaKind::Video
    } else {
        if matches!(extension.as_str(), "mp4" | "webm" | "mkv" | "mov") {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        InstagramMediaKind::Image
    };
    let expected_page = format!("https://www.instagram.com/p/{}/", raw.post_shortcode);
    if raw.post_url != expected.source_page_url
        || expected_page != expected.source_page_url
        || raw.num != expected.public.ordinal
        || raw.media_id != expected.media_id
        || kind != expected.public.kind
    {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    Ok(())
}

fn private_asset_url(value: &str) -> Result<Url, AnalysisErrorCode> {
    let parsed = Url::parse(value).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let host = match parsed.host() {
        Some(Host::Domain(value)) => value,
        _ => return Err(AnalysisErrorCode::InvalidMetadata),
    };
    let approved_host = host == "instagram.com"
        || host.ends_with(".instagram.com")
        || host == "cdninstagram.com"
        || host.ends_with(".cdninstagram.com")
        || host == "fbcdn.net"
        || host.ends_with(".fbcdn.net");
    if parsed.scheme() != "https"
        || !approved_host
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.fragment().is_some()
        || parsed.path().is_empty()
        || parsed.path() == "/"
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(parsed)
}

fn private_path_extension(url: &Url) -> Option<String> {
    url.path_segments()?
        .next_back()?
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| ext.len() <= 8)
}

fn normalize_extension(value: &str) -> Result<String, AnalysisErrorCode> {
    let value = value.to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 8
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(if value == "jpeg" {
        "jpg".to_owned()
    } else {
        value
    })
}

fn normalize_text(value: Option<&str>, max: usize) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_owned())
    }
}

fn dimensions(width: Option<u32>, height: Option<u32>) -> bool {
    matches!((width, height), (None, None))
        || matches!((width, height), (Some(w), Some(h)) if (1..=16_384).contains(&w) && (1..=16_384).contains(&h))
}

fn token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn next_item_id() -> Result<String, AnalysisErrorCode> {
    NEXT_ITEM_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            (value < MAX_SAFE_INTEGER).then_some(value + 1)
        })
        .map(|value| value.to_string())
        .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)
}

struct NoopSink;
impl ProcessEventSink for NoopSink {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
}
