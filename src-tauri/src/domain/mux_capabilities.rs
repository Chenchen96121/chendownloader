//! Closed codec combinations verified with the pinned FFmpeg, not extension guesses.
//! Container selection depends on the original video and audio codecs.
use super::output_policy::{OutputContainer, TargetAudioCodec};

pub fn lossless_video_containers(
    video: &str,
    audio: Option<TargetAudioCodec>,
) -> &'static [OutputContainer] {
    use OutputContainer::{Mkv, Mp4, Webm};
    let webm_audio = matches!(
        audio,
        None | Some(TargetAudioCodec::Opus | TargetAudioCodec::Vorbis)
    );
    match video {
        "h264" | "hevc" => &[Mp4, Mkv],
        "av1" | "vp9" if webm_audio => &[Mp4, Webm, Mkv],
        "av1" | "vp9" => &[Mp4, Mkv],
        "vp8" if webm_audio => &[Webm, Mkv],
        "vp8" => &[Mkv],
        _ => &[],
    }
}

pub fn lossless_audio_container(audio: TargetAudioCodec) -> OutputContainer {
    match audio {
        TargetAudioCodec::Aac | TargetAudioCodec::Alac => OutputContainer::M4a,
        TargetAudioCodec::Opus | TargetAudioCodec::Vorbis => OutputContainer::Webm,
        TargetAudioCodec::Mp3 => OutputContainer::Mp3,
        TargetAudioCodec::Flac => OutputContainer::Flac,
    }
}
