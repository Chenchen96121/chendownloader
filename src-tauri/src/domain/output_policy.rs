#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoContainer {
    Mp4,
    Webm,
}

impl VideoContainer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
        }
    }

    pub fn audio_codec(self) -> TargetAudioCodec {
        match self {
            Self::Mp4 => TargetAudioCodec::Aac,
            Self::Webm => TargetAudioCodec::Opus,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetAudioCodec {
    Aac,
    Opus,
    Vorbis,
    Mp3,
    Flac,
    Alac,
}

impl TargetAudioCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Alac => "alac",
        }
    }
    pub fn matches(self, codec: Option<&str>) -> bool {
        normalized_audio_codec(codec) == Some(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamProcessing {
    Copy,
    Transcode,
}

pub type AudioProcessing = StreamProcessing;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputContainer {
    Mp4,
    Webm,
    Mkv,
    M4a,
    Mp3,
    Flac,
}
impl OutputContainer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Mkv => "mkv",
            Self::M4a => "m4a",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mp4" => Some(Self::Mp4),
            "webm" => Some(Self::Webm),
            "mkv" => Some(Self::Mkv),
            "m4a" => Some(Self::M4a),
            "mp3" => Some(Self::Mp3),
            "flac" => Some(Self::Flac),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerSelection {
    Automatic,
    Explicit,
    Compatibility,
}

pub fn normalized_audio_codec(codec: Option<&str>) -> Option<TargetAudioCodec> {
    match codec?.to_ascii_lowercase().as_str() {
        "aac" | "mp4a.40.2" | "mp4a.40.5" | "mp4a.40.29" => Some(TargetAudioCodec::Aac),
        "opus" => Some(TargetAudioCodec::Opus),
        "vorbis" => Some(TargetAudioCodec::Vorbis),
        "mp3" => Some(TargetAudioCodec::Mp3),
        "flac" => Some(TargetAudioCodec::Flac),
        "alac" => Some(TargetAudioCodec::Alac),
        _ => None,
    }
}

pub fn normalized_video_codec(codec: Option<&str>) -> Option<&'static str> {
    let codec = codec?.split('.').next()?.to_ascii_lowercase();
    match codec.as_str() {
        "av1" | "av01" => Some("av1"),
        "vp9" | "vp09" => Some("vp9"),
        "vp8" | "vp08" => Some("vp8"),
        "hevc" | "h265" | "hev1" | "hvc1" => Some("hevc"),
        "h264" | "avc1" | "avc3" => Some("h264"),
        _ => None,
    }
}

pub fn supported_video_containers(codec: Option<&str>) -> &'static [VideoContainer] {
    match normalized_video_codec(codec) {
        Some("av1" | "vp9") => &[VideoContainer::Mp4, VideoContainer::Webm],
        Some("h264" | "hevc") => &[VideoContainer::Mp4],
        Some("vp8") => &[VideoContainer::Webm],
        _ => &[],
    }
}

pub fn default_video_container(codec: Option<&str>) -> Option<VideoContainer> {
    supported_video_containers(codec).first().copied()
}
