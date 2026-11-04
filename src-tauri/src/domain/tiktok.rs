use std::collections::HashSet;

use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

use super::{
    download::{valid_opaque_id, DownloadErrorCode},
    media::{AnalysisErrorCode, FileSize},
};

const MAX_ITEMS: usize = 100;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_PREVIEW_BASE64_BYTES: usize = 11_184_812;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TikTokMediaKind {
    Image,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TikTokItemAvailability {
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TikTokJobState {
    Queued,
    Downloading,
    Processing,
    Verifying,
    Publishing,
    Completed,
    Partial,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TikTokItemOutcomeState {
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokCapabilities {
    pub post_batch: bool,
    pub images: bool,
    pub videos: bool,
    pub compatibility: bool,
    pub clip: bool,
    pub manual_quality: bool,
    pub audio_only: bool,
    pub video_only: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokCapabilitiesWire {
    post_batch: bool,
    images: bool,
    videos: bool,
    compatibility: bool,
    clip: bool,
    manual_quality: bool,
    audio_only: bool,
    video_only: bool,
}

impl TikTokCapabilities {
    pub const PUBLIC_MEDIA: Self = Self {
        post_batch: true,
        images: true,
        videos: true,
        compatibility: true,
        clip: false,
        manual_quality: false,
        audio_only: false,
        video_only: false,
    };
}

impl<'de> Deserialize<'de> for TikTokCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokCapabilitiesWire::deserialize(deserializer)?;
        let value = Self {
            post_batch: wire.post_batch,
            images: wire.images,
            videos: wire.videos,
            compatibility: wire.compatibility,
            clip: wire.clip,
            manual_quality: wire.manual_quality,
            audio_only: wire.audio_only,
            video_only: wire.video_only,
        };
        if value != Self::PUBLIC_MEDIA {
            return Err(D::Error::custom("invalid TikTok capabilities"));
        }
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokMediaItem {
    pub item_id: String,
    pub ordinal: u16,
    pub kind: TikTokMediaKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<FileSize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_bitrate_kbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bitrate_kbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_range: Option<String>,
    pub preview_available: bool,
    pub availability: TikTokItemAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<AnalysisErrorCode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokMediaItemWire {
    item_id: String,
    ordinal: u16,
    kind: TikTokMediaKind,
    width: Option<u32>,
    height: Option<u32>,
    container: String,
    file_size: Option<FileSize>,
    fps: Option<f64>,
    video_bitrate_kbps: Option<f64>,
    audio_bitrate_kbps: Option<f64>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    dynamic_range: Option<String>,
    preview_available: bool,
    availability: TikTokItemAvailability,
    unavailable_reason: Option<AnalysisErrorCode>,
}

impl TikTokMediaItem {
    pub fn validate(&self) -> Result<(), AnalysisErrorCode> {
        let availability_valid = matches!(
            (self.availability, self.unavailable_reason),
            (TikTokItemAvailability::Ready, None) | (TikTokItemAvailability::Unavailable, Some(_))
        );
        let media_valid = match self.kind {
            TikTokMediaKind::Image => {
                self.fps.is_none()
                    && self.video_bitrate_kbps.is_none()
                    && self.audio_bitrate_kbps.is_none()
                    && self.video_codec.is_none()
                    && self.audio_codec.is_none()
                    && self.dynamic_range.is_none()
            }
            TikTokMediaKind::Video => {
                (self.video_bitrate_kbps.is_none() && self.dynamic_range.is_none()
                    || self.video_codec.is_some())
                    && (self.audio_bitrate_kbps.is_none() || self.audio_codec.is_some())
            }
        };
        if !valid_opaque_id(&self.item_id)
            || !(1..=MAX_ITEMS as u16).contains(&self.ordinal)
            || !container(&self.container)
            || self
                .width
                .is_some_and(|value| !(1..=16_384).contains(&value))
            || self
                .height
                .is_some_and(|value| !(1..=16_384).contains(&value))
            || self
                .file_size
                .is_some_and(|value| value.bytes == 0 || value.bytes > MAX_SAFE_INTEGER)
            || self
                .fps
                .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 240.0)
            || self
                .video_bitrate_kbps
                .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1_000_000.0)
            || self
                .audio_bitrate_kbps
                .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 100_000.0)
            || self
                .video_codec
                .as_deref()
                .is_some_and(|value| !codec(value))
            || self
                .audio_codec
                .as_deref()
                .is_some_and(|value| !codec(value))
            || self.dynamic_range.as_deref().is_some_and(|value| {
                !matches!(
                    value,
                    "SDR" | "HDR" | "HDR10" | "HDR10+" | "HDR12" | "HLG" | "DV"
                )
            })
            || !availability_valid
            || !media_valid
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TikTokMediaItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokMediaItemWire::deserialize(deserializer)?;
        let value = Self {
            item_id: wire.item_id,
            ordinal: wire.ordinal,
            kind: wire.kind,
            width: wire.width,
            height: wire.height,
            container: wire.container,
            file_size: wire.file_size,
            fps: wire.fps,
            video_bitrate_kbps: wire.video_bitrate_kbps,
            audio_bitrate_kbps: wire.audio_bitrate_kbps,
            video_codec: wire.video_codec,
            audio_codec: wire.audio_codec,
            dynamic_range: wire.dynamic_range,
            preview_available: wire.preview_available,
            availability: wire.availability,
            unavailable_reason: wire.unavailable_reason,
        };
        value
            .validate()
            .map_err(|_| D::Error::custom("invalid TikTok media item"))?;
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokAnalysisPreview {
    pub analysis_id: String,
    pub post_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    pub source_page_url: String,
    pub capabilities: TikTokCapabilities,
    pub items: Vec<TikTokMediaItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokAnalysisPreviewWire {
    analysis_id: String,
    post_id: String,
    title: String,
    creator: Option<String>,
    source_page_url: String,
    capabilities: TikTokCapabilities,
    items: Vec<TikTokMediaItem>,
}

impl TikTokAnalysisPreview {
    pub fn validate(&self) -> Result<(), AnalysisErrorCode> {
        let valid_source = valid_source_page_url(&self.source_page_url, &self.post_id);
        let mut ids = HashSet::new();
        if !valid_opaque_id(&self.analysis_id)
            || !post_id(&self.post_id)
            || !display_text(&self.title, 512)
            || self
                .creator
                .as_deref()
                .is_some_and(|value| !display_text(value, 256))
            || !valid_source
            || self.capabilities != TikTokCapabilities::PUBLIC_MEDIA
            || !(1..=MAX_ITEMS).contains(&self.items.len())
            || self.items.iter().enumerate().any(|(index, item)| {
                item.ordinal != (index + 1) as u16
                    || item.validate().is_err()
                    || !ids.insert(&item.item_id)
            })
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TikTokAnalysisPreview {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokAnalysisPreviewWire::deserialize(deserializer)?;
        let value = Self {
            analysis_id: wire.analysis_id,
            post_id: wire.post_id,
            title: wire.title,
            creator: wire.creator,
            source_page_url: wire.source_page_url,
            capabilities: wire.capabilities,
            items: wire.items,
        };
        value
            .validate()
            .map_err(|_| D::Error::custom("invalid TikTok analysis preview"))?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokPreviewPayload {
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokPreviewPayloadWire {
    mime_type: String,
    data_base64: String,
}

impl<'de> Deserialize<'de> for TikTokPreviewPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokPreviewPayloadWire::deserialize(deserializer)?;
        if !matches!(
            wire.mime_type.as_str(),
            "image/jpeg" | "image/png" | "image/webp" | "image/avif"
        ) || !base64_payload(&wire.data_base64)
        {
            return Err(D::Error::custom("invalid TikTok preview payload"));
        }
        Ok(Self {
            mime_type: wire.mime_type,
            data_base64: wire.data_base64,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokDownloadIntent {
    pub item_ids: Vec<String>,
    pub compatibility_mode: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokDownloadIntentWire {
    item_ids: Vec<String>,
    compatibility_mode: bool,
}

impl TikTokDownloadIntent {
    pub fn validate(&self) -> Result<(), DownloadErrorCode> {
        let mut ids = HashSet::new();
        if !(1..=MAX_ITEMS).contains(&self.item_ids.len())
            || self
                .item_ids
                .iter()
                .any(|id| !valid_opaque_id(id) || !ids.insert(id))
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TikTokDownloadIntent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokDownloadIntentWire::deserialize(deserializer)?;
        let value = Self {
            item_ids: wire.item_ids,
            compatibility_mode: wire.compatibility_mode,
        };
        value
            .validate()
            .map_err(|_| D::Error::custom("invalid TikTok download intent"))?;
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokItemOutcome {
    pub item_id: String,
    pub ordinal: u16,
    pub kind: TikTokMediaKind,
    pub state: TikTokItemOutcomeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<DownloadErrorCode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokItemOutcomeWire {
    item_id: String,
    ordinal: u16,
    kind: TikTokMediaKind,
    state: TikTokItemOutcomeState,
    display_name: Option<String>,
    error_code: Option<DownloadErrorCode>,
}

impl TikTokItemOutcome {
    fn valid(&self) -> bool {
        valid_opaque_id(&self.item_id)
            && (1..=MAX_ITEMS as u16).contains(&self.ordinal)
            && self
                .display_name
                .as_deref()
                .is_none_or(|value| safe_display_name(value, 160))
            && matches!(
                (
                    self.state,
                    self.display_name.is_some(),
                    self.error_code.is_some()
                ),
                (TikTokItemOutcomeState::Completed, true, false)
                    | (TikTokItemOutcomeState::Failed, false, true)
                    | (TikTokItemOutcomeState::Canceled, false, false)
            )
    }
}

impl<'de> Deserialize<'de> for TikTokItemOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokItemOutcomeWire::deserialize(deserializer)?;
        let value = Self {
            item_id: wire.item_id,
            ordinal: wire.ordinal,
            kind: wire.kind,
            state: wire.state,
            display_name: wire.display_name,
            error_code: wire.error_code,
        };
        if !value.valid() {
            return Err(D::Error::custom("invalid TikTok item outcome"));
        }
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokDownloadProgress {
    pub job_id: String,
    pub state: TikTokJobState,
    pub item_index: u16,
    pub item_count: u16,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokDownloadProgressWire {
    job_id: String,
    state: TikTokJobState,
    item_index: u16,
    item_count: u16,
    #[serde(deserialize_with = "deserialize_required_option")]
    downloaded_bytes: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    total_bytes: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    percent: Option<f64>,
}

impl TikTokDownloadProgress {
    pub fn validate(&self) -> Result<(), DownloadErrorCode> {
        if !valid_opaque_id(&self.job_id)
            || !(1..=MAX_ITEMS as u16).contains(&self.item_count)
            || !(1..=self.item_count).contains(&self.item_index)
            || self
                .downloaded_bytes
                .is_some_and(|value| value > MAX_SAFE_INTEGER)
            || self
                .total_bytes
                .is_some_and(|value| value == 0 || value > MAX_SAFE_INTEGER)
            || matches!((self.downloaded_bytes, self.total_bytes), (Some(done), Some(total)) if done > total)
            || self.percent.is_some_and(|value| {
                !value.is_finite()
                    || !(0.0..=100.0).contains(&value)
                    || self.downloaded_bytes.is_none()
                    || self.total_bytes.is_none()
            })
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TikTokDownloadProgress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokDownloadProgressWire::deserialize(deserializer)?;
        let value = Self {
            job_id: wire.job_id,
            state: wire.state,
            item_index: wire.item_index,
            item_count: wire.item_count,
            downloaded_bytes: wire.downloaded_bytes,
            total_bytes: wire.total_bytes,
            percent: wire.percent,
        };
        value
            .validate()
            .map_err(|_| D::Error::custom("invalid TikTok download progress"))?;
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokDownloadResult {
    pub job_id: String,
    pub state: TikTokJobState,
    pub post_item_count: u16,
    pub unavailable_count: u16,
    pub content_changed: bool,
    pub completed_count: u16,
    pub failed_count: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_display_name: Option<String>,
    pub items: Vec<TikTokItemOutcome>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokDownloadResultWire {
    job_id: String,
    state: TikTokJobState,
    post_item_count: u16,
    unavailable_count: u16,
    content_changed: bool,
    completed_count: u16,
    failed_count: u16,
    folder_display_name: Option<String>,
    items: Vec<TikTokItemOutcome>,
}

impl TikTokDownloadResult {
    pub fn validate(&self) -> Result<(), DownloadErrorCode> {
        let mut ids = HashSet::new();
        let mut previous_ordinal = 0;
        let completed = self
            .items
            .iter()
            .filter(|item| item.state == TikTokItemOutcomeState::Completed)
            .count();
        let failed = self
            .items
            .iter()
            .filter(|item| item.state == TikTokItemOutcomeState::Failed)
            .count();
        let canceled = self
            .items
            .iter()
            .filter(|item| item.state == TikTokItemOutcomeState::Canceled)
            .count();
        let state_valid = match self.state {
            TikTokJobState::Completed => completed == self.items.len(),
            TikTokJobState::Partial => completed > 0 && failed > 0 && canceled == 0,
            TikTokJobState::Failed => completed == 0 && failed == self.items.len(),
            TikTokJobState::Canceled => canceled > 0,
            _ => false,
        };
        let folder_valid = match (completed > 0, &self.folder_display_name) {
            (true, Some(value)) => safe_display_name(value, 160),
            (false, None) => true,
            _ => false,
        };
        if !valid_opaque_id(&self.job_id)
            || !(1..=MAX_ITEMS).contains(&self.items.len())
            || self.post_item_count == 0
            || self.post_item_count > 100
            || self.items.len() + self.unavailable_count as usize > self.post_item_count as usize
            || self
                .items
                .iter()
                .any(|item| item.ordinal > self.post_item_count)
            || self.completed_count as usize != completed
            || self.failed_count as usize != failed
            || !state_valid
            || !folder_valid
            || self.items.iter().any(|item| {
                let invalid =
                    item.ordinal <= previous_ordinal || !item.valid() || !ids.insert(&item.item_id);
                previous_ordinal = item.ordinal;
                invalid
            })
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TikTokDownloadResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TikTokDownloadResultWire::deserialize(deserializer)?;
        let value = Self {
            job_id: wire.job_id,
            state: wire.state,
            post_item_count: wire.post_item_count,
            unavailable_count: wire.unavailable_count,
            content_changed: wire.content_changed,
            completed_count: wire.completed_count,
            failed_count: wire.failed_count,
            folder_display_name: wire.folder_display_name,
            items: wire.items,
        };
        value
            .validate()
            .map_err(|_| D::Error::custom("invalid TikTok download result"))?;
        Ok(value)
    }
}

fn post_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit())
}

fn container(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn codec(value: &str) -> bool {
    value != "none"
        && !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn display_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value == value.trim()
        && value.chars().count() <= max
        && !value.chars().any(char::is_control)
}

fn safe_display_name(value: &str, max: usize) -> bool {
    display_text(value, max)
        && !value
            .chars()
            .any(|character| matches!(character, '/' | '\\' | ':') || is_bidi_control(character))
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn base64_payload(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_PREVIEW_BASE64_BYTES || !value.len().is_multiple_of(4)
    {
        return false;
    }
    let bytes = value.as_bytes();
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    padding <= 2
        && bytes[..bytes.len() - padding]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
        && bytes[bytes.len() - padding..]
            .iter()
            .all(|byte| *byte == b'=')
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn valid_source_page_url(value: &str, post_id: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let parts = url.path().split('/').collect::<Vec<_>>();
    url.scheme() == "https"
        && url.host_str() == Some("www.tiktok.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && parts.len() == 4
        && parts[1].strip_prefix('@').is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        })
        && matches!(parts[2], "video" | "photo")
        && parts[3] == post_id
}
