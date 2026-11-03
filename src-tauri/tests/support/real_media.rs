#![allow(dead_code)]
use local_media_downloader_lib::domain::managed_tools::ManagedToolManifest;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[derive(Clone, Copy)]
pub enum StreamRole {
    Video,
    Audio,
}

pub fn tools() -> PathBuf {
    let root =
        PathBuf::from(std::env::var_os("LMD_REAL_TOOL_PACKAGE").expect("pinned tools required"));
    let manifest: ManagedToolManifest =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    use sha2::{Digest, Sha256};
    for tool in manifest.tools {
        let name = tool.name.as_str();
        use std::io::Read;
        let mut file =
            std::fs::File::open(root.join(format!("{name}-x86_64-pc-windows-msvc.exe"))).unwrap();
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 65_536];
        loop {
            let count = file.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        assert_eq!(
            format!("{:x}", digest.finalize()),
            tool.sha256,
            "tool digest {name}"
        );
    }
    root
}

pub fn checked(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

pub fn generate_source(
    ffmpeg: &Path,
    output: &Path,
    video: Option<&str>,
    audio: Option<&str>,
) -> PathBuf {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-v", "error", "-nostdin"]);
    if video.is_some() {
        cmd.args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=30:duration=3",
        ]);
    }
    if audio.is_some() {
        cmd.args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000:duration=3",
        ]);
    }
    if let Some(video) = video {
        cmd.args([
            "-map",
            "0:v:0",
            "-threads",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_primaries",
            "bt709",
        ]);
        match video {
            "h264" => {
                cmd.args(["-c:v", "libx264", "-preset", "ultrafast"]);
            }
            "hevc" => {
                cmd.args([
                    "-c:v",
                    "libx265",
                    "-preset",
                    "ultrafast",
                    "-x265-params",
                    "pools=1:frame-threads=1:log-level=error",
                ]);
            }
            "av1" => {
                cmd.args(["-c:v", "libaom-av1", "-cpu-used", "8"]);
            }
            "vp9" => {
                cmd.args([
                    "-c:v",
                    "libvpx-vp9",
                    "-deadline",
                    "realtime",
                    "-cpu-used",
                    "8",
                ]);
            }
            "vp8" => {
                cmd.args(["-c:v", "libvpx", "-deadline", "realtime", "-cpu-used", "8"]);
            }
            _ => panic!("unknown fixture codec"),
        }
    }
    if let Some(audio) = audio {
        cmd.args([
            "-map",
            if video.is_some() { "1:a:0" } else { "0:a:0" },
            "-c:a",
            match audio {
                "opus" => "libopus",
                "vorbis" => "libvorbis",
                "mp3" => "libmp3lame",
                "aac" => "aac",
                "flac" => "flac",
                "alac" => "alac",
                _ => panic!("unknown audio fixture"),
            },
        ]);
    }
    checked(cmd.arg("-n").arg(output).output().unwrap());
    output.to_owned()
}

pub fn probe(ffprobe: &Path, path: &Path) -> serde_json::Value {
    serde_json::from_slice(&checked(
        Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap(),
    ))
    .unwrap()
}

pub fn probe_video_frame(ffprobe: &Path, path: &Path) -> serde_json::Value {
    let frames: serde_json::Value = serde_json::from_slice(&checked(Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-read_intervals", "%+#32", "-show_frames",
            "-show_entries", "frame=pix_fmt,color_range,color_space,color_transfer,color_primaries,side_data_list", "-of", "json"])
        .arg(path).output().unwrap())).unwrap();
    frames["frames"][0].clone()
}

pub fn content_hash(ffmpeg: &Path, path: &Path, role: StreamRole) -> String {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-v", "error", "-i"]).arg(path).args([
        "-map",
        match role {
            StreamRole::Video => "0:v:0",
            StreamRole::Audio => "0:a:0",
        },
    ]);
    match role {
        StreamRole::Video => {
            cmd.args(["-an", "-fps_mode", "passthrough", "-f", "framemd5"]);
        }
        StreamRole::Audio => {
            cmd.args(["-vn", "-c:a", "copy", "-f", "streamhash", "-hash", "sha256"]);
        }
    }
    let data = checked(cmd.arg("-").output().unwrap());
    String::from_utf8(data)
        .unwrap()
        .lines()
        .filter(|s| !s.starts_with('#') && !s.trim().is_empty())
        .map(|s| s.rsplit(',').next().unwrap().trim())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn assert_copy_equivalent(ffmpeg: &Path, source: &Path, output: &Path, role: StreamRole) {
    let expected = content_hash(ffmpeg, source, role);
    assert!(!expected.is_empty());
    assert_eq!(
        content_hash(ffmpeg, output, role),
        expected,
        "stream content changed: {}",
        output.display()
    );
}
