use local_media_downloader_lib::domain::{
    mux_capabilities::{lossless_audio_container, lossless_video_containers},
    output_policy::{OutputContainer as C, TargetAudioCodec as A},
};

#[test]
fn lossless_container_priority_preserves_the_original_audio_codec() {
    for (video, audio, expected) in [
        ("h264", Some(A::Opus), vec![C::Mp4, C::Mkv]),
        ("hevc", Some(A::Flac), vec![C::Mp4, C::Mkv]),
        ("av1", Some(A::Opus), vec![C::Mp4, C::Webm, C::Mkv]),
        ("vp9", Some(A::Aac), vec![C::Mp4, C::Mkv]),
        ("vp8", Some(A::Opus), vec![C::Webm, C::Mkv]),
        ("vp8", Some(A::Aac), vec![C::Mkv]),
        ("vp8", None, vec![C::Webm, C::Mkv]),
        ("unknown", Some(A::Aac), vec![]),
    ] {
        assert_eq!(lossless_video_containers(video, audio), expected);
    }
}

#[test]
fn audio_only_uses_a_lossless_container_for_each_known_codec() {
    for (audio, expected) in [
        (A::Aac, C::M4a),
        (A::Alac, C::M4a),
        (A::Opus, C::Webm),
        (A::Vorbis, C::Webm),
        (A::Mp3, C::Mp3),
        (A::Flac, C::Flac),
    ] {
        assert_eq!(lossless_audio_container(audio), expected);
    }
}
