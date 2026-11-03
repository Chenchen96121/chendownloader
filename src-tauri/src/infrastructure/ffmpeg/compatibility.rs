use crate::domain::{
    download::DownloadErrorCode,
    output_policy::{StreamProcessing, TargetAudioCodec},
};
use crate::infrastructure::ffprobe::stream_properties::{
    frame_rate, AudioStreamProperties, VideoStreamProperties,
};

#[derive(Clone, Debug, PartialEq)]
pub struct CompatibilityDecision {
    pub video_processing: Option<StreamProcessing>,
    pub audio_processing: Option<StreamProcessing>,
    pub hdr_to_sdr: bool,
}

pub fn decide_compatibility(
    video: Option<&VideoStreamProperties>,
    audio: Option<&AudioStreamProperties>,
) -> Result<CompatibilityDecision, DownloadErrorCode> {
    if video.is_none() && audio.is_none() {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let filter = video.map(compatibility_filter).transpose()?.flatten();
    let video_processing = video.map(|v| {
        if filter.is_none()
            && v.codec == "h264"
            && v.pixel_format.as_deref() == Some("yuv420p")
            && matches!(
                v.profile.as_deref(),
                Some("Baseline" | "Constrained Baseline" | "Main" | "High")
            )
        {
            StreamProcessing::Copy
        } else {
            StreamProcessing::Transcode
        }
    });
    let audio_processing = if let Some(a) = audio {
        let layout_matches = matches!(
            (a.channels, a.channel_layout.as_deref()),
            (1, Some("mono")) | (2, Some("stereo")) | (6, Some("5.1"))
        );
        if !layout_matches || !matches!(a.sample_rate_hz, 44100 | 48000) {
            return Err(DownloadErrorCode::FormatUnavailable);
        }
        Some(if a.codec == TargetAudioCodec::Aac {
            StreamProcessing::Copy
        } else {
            StreamProcessing::Transcode
        })
    } else {
        None
    };
    Ok(CompatibilityDecision {
        video_processing,
        audio_processing,
        hdr_to_sdr: filter.is_some(),
    })
}

pub fn compatibility_filter(
    video: &VideoStreamProperties,
) -> Result<Option<String>, DownloadErrorCode> {
    crate::infrastructure::ffprobe::stream_properties::normalized_hdr_side_data(
        &video.hdr_side_data,
    )?;
    if !video.width.is_multiple_of(2)
        || !video.height.is_multiple_of(2)
        || frame_rate(&video.frame_rate).is_none()
        || !matches!(
            video.pixel_format.as_deref(),
            Some("yuv420p" | "yuv420p10le" | "yuv444p")
        )
        || !matches!(video.color_range.as_deref(), Some("tv" | "pc"))
    {
        return Err(DownloadErrorCode::FormatUnavailable);
    }
    if video.color_transfer.as_deref() == Some("bt709")
        && video.color_space.as_deref() == Some("bt709")
        && video.color_primaries.as_deref() == Some("bt709")
        && video.hdr_side_data.is_empty()
    {
        return Ok(None);
    }
    if matches!(
        video.color_transfer.as_deref(),
        Some("smpte2084" | "arib-std-b67")
    ) && video.color_space.as_deref() == Some("bt2020nc")
        && video.color_primaries.as_deref() == Some("bt2020")
        && video.color_range.as_deref() == Some("tv")
        && video.pixel_format.as_deref() == Some("yuv420p10le")
    {
        // Pinned-tool independent PQ/HLG RGB/4:2:0 reference proof, not retagging.
        // Delete static HDR side data only AFTER actual linear-light tone mapping.
        return Ok(Some("zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p,sidedata=mode=delete:type=MASTERING_DISPLAY_METADATA,sidedata=mode=delete:type=CONTENT_LIGHT_LEVEL".into()));
    }
    Err(DownloadErrorCode::FormatUnavailable)
}
