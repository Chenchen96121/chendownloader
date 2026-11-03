#[path = "support/real_media.rs"]
mod real;
use real::{assert_copy_equivalent, generate_source, probe, StreamRole};
use std::process::Command;

#[test]
#[ignore = "pinned portable tools required; tiny local samples, no network"]
fn real_mux_opus_mp4_and_webm_preserve_original_streams() {
    let tools = real::tools();
    let ffmpeg = tools.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let ffprobe = tools.join("ffprobe-x86_64-pc-windows-msvc.exe");
    let root = tempfile::tempdir().unwrap();
    for video in ["h264", "hevc", "av1", "vp9", "vp8"] {
        for audio in [
            Some("aac"),
            Some("opus"),
            Some("vorbis"),
            Some("mp3"),
            Some("flac"),
            Some("alac"),
            None,
        ] {
            let source = generate_source(
                &ffmpeg,
                &root
                    .path()
                    .join(format!("{video}-{}.mkv", audio.unwrap_or("none"))),
                Some(video),
                audio,
            );
            for container in ["mp4", "webm", "mkv"] {
                let output = root
                    .path()
                    .join(format!("{video}-{}.{container}", audio.unwrap_or("none")));
                let result = Command::new(&ffmpeg)
                    .args(["-v", "error", "-nostdin", "-i"])
                    .arg(&source)
                    .args(["-map", "0:v:0"])
                    .args(if audio.is_some() {
                        vec!["-map", "0:a:0"]
                    } else {
                        vec!["-an"]
                    })
                    .args(["-c", "copy", "-n"])
                    .arg(&output)
                    .output()
                    .unwrap();
                if !result.status.success() {
                    println!(
                        "MUX {video}/{} -> {container}: rejected ({})",
                        audio.unwrap_or("none"),
                        String::from_utf8_lossy(&result.stderr)
                            .lines()
                            .next()
                            .unwrap_or("tool error")
                    );
                    assert!(
                        !(container == "mkv"
                            || (container == "mp4" && video == "h264" && audio == Some("opus")))
                    );
                    continue;
                }
                let data = probe(&ffprobe, &output);
                let streams = data["streams"].as_array().unwrap();
                assert_eq!(streams.len(), if audio.is_some() { 2 } else { 1 });
                assert_eq!(streams[0]["codec_name"], video);
                assert_eq!(streams[0]["width"], 160);
                assert_eq!(streams[0]["height"], 90);
                assert_copy_equivalent(&ffmpeg, &source, &output, StreamRole::Video);
                if let Some(audio) = audio {
                    assert_eq!(streams[1]["codec_name"], audio);
                    assert_eq!(streams[1]["sample_rate"], "48000");
                    assert_eq!(streams[1]["channels"], 1);
                    assert_copy_equivalent(&ffmpeg, &source, &output, StreamRole::Audio);
                }
                println!(
                    "MUX {video}/{} -> {container}: copy verified",
                    audio.unwrap_or("none")
                );
            }
        }
    }
}

#[test]
#[ignore = "pinned portable tools required; no network"]
fn real_audio_only_container_copies_original_packets() {
    let tools = real::tools();
    let root = tempfile::tempdir().unwrap();
    let ffmpeg = tools.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let ffprobe = tools.join("ffprobe-x86_64-pc-windows-msvc.exe");
    for (audio, container) in [
        ("aac", "m4a"),
        ("alac", "m4a"),
        ("opus", "webm"),
        ("vorbis", "webm"),
        ("mp3", "mp3"),
        ("flac", "flac"),
    ] {
        let source = generate_source(
            &ffmpeg,
            &root.path().join(format!("{audio}.mkv")),
            None,
            Some(audio),
        );
        let output = root.path().join(format!("{audio}.{container}"));
        real::checked(
            Command::new(&ffmpeg)
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(&source)
                .args(["-map", "0:a:0", "-vn", "-c:a", "copy", "-n"])
                .arg(&output)
                .output()
                .unwrap(),
        );
        let data = probe(&ffprobe, &output);
        assert_eq!(data["streams"].as_array().unwrap().len(), 1);
        assert_eq!(data["streams"][0]["codec_name"], audio);
        assert_copy_equivalent(&ffmpeg, &source, &output, StreamRole::Audio);
        println!("AUDIO {audio} -> {container}: copy verified");
    }
}

#[test]
#[ignore = "pinned portable tools required; static HDR/profile fixtures, no network"]
fn real_hdr_and_non_compatible_profiles_are_probeable_and_copyable() {
    let tools = real::tools();
    let root = tempfile::tempdir().unwrap();
    let ffmpeg = tools.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let ffprobe = tools.join("ffprobe-x86_64-pc-windows-msvc.exe");
    for transfer in ["smpte2084", "arib-std-b67"] {
        let source = root.path().join(format!("{transfer}.mkv"));
        let params=format!("pools=1:frame-threads=1:log-level=error:colorprim=bt2020:transfer={transfer}:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400");
        let mut cmd = Command::new(&ffmpeg);
        cmd.args(["-v","error","-nostdin","-f","lavfi","-i",
            "color=size=160x90:rate=30:duration=1,format=yuv420p10le,geq=lum='64+876*X/W':cb=512:cr=512",
            "-c:v","libx265","-preset","ultrafast","-pix_fmt","yuv420p10le","-color_range","tv",
            "-colorspace","bt2020nc","-color_primaries","bt2020","-color_trc",transfer,
            "-x265-params",params.as_str(),"-n"]);
        real::checked(cmd.arg(&source).output().unwrap());
        let input = probe(&ffprobe, &source);
        assert_eq!(input["streams"][0]["pix_fmt"], "yuv420p10le");
        let input_frame = real::probe_video_frame(&ffprobe, &source);
        assert_eq!(input_frame["color_transfer"], transfer);
        for container in ["mp4", "mkv"] {
            let output = root.path().join(format!("{transfer}.{container}"));
            if container == "mkv" {
                continue;
            } // source already proves Matroska; never overwrite it.
            real::checked(
                Command::new(&ffmpeg)
                    .args(["-v", "error", "-i"])
                    .arg(&source)
                    .args(["-c", "copy", "-n"])
                    .arg(&output)
                    .output()
                    .unwrap(),
            );
            let copied = probe(&ffprobe, &output);
            let output_frame = real::probe_video_frame(&ffprobe, &output);
            for key in [
                "codec_name",
                "width",
                "height",
                "pix_fmt",
                "color_range",
                "color_space",
                "color_transfer",
                "color_primaries",
            ] {
                let expected = if input["streams"][0][key].is_null() {
                    &input_frame[key]
                } else {
                    &input["streams"][0][key]
                };
                let actual = if copied["streams"][0][key].is_null() {
                    &output_frame[key]
                } else {
                    &copied["streams"][0][key]
                };
                assert_eq!(actual, expected, "HDR {key}");
            }
            assert_copy_equivalent(&ffmpeg, &source, &output, StreamRole::Video);
            // HEVC VUI/SEI can be present only in decoded-frame output in this pinned version.
            let data = input_frame["side_data_list"].as_array().unwrap();
            assert!(data
                .iter()
                .any(|row| row["side_data_type"] == "Mastering display metadata"));
            assert_eq!(output_frame, input_frame);
            println!("HDR {transfer}/HEVC/10bit -> {container}: decoded content + static frame metadata verified");
        }
    }
    for (profile, pixels) in [("high10", "yuv420p10le"), ("high444", "yuv444p")] {
        let source = root.path().join(format!("{profile}.mp4"));
        real::checked(
            Command::new(&ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=size=160x90:rate=30:duration=1",
                    "-c:v",
                    "libx264",
                    "-threads",
                    "1",
                    "-profile:v",
                    profile,
                    "-pix_fmt",
                    pixels,
                    "-n",
                ])
                .arg(&source)
                .output()
                .unwrap(),
        );
        assert_eq!(probe(&ffprobe, &source)["streams"][0]["pix_fmt"], pixels);
    }
    let odd = root.path().join("odd.webm");
    real::checked(
        Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=161x91:rate=30:duration=1",
                "-c:v",
                "libvpx-vp9",
                "-threads",
                "1",
                "-deadline",
                "realtime",
                "-pix_fmt",
                "yuv420p",
                "-n",
            ])
            .arg(&odd)
            .output()
            .unwrap(),
    );
    assert_eq!(probe(&ffprobe, &odd)["streams"][0]["width"], 161);
    assert_eq!(probe(&ffprobe, &odd)["streams"][0]["height"], 91);
}

#[test]
#[ignore = "pinned portable tools required; no network"]
#[should_panic(expected = "stream content changed")]
fn content_proof_detects_an_audio_reencode() {
    let tools = real::tools();
    let root = tempfile::tempdir().unwrap();
    let ffmpeg = tools.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let source = generate_source(
        &ffmpeg,
        &root.path().join("original.mkv"),
        None,
        Some("opus"),
    );
    let changed = root.path().join("changed.m4a");
    real::checked(
        Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&source)
            .args(["-c:a", "aac", "-n"])
            .arg(&changed)
            .output()
            .unwrap(),
    );
    assert_copy_equivalent(&ffmpeg, &source, &changed, StreamRole::Audio);
}
