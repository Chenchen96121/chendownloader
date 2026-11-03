#![allow(dead_code)]
use local_media_downloader_lib::{
    domain::media::{MediaFormat, StreamKind},
    infrastructure::ffprobe::stream_properties::VideoStreamProperties,
};
pub fn video_format(id: &str, codec: &str) -> MediaFormat {
    MediaFormat {
        id: id.into(),
        container: "mp4".into(),
        kind: StreamKind::Combined,
        width: Some(1280),
        height: Some(720),
        fps: Some(30.0),
        video_codec: Some(codec.into()),
        audio_codec: Some("aac".into()),
        audio_channels: Some(2),
        audio_sample_rate_hz: Some(48000),
        video_bitrate_kbps: None,
        audio_bitrate_kbps: None,
        dynamic_range: None,
        file_size: None,
    }
}
pub fn video_proof() -> VideoStreamProperties {
    VideoStreamProperties {
        profile: None,
        hdr_side_data: vec![],
        codec: "h264".into(),
        width: 1280,
        height: 720,
        frame_rate: "30/1".into(),
        pixel_format: Some("yuv420p".into()),
        color_range: None,
        color_space: None,
        color_transfer: None,
        color_primaries: None,
    }
}

pub fn audio_format(id: &str, codec: &str) -> MediaFormat {
    let mut row = video_format(id, "h264");
    row.audio_codec = Some(codec.into());
    row
}

pub fn evidence(
    plan: &local_media_downloader_lib::domain::format_selection::SelectionPlan,
) -> local_media_downloader_lib::application::local_processing::ProcessingEvidence {
    use local_media_downloader_lib::{
        domain::output_policy::{OutputContainer, StreamProcessing},
        infrastructure::ffprobe::stream_properties::AudioStreamProperties,
    };
    local_media_downloader_lib::application::local_processing::ProcessingEvidence {
        source_video: plan.video_format_id.as_ref().map(|_| video_proof()),
        source_audio: plan
            .audio_format_id
            .as_ref()
            .map(|_| AudioStreamProperties {
                codec: plan.output_audio_codec.unwrap(),
                channels: 2,
                sample_rate_hz: 48000,
                channel_layout: None,
            }),
        actual_container: OutputContainer::parse(plan.output_container.as_deref().unwrap())
            .unwrap(),
        video_processing: plan
            .video_format_id
            .as_ref()
            .map(|_| StreamProcessing::Copy),
        audio_processing: plan
            .audio_format_id
            .as_ref()
            .map(|_| StreamProcessing::Copy),
    }
}

pub fn slots(
    ids: &[String],
) -> Vec<local_media_downloader_lib::application::local_processing::DownloadedSource> {
    ids.iter()
        .enumerate()
        .map(|(slot, id)| {
            local_media_downloader_lib::application::local_processing::DownloadedSource {
                format_id: id.clone(),
                slot,
            }
        })
        .collect()
}
