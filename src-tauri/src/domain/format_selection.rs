use std::cmp::Ordering;

use super::media::{MediaFormat, MediaInfo, StreamKind};
use super::mux_capabilities::{lossless_audio_container, lossless_video_containers};
use super::output_policy::{
    normalized_audio_codec, normalized_video_codec, AudioProcessing, ContainerSelection,
    OutputContainer, StreamProcessing, TargetAudioCodec, VideoContainer,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    BestAvailable,
    VideoOnly,
    AudioOnly,
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipRange {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionIntent {
    #[serde(default)]
    pub compatibility_mode: bool,
    pub mode: SelectionMode,
    pub video_format_id: Option<String>,
    pub output_container: Option<VideoContainer>,
    pub clip: Option<ClipRange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionOperation {
    Keep,
    Merge,
    ExtractAudio,
    ExtractVideo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionWarning {
    MetadataIncomplete,
    ContainerUnverified,
    OutputSizeUnknown,
    ClipBoundaryMayVary,
    ClipTransferMayExceedRange,
    CompatibilityMayReencode,
    HdrToSdr,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionPlan {
    pub compatibility_mode: bool,
    pub container_selection: ContainerSelection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_processing: Option<StreamProcessing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_video_codec: Option<String>,
    pub mode: SelectionMode,
    pub source_format_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_format_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_format_id: Option<String>,
    pub operation: SelectionOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_audio_codec: Option<TargetAudioCodec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_processing: Option<AudioProcessing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_size_bytes: Option<u64>,
    pub warnings: Vec<SelectionWarning>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionUnavailableCode {
    FormatUnavailable,
    InvalidSelection,
    InvalidClip,
    ClipDurationUnknown,
    CompatibilityUnavailable,
}

/// Sources rank independently of container; conversion requires an explicit opt-in.
pub fn select_formats(
    media: &MediaInfo,
    intent: &SelectionIntent,
) -> Result<SelectionPlan, SelectionUnavailableCode> {
    let selectable = matches!(
        intent.mode,
        SelectionMode::Manual | SelectionMode::VideoOnly
    );
    if !selectable && (intent.video_format_id.is_some() || intent.output_container.is_some()) {
        return Err(SelectionUnavailableCode::InvalidSelection);
    }
    validate_clip(media, intent)?;
    let unavailable = if selectable {
        SelectionUnavailableCode::InvalidSelection
    } else {
        SelectionUnavailableCode::FormatUnavailable
    };
    let video = match intent.mode {
        SelectionMode::AudioOnly => None,
        SelectionMode::Manual => Some(find_manual_format(
            media,
            intent.video_format_id.as_deref(),
        )?),
        SelectionMode::VideoOnly | SelectionMode::BestAvailable => {
            Some(match intent.video_format_id.as_deref() {
                Some(id) => find_manual_format(media, Some(id))?,
                None => ranked_formats(media, true)
                    .into_iter()
                    .next()
                    .ok_or(unavailable)?,
            })
        }
    };
    let audio = if intent.mode == SelectionMode::VideoOnly {
        None
    } else {
        Some(
            ranked_formats(media, false)
                .into_iter()
                .next()
                .ok_or(unavailable)?,
        )
    };
    let native_video = video
        .map(|v| normalized_video_codec(v.video_codec.as_deref()).ok_or(unavailable))
        .transpose()?;
    let native_audio = audio
        .map(|a| normalized_audio_codec(a.audio_codec.as_deref()).ok_or(unavailable))
        .transpose()?;
    if intent.compatibility_mode
        && video.is_some_and(|v| {
            v.width.is_some_and(|n| n % 2 != 0)
                || v.height.is_some_and(|n| n % 2 != 0)
                || matches!(v.dynamic_range.as_deref(), Some("DV" | "HDR10+" | "HDR12"))
        })
    {
        return Err(SelectionUnavailableCode::CompatibilityUnavailable);
    }
    let container_selection = if intent.compatibility_mode {
        ContainerSelection::Compatibility
    } else if intent.output_container.is_some() {
        ContainerSelection::Explicit
    } else {
        ContainerSelection::Automatic
    };
    let container = if intent.compatibility_mode {
        if intent.output_container == Some(VideoContainer::Webm) {
            return Err(SelectionUnavailableCode::InvalidSelection);
        }
        if video.is_some() {
            OutputContainer::Mp4
        } else {
            OutputContainer::M4a
        }
    } else if let Some(codec) = native_video {
        let candidates = lossless_video_containers(codec, native_audio);
        if let Some(requested) = intent.output_container {
            let requested = OutputContainer::parse(requested.as_str()).ok_or(unavailable)?;
            if !candidates.contains(&requested) {
                return Err(unavailable);
            }
            requested
        } else {
            *candidates.first().ok_or(unavailable)?
        }
    } else {
        lossless_audio_container(native_audio.ok_or(unavailable)?)
    };
    let target_audio = native_audio.map(|codec| {
        if intent.compatibility_mode {
            TargetAudioCodec::Aac
        } else {
            codec
        }
    });
    // Metadata lacks H.264 profile/pixel format. Real source probe may refine this approved
    // conversion to copy, but only compatibility mode may ever refine copy to conversion.
    let video_processing = video.map(|_| {
        if intent.compatibility_mode {
            StreamProcessing::Transcode
        } else {
            StreamProcessing::Copy
        }
    });
    let operation = match (video, audio) {
        (Some(video), None) if video.kind == StreamKind::Combined => {
            SelectionOperation::ExtractVideo
        }
        (Some(video), Some(audio)) if video.id != audio.id => SelectionOperation::Merge,
        (None, Some(audio)) if audio.kind == StreamKind::Combined => {
            SelectionOperation::ExtractAudio
        }
        _ => SelectionOperation::Keep,
    };
    let source_format_ids = match (video, audio) {
        (Some(video), Some(audio)) if video.id != audio.id => {
            vec![video.id.clone(), audio.id.clone()]
        }
        (Some(video), _) => vec![video.id.clone()],
        (None, Some(audio)) => vec![audio.id.clone()],
        _ => return Err(unavailable),
    };
    let audio_processing = target_audio.zip(native_audio).map(|(target, original)| {
        if target == original {
            AudioProcessing::Copy
        } else {
            AudioProcessing::Transcode
        }
    });
    let estimated_size_bytes = if !intent.compatibility_mode
        && intent.clip.is_none()
        && operation != SelectionOperation::ExtractVideo
        && operation != SelectionOperation::ExtractAudio
        && audio_processing != Some(AudioProcessing::Transcode)
    {
        (video.or(audio)).and_then(|source| {
            let bytes = if operation == SelectionOperation::Merge {
                if video?.kind != StreamKind::VideoOnly || audio?.kind != StreamKind::AudioOnly {
                    return None;
                }
                source
                    .file_size?
                    .bytes
                    .checked_add(audio?.file_size?.bytes)?
            } else {
                source.file_size?.bytes
            };
            (bytes <= 9_007_199_254_740_991).then_some(bytes)
        })
    } else {
        None
    };
    let incomplete_video = video.is_some_and(|video| {
        video.width.is_none()
            || video.height.is_none()
            || video.fps.is_none()
            || video.dynamic_range.is_none()
    });
    let incomplete_audio = audio.is_some_and(|audio| {
        audio.audio_channels.is_none()
            || audio.audio_bitrate_kbps.is_none()
            || audio.audio_sample_rate_hz.is_none()
    });
    let mut warnings = Vec::new();
    if incomplete_video || incomplete_audio {
        warnings.push(SelectionWarning::MetadataIncomplete);
    }
    warnings.push(SelectionWarning::ContainerUnverified);
    if estimated_size_bytes.is_none() {
        warnings.push(SelectionWarning::OutputSizeUnknown);
    }
    if intent.clip.is_some() {
        warnings.push(SelectionWarning::ClipBoundaryMayVary);
        warnings.push(SelectionWarning::ClipTransferMayExceedRange);
    }
    if intent.compatibility_mode {
        warnings.push(SelectionWarning::CompatibilityMayReencode);
        if video
            .is_some_and(|v| matches!(v.dynamic_range.as_deref(), Some("HDR" | "HDR10" | "HLG")))
        {
            warnings.push(SelectionWarning::HdrToSdr);
        }
    }
    Ok(SelectionPlan {
        compatibility_mode: intent.compatibility_mode,
        container_selection,
        video_processing,
        output_video_codec: native_video
            .map(|codec| {
                if intent.compatibility_mode {
                    "h264"
                } else {
                    codec
                }
            })
            .map(str::to_owned),
        mode: intent.mode,
        source_format_ids,
        video_format_id: video.map(|video| video.id.clone()),
        audio_format_id: audio.map(|audio| audio.id.clone()),
        operation,
        output_container: Some(container.as_str().into()),
        output_audio_codec: target_audio,
        audio_processing,
        clip: intent.clip,
        estimated_size_bytes,
        warnings,
    })
}

fn validate_clip(
    media: &MediaInfo,
    intent: &SelectionIntent,
) -> Result<(), SelectionUnavailableCode> {
    let Some(clip) = intent.clip else {
        return Ok(());
    };
    if intent.mode == SelectionMode::AudioOnly {
        return Err(SelectionUnavailableCode::InvalidSelection);
    }
    let duration = media
        .duration_seconds
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or(SelectionUnavailableCode::ClipDurationUnknown)?;
    if clip.start_ms >= clip.end_ms
        || clip.start_ms > 31_536_000_000
        || clip.end_ms > 31_536_000_000
        || clip.end_ms as f64 > (duration * 1000.0).floor()
    {
        return Err(SelectionUnavailableCode::InvalidClip);
    }
    Ok(())
}

fn find_manual_format<'a>(
    media: &'a MediaInfo,
    id: Option<&str>,
) -> Result<&'a MediaFormat, SelectionUnavailableCode> {
    let id = id
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                })
        })
        .ok_or(SelectionUnavailableCode::InvalidSelection)?;
    media
        .formats
        .iter()
        .find(|format| format.id == id)
        .ok_or(SelectionUnavailableCode::InvalidSelection)
}

fn video_group(codec: Option<&str>) -> u8 {
    let Some(codec) = codec else { return 0 };
    let alias = codec.split('.').next().unwrap_or_default();
    if ["av1", "av01"]
        .iter()
        .any(|name| alias.eq_ignore_ascii_case(name))
    {
        4
    } else if ["vp9", "vp09"]
        .iter()
        .any(|name| alias.eq_ignore_ascii_case(name))
    {
        3
    } else if ["hevc", "h265", "hev1", "hvc1"]
        .iter()
        .any(|name| alias.eq_ignore_ascii_case(name))
    {
        2
    } else if ["h264", "avc1", "avc3"]
        .iter()
        .any(|name| alias.eq_ignore_ascii_case(name))
    {
        1
    } else {
        0
    }
}

fn audio_group(codec: Option<&str>) -> u8 {
    match codec.map(str::to_ascii_lowercase).as_deref() {
        Some("flac" | "alac") => 5,
        Some("opus") => 4,
        Some("vorbis") => 3,
        Some("aac" | "mp4a.40.2" | "mp4a.40.5" | "mp4a.40.29") => 2,
        Some("mp3") => 1,
        _ => 0,
    }
}

fn range_group(format: &MediaFormat) -> u8 {
    match format.dynamic_range.as_deref() {
        Some("HDR" | "HDR10" | "HDR10+" | "HDR12" | "HLG") => 2,
        Some("SDR") => 1,
        _ => 0,
    }
}

fn short_edge(format: &MediaFormat) -> Option<u32> {
    format
        .width
        .zip(format.height)
        .map(|(width, height)| width.min(height))
}

fn compare_number(left: Option<f64>, right: Option<f64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

fn compare_video(left: &MediaFormat, right: &MediaFormat) -> Ordering {
    short_edge(left)
        .cmp(&short_edge(right))
        .then_with(|| compare_number(left.fps, right.fps))
        .then_with(|| range_group(left).cmp(&range_group(right)))
        .then_with(|| {
            video_group(left.video_codec.as_deref()).cmp(&video_group(right.video_codec.as_deref()))
        })
        .then_with(|| compare_number(left.video_bitrate_kbps, right.video_bitrate_kbps))
}

fn compare_audio(left: &MediaFormat, right: &MediaFormat) -> Ordering {
    left.audio_channels
        .cmp(&right.audio_channels)
        .then_with(|| {
            audio_group(left.audio_codec.as_deref()).cmp(&audio_group(right.audio_codec.as_deref()))
        })
        .then_with(|| compare_number(left.audio_bitrate_kbps, right.audio_bitrate_kbps))
        .then_with(|| left.audio_sample_rate_hz.cmp(&right.audio_sample_rate_hz))
        .then_with(|| {
            (left.kind == StreamKind::AudioOnly).cmp(&(right.kind == StreamKind::AudioOnly))
        })
}

fn ranked_formats(media: &MediaInfo, video: bool) -> Vec<&MediaFormat> {
    let mut formats: Vec<_> = media
        .formats
        .iter()
        .enumerate()
        .filter(|(_, format)| {
            if video {
                format.kind != StreamKind::AudioOnly
            } else {
                format.kind != StreamKind::VideoOnly
            }
        })
        .collect();
    formats.sort_by(|(left_index, left), (right_index, right)| {
        let quality = if video {
            compare_video(right, left)
        } else {
            compare_audio(right, left)
        };
        quality.then_with(|| left_index.cmp(right_index))
    });
    formats.into_iter().map(|(_, format)| format).collect()
}

/// X must prove the winning role instead of interpreting missing facts as low
/// quality. Other platforms keep their existing ranking behavior unchanged.
pub(crate) fn unproven_quality_format_ids(media: &MediaInfo) -> std::collections::HashSet<String> {
    let mut needed = std::collections::HashSet::new();
    for video in [true, false] {
        let ranked = ranked_formats(media, video);
        let Some(winner) = ranked.first() else {
            continue;
        };
        for contender in ranked.iter().skip(1) {
            let pairs = if video {
                vec![
                    (
                        short_edge(winner).map(f64::from),
                        short_edge(contender).map(f64::from),
                    ),
                    (winner.fps, contender.fps),
                    (
                        winner
                            .dynamic_range
                            .as_ref()
                            .map(|_| f64::from(range_group(winner))),
                        contender
                            .dynamic_range
                            .as_ref()
                            .map(|_| f64::from(range_group(contender))),
                    ),
                    (
                        Some(f64::from(video_group(winner.video_codec.as_deref()))),
                        Some(f64::from(video_group(contender.video_codec.as_deref()))),
                    ),
                    (winner.video_bitrate_kbps, contender.video_bitrate_kbps),
                ]
            } else {
                vec![
                    (
                        winner.audio_channels.map(f64::from),
                        contender.audio_channels.map(f64::from),
                    ),
                    (
                        Some(f64::from(audio_group(winner.audio_codec.as_deref()))),
                        Some(f64::from(audio_group(contender.audio_codec.as_deref()))),
                    ),
                    (winner.audio_bitrate_kbps, contender.audio_bitrate_kbps),
                    (
                        winner.audio_sample_rate_hz.map(f64::from),
                        contender.audio_sample_rate_hz.map(f64::from),
                    ),
                ]
            };
            for (left, right) in pairs {
                match (left, right) {
                    (Some(left), Some(right)) if left.total_cmp(&right) == Ordering::Equal => {}
                    (Some(_), Some(_)) => break,
                    _ => {
                        if left.is_none() {
                            needed.insert(winner.id.clone());
                        }
                        if right.is_none() {
                            needed.insert(contender.id.clone());
                        }
                        break;
                    }
                }
            }
        }
    }
    needed
}

/// Return video-bearing IDs in recommendation order without changing normalized media.
pub fn ranked_video_format_ids(media: &MediaInfo) -> Vec<String> {
    ranked_formats(media, true)
        .iter()
        .map(|format| format.id.clone())
        .collect()
}

/// Return all audio-bearing IDs, including combined sources, in recommendation order.
pub fn ranked_audio_format_ids(media: &MediaInfo) -> Vec<String> {
    ranked_formats(media, false)
        .iter()
        .map(|format| format.id.clone())
        .collect()
}
