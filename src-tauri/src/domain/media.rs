#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Youtube,
    Bilibili,
    X,
    Instagram,
    Tiktok,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisErrorCode {
    InvalidUrl,
    UnsupportedPlatform,
    UnsupportedMedia,
    InvalidMetadata,
    FormatUnavailable,
    AnalysisUnavailable,
    AnalysisExpired,
    ToolMissing,
    ToolIntegrityFailed,
    AuthenticationRequired,
    AccessDenied,
    ContentUnavailable,
    ContentChanged,
    RateLimited,
    ExtractorOutdated,
    NetworkFailed,
    AnalysisTimeout,
    ProcessFailed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    VideoOnly,
    AudioOnly,
    Combined,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub platform: Platform,
    pub media_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    pub formats: Vec<MediaFormat>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFormat {
    pub id: String,
    pub container: String,
    pub kind: StreamKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_bitrate_kbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bitrate_kbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_sample_rate_hz: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_channels: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<FileSize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSize {
    pub bytes: u64,
    pub is_approximate: bool,
}

impl MediaInfo {
    /// Recheck the sanitized boundary even for an injected metadata producer.
    pub fn validate(&self) -> Result<(), AnalysisErrorCode> {
        let mut ids = std::collections::HashSet::new();
        if !safe_token(&self.media_id, 128)
            || !display_text(&self.title, 512)
            || self
                .creator
                .as_deref()
                .is_some_and(|text| !display_text(text, 256))
            || self
                .duration_seconds
                .is_some_and(|value| !value.is_finite() || !(0.0..=31_536_000.0).contains(&value))
            || !(1..=512).contains(&self.formats.len())
            || self
                .formats
                .iter()
                .any(|row| !row.valid() || !ids.insert(&row.id))
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(())
    }
}

impl MediaFormat {
    fn valid(&self) -> bool {
        let codec_valid = |codec: Option<&str>| {
            codec.is_none_or(|token| token != "none" && safe_token(token, 64))
        };
        let streams_agree = matches!(
            (
                self.kind,
                self.video_codec.is_some(),
                self.audio_codec.is_some()
            ),
            (StreamKind::VideoOnly, true, false)
                | (StreamKind::AudioOnly, false, true)
                | (StreamKind::Combined, true, true)
        );
        safe_token(&self.id, 64)
            && !self.container.is_empty()
            && self.container.len() <= 16
            && self
                .container
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && codec_valid(self.video_codec.as_deref())
            && codec_valid(self.audio_codec.as_deref())
            && streams_agree
            && self.width.is_none_or(|value| (1..=16384).contains(&value))
            && self.height.is_none_or(|value| (1..=16384).contains(&value))
            && self
                .fps
                .is_none_or(|value| value.is_finite() && value > 0.0 && value <= 240.0)
            && self
                .video_bitrate_kbps
                .is_none_or(|value| value.is_finite() && value > 0.0 && value <= 1_000_000.0)
            && self
                .audio_bitrate_kbps
                .is_none_or(|value| value.is_finite() && value > 0.0 && value <= 100_000.0)
            && self
                .audio_sample_rate_hz
                .is_none_or(|value| (1..=768_000).contains(&value))
            && self
                .audio_channels
                .is_none_or(|value| (1..=64).contains(&value))
            && self
                .file_size
                .is_none_or(|size| (1..=9_007_199_254_740_991).contains(&size.bytes))
            && self.dynamic_range.as_deref().is_none_or(|value| {
                matches!(
                    value,
                    "SDR" | "HDR" | "HDR10" | "HDR10+" | "HDR12" | "HLG" | "DV"
                )
            })
            && (self.kind != StreamKind::AudioOnly
                || (self.width.is_none()
                    && self.height.is_none()
                    && self.fps.is_none()
                    && self.video_bitrate_kbps.is_none()
                    && self.dynamic_range.is_none()))
            && (self.kind != StreamKind::VideoOnly
                || (self.audio_bitrate_kbps.is_none()
                    && self.audio_sample_rate_hz.is_none()
                    && self.audio_channels.is_none()))
    }
}

fn safe_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
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
