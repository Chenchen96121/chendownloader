#[path = "support/output_policy.rs"]
mod policy_fixture;
#[path = "support/real_media.rs"]
mod real_media;
use local_media_downloader_lib::{
    application::{
        download_executor::StagedDownload,
        local_processing::{LocalMediaProcessor, LocalProcessingRequest},
        output_verifier::OutputVerifier,
        process::CancellationToken,
        tool_bootstrap::ToolBootstrapService,
    },
    domain::{
        format_selection::{ClipRange, SelectionMode, SelectionOperation, SelectionPlan},
        managed_tools::{ManagedToolManifest, BUILD_TARGET},
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::{FfprobeOutputVerifier, CLIP_DURATION_TOLERANCE_MS},
        process::WindowsProcessSupervisor,
        tools::{FilesystemManagedToolStore, NativeToolVersionProbe},
    },
};
use std::{env, fs, path::PathBuf, sync::Arc};

#[test]
#[ignore = "requires pinned portable tools; two combined sources and static HDR, no network"]
fn real_distinct_combined_roles_and_hdr_preserve_content() {
    use local_media_downloader_lib::domain::output_policy::{
        ContainerSelection, StreamProcessing, TargetAudioCodec,
    };
    let package = real_media::tools();
    let root = tempfile::tempdir().unwrap();
    let manifest: ManagedToolManifest =
        serde_json::from_slice(&fs::read(package.join("manifest.json")).unwrap()).unwrap();
    let store = Arc::new(FilesystemManagedToolStore::new(
        root.path().join("tools"),
        BUILD_TARGET,
    ));
    let process = Arc::new(WindowsProcessSupervisor);
    ToolBootstrapService::new(
        store.as_ref(),
        &NativeToolVersionProbe::new(process.as_ref()),
        BUILD_TARGET,
    )
    .bootstrap(&package, &manifest)
    .unwrap();
    let ffmpeg = package.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let processor = FfmpegMediaProcessor::new(Arc::clone(&store), Arc::clone(&process));
    let verifier = FfprobeOutputVerifier::new(Arc::clone(&store), Arc::clone(&process));
    for (job, transfer) in [
        ("901", None),
        ("902", Some("smpte2084")),
        ("903", Some("arib-std-b67")),
    ] {
        let staging = root.path().join(job);
        let source_dir = staging.join("source");
        fs::create_dir_all(source_dir.join("input-0")).unwrap();
        fs::create_dir(source_dir.join("input-1")).unwrap();
        let video_file = source_dir.join("input-0/source.mkv");
        let audio_file = source_dir.join("input-1/source.mkv");
        let codec = if transfer.is_some() { "hevc" } else { "h264" };
        if let Some(transfer) = transfer {
            let parameters = format!("pools=1:frame-threads=1:log-level=error:colorprim=bt2020:transfer={transfer}:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400");
            real_media::checked(std::process::Command::new(&ffmpeg).args(["-v","error","-f","lavfi","-i","color=size=160x90:rate=30:duration=1,format=yuv420p10le,geq=lum='64+876*X/W':cb=512:cr=512","-f","lavfi","-i","sine=frequency=440:duration=1:sample_rate=48000","-c:v","libx265","-preset","ultrafast","-pix_fmt","yuv420p10le","-color_range","tv","-colorspace","bt2020nc","-color_primaries","bt2020","-color_trc",transfer,"-x265-params",&parameters,"-c:a","aac","-n"]).arg(&video_file).output().unwrap());
        } else {
            real_media::generate_source(&ffmpeg, &video_file, Some(codec), Some("aac"));
        }
        // Another combined source has lower-resolution video and a different audio codec.
        real_media::generate_source(&ffmpeg, &audio_file, Some("vp9"), Some("flac"));
        let mut video = policy_fixture::video_format("video", codec);
        video.width = Some(160);
        video.height = Some(90);
        video.audio_codec = Some("aac".into());
        let mut audio = policy_fixture::audio_format("audio", "flac");
        audio.audio_channels = Some(1);
        audio.audio_sample_rate_hz = Some(48000);
        let plan = SelectionPlan {
            compatibility_mode: false,
            container_selection: ContainerSelection::Automatic,
            video_processing: Some(StreamProcessing::Copy),
            output_video_codec: Some(codec.into()),
            mode: SelectionMode::BestAvailable,
            source_format_ids: vec!["video".into(), "audio".into()],
            video_format_id: Some("video".into()),
            audio_format_id: Some("audio".into()),
            operation: SelectionOperation::Merge,
            output_container: Some("mp4".into()),
            output_audio_codec: Some(TargetAudioCodec::Flac),
            audio_processing: Some(StreamProcessing::Copy),
            clip: None,
            estimated_size_bytes: None,
            warnings: vec![],
        };
        let processed = processor
            .process(
                LocalProcessingRequest {
                    job_id: job.into(),
                    staging_dir: staging.clone(),
                    source_dir: source_dir.clone(),
                    plan: plan.clone(),
                    video_source: Some(video),
                    audio_source: Some(audio),
                    sources: policy_fixture::slots(&plan.source_format_ids),
                },
                CancellationToken::new(),
            )
            .unwrap();
        let output = staging.join("stage.mp4");
        real_media::assert_copy_equivalent(
            &ffmpeg,
            &video_file,
            &output,
            real_media::StreamRole::Video,
        );
        real_media::assert_copy_equivalent(
            &ffmpeg,
            &audio_file,
            &output,
            real_media::StreamRole::Audio,
        );
        if let Some(transfer) = transfer {
            let proof = processed.evidence.source_video.as_ref().unwrap();
            assert_eq!(proof.color_transfer.as_deref(), Some(transfer));
            assert_eq!(proof.color_primaries.as_deref(), Some("bt2020"));
            assert_eq!(proof.hdr_side_data.len(), 2);
        }
        fs::remove_file(video_file).unwrap();
        fs::remove_file(audio_file).unwrap();
        fs::remove_dir(source_dir.join("input-0")).unwrap();
        fs::remove_dir(source_dir.join("input-1")).unwrap();
        fs::remove_dir(source_dir).unwrap();
        let output = verifier
            .verify(
                StagedDownload {
                    job_id: job.into(),
                    staging_dir: staging,
                    expected_operation: plan.operation,
                    expected_container: Some("mp4".into()),
                    requested_clip: None,
                    evidence: processed.evidence,
                },
                &plan,
                CancellationToken::new(),
            )
            .unwrap();
        assert!(output.has_audio && output.has_video);
    }
}

#[test]
#[ignore = "requires pinned tools; independent HDR pixel reference and AAC layout characterization"]
fn real_fixed_compatibility_candidate_has_verified_colors_and_audio_layouts() {
    use std::process::Command;
    let package = real_media::tools();
    let root = tempfile::tempdir().unwrap();
    let ffmpeg = package.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let ffprobe = package.join("ffprobe-x86_64-pc-windows-msvc.exe");
    for rate in [44100, 48000] {
        for (layout, channels, bitrate) in [
            ("mono", 1, "96k"),
            ("stereo", 2, "192k"),
            ("5.1", 6, "512k"),
        ] {
            let output = root.path().join(format!("{rate}-{channels}.m4a"));
            real_media::checked(
                Command::new(&ffmpeg)
                    .args([
                        "-v",
                        "error",
                        "-f",
                        "lavfi",
                        "-i",
                        &format!("anullsrc=r={rate}:cl={layout}:d=1"),
                        "-c:a",
                        "aac",
                        "-profile:a",
                        "aac_low",
                        "-b:a",
                        bitrate,
                        "-n",
                    ])
                    .arg(&output)
                    .output()
                    .unwrap(),
            );
            let actual = real_media::probe(&ffprobe, &output);
            assert_eq!(actual["streams"][0]["channels"], channels);
            assert_eq!(actual["streams"][0]["channel_layout"], layout);
            assert_eq!(actual["streams"][0]["sample_rate"], rate.to_string());
            assert_eq!(actual["streams"][0]["profile"], "LC");
        }
    }
    let candidate="zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p,sidedata=mode=delete:type=MASTERING_DISPLAY_METADATA,sidedata=mode=delete:type=CONTENT_LIGHT_LEVEL";
    let proof_dir = env::var_os("LMD_TONE_PROOF_DIRECTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.path().join("pictures"));
    fs::create_dir_all(&proof_dir).unwrap();
    for transfer in ["smpte2084", "arib-std-b67"] {
        let source = root.path().join(format!("{transfer}.mkv"));
        let output = root.path().join(format!("{transfer}.mp4"));
        let params=format!("pools=1:frame-threads=1:log-level=error:colorprim=bt2020:transfer={transfer}:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400");
        real_media::checked(Command::new(&ffmpeg).args(["-v","error","-f","lavfi","-i","color=size=160x90:rate=30:duration=1,format=yuv420p10le,geq=lum='64+600*X/W':cb='512+50*sin(2*PI*Y/H)':cr='512+50*cos(2*PI*Y/H)'","-c:v","libx265","-preset","ultrafast","-pix_fmt","yuv420p10le","-color_range","tv","-colorspace","bt2020nc","-color_primaries","bt2020","-color_trc",transfer,"-x265-params",&params,"-n"]).arg(&source).output().unwrap());
        real_media::checked(
            Command::new(&ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(&source)
                .args([
                    "-vf",
                    candidate,
                    "-c:v",
                    "libx264",
                    "-crf",
                    "18",
                    "-preset",
                    "medium",
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
                    "-n",
                ])
                .arg(&output)
                .output()
                .unwrap(),
        );
        let actual = real_media::probe(&ffprobe, &output);
        assert_eq!(actual["streams"][0]["codec_name"], "h264");
        assert_eq!(actual["streams"][0]["pix_fmt"], "yuv420p");
        assert_eq!(actual["streams"][0]["color_transfer"], "bt709");
        let frame = real_media::probe_video_frame(&ffprobe, &output);
        assert!(!frame["side_data_list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| matches!(
                s["side_data_type"].as_str(),
                Some("Mastering display metadata" | "Content light level metadata")
            )));
        let raw = |path: &std::path::Path, pixels: &str| {
            real_media::checked(
                Command::new(&ffmpeg)
                    .args(["-v", "error", "-i"])
                    .arg(path)
                    .args([
                        "-map",
                        "0:v:0",
                        "-frames:v",
                        "1",
                        "-pix_fmt",
                        pixels,
                        "-f",
                        "rawvideo",
                        "-",
                    ])
                    .output()
                    .unwrap(),
            )
        };
        let original = raw(&source, "yuv420p10le");
        let result = raw(&output, "yuv420p");
        assert_eq!(original.len(), 43200);
        assert_eq!(result.len(), 21600);
        let hable =
            |x: f64| ((x * (0.15 * x + 0.05) + 0.004) / (x * (0.15 * x + 0.5) + 0.06)) - 0.02 / 0.3;
        let eotf = |x: f64| {
            let x = x.max(0.0);
            if transfer == "smpte2084" {
                let p = x.powf(1.0 / 78.84375);
                ((p - 0.8359375).max(0.0) / (18.8515625 - 18.6875 * p)).powf(1.0 / 0.1593017578125)
                    * 100.0
            } else {
                let linear = if x <= 0.5 {
                    x * x / 3.0
                } else {
                    (((x - 0.55991073) / 0.17883277).exp() + 0.28466892) / 12.0
                };
                linear.powf(1.2) * 10.0
            }
        };
        // zimg's display-referred BT.709 uses inverse BT.1886, not the camera OETF.
        // Independent scalar reference; the production filter is not used to derive it.
        let oetf = |x: f64| x.max(0.0).powf(1.0 / 2.4);
        let word = |index: usize| {
            u16::from_le_bytes([original[index * 2], original[index * 2 + 1]]) as f64
        };
        let chroma = |offset: usize, x: usize, y: usize| {
            // HEVC's left/vertical-center 4:2:0 samples, bilinear upsampling.
            let cx = x as f64 / 2.0;
            let cy = ((y as f64 - 0.5) / 2.0).clamp(0.0, 44.0);
            let ix = cx.floor() as usize;
            let iy = cy.floor() as usize;
            let fx = cx - ix as f64;
            let fy = cy - iy as f64;
            let sample = |xx: usize, yy: usize| word(offset + yy.min(44) * 80 + xx.min(79));
            (sample(ix, iy) * (1.0 - fx) + sample(ix + 1, iy) * fx) * (1.0 - fy)
                + (sample(ix, iy + 1) * (1.0 - fx) + sample(ix + 1, iy + 1) * fx) * fy
        };
        let reference = |x: usize, y: usize| {
            let yy = (word(y * 160 + x) - 64.0) / 876.0;
            let cb = (chroma(14400, x, y) - 512.0) / 896.0;
            let cr = (chroma(18000, x, y) - 512.0) / 896.0;
            let r = eotf(yy + 1.4746 * cr);
            let b = eotf(yy + 1.8814 * cb);
            let g = eotf((yy - 0.2627 * (yy + 1.4746 * cr) - 0.0593 * (yy + 1.8814 * cb)) / 0.6780);
            // Independent BT.2020→BT.709 linear RGB matrix; not another call to the filter.
            let rgb = [
                1.660491 * r - 0.587641 * g - 0.072850 * b,
                -0.124550 * r + 1.132900 * g - 0.008350 * b,
                -0.018151 * r - 0.100579 * g + 1.118730 * b,
            ];
            let peak = rgb.iter().copied().fold(1e-6, f64::max);
            let scale = hable(peak) / hable(10.0) / peak;
            let rgb = rgb.map(|v| oetf(v * scale));
            let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            (
                rgb,
                [
                    16.0 + 219.0 * luma,
                    128.0 + 224.0 * (rgb[2] - luma) / 1.8556,
                    128.0 + 224.0 * (rgb[0] - luma) / 1.5748,
                ],
            )
        };
        let mut maximum = 0.0_f64;
        let mut sum = 0.0;
        let mut count = 0;
        let mut worst = (0, 0, 0, 0.0, 0);
        for y in (20..70).step_by(4) {
            for x in (20..140).step_by(4) {
                let (_, mut expected) = reference(x, y);
                // Independent antialiasing kernel for bilinear 2× 4:2:0 downsampling.
                for plane in [1, 2] {
                    expected[plane] = 0.0;
                    for (dx, wx) in [(-1, 0.25), (0, 0.5), (1, 0.25)] {
                        for (dy, wy) in [(-1, 0.125), (0, 0.375), (1, 0.375), (2, 0.125)] {
                            expected[plane] +=
                                reference((x as isize + dx) as usize, (y as isize + dy) as usize).1
                                    [plane]
                                    * wx
                                    * wy;
                        }
                    }
                }
                let measured = [
                    result[y * 160 + x],
                    result[14400 + (y / 2) * 80 + x / 2],
                    result[18000 + (y / 2) * 80 + x / 2],
                ];
                for (plane, (actual, expected)) in measured.into_iter().zip(expected).enumerate() {
                    let difference = (f64::from(actual) - expected.clamp(0.0, 255.0)).abs();
                    if difference > maximum {
                        maximum = difference;
                        worst = (x, y, plane, expected, actual);
                    }
                    sum += difference;
                    count += 1;
                }
            }
        }
        println!(
            "{transfer} independent pixel reference: max={maximum:.3}, mean={:.3}",
            sum / f64::from(count)
        );
        println!("worst RGB/YUV coordinate: {worst:?}");
        assert!(
            maximum <= 5.0 && sum / f64::from(count) <= 1.5,
            "tone map differs from independent RGB reference"
        );
        let mut ppm = b"P6\n160 90\n255\n".to_vec();
        for y in 0..90 {
            for x in 0..160 {
                ppm.extend(
                    reference(x, y)
                        .0
                        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8),
                );
            }
        }
        let reference_file = proof_dir.join(format!("{transfer}-reference.ppm"));
        fs::write(&reference_file, ppm).unwrap();
        for (input, name) in [(&output, "actual"), (&reference_file, "reference")] {
            real_media::checked(
                Command::new(&ffmpeg)
                    .args(["-v", "error", "-i"])
                    .arg(input)
                    .args(["-frames:v", "1", "-n"])
                    .arg(proof_dir.join(format!("{transfer}-{name}.png")))
                    .output()
                    .unwrap(),
            );
        }
    }
}

#[test]
#[ignore = "requires pinned tools; real approved compatibility pipeline and cancellation, no network"]
fn real_approved_compatibility_preserves_dimensions_rates_and_track_roles() {
    use local_media_downloader_lib::domain::output_policy::{
        ContainerSelection, StreamProcessing, TargetAudioCodec,
    };
    use std::process::Command;
    let package = real_media::tools();
    let root = tempfile::tempdir().unwrap();
    let manifest: ManagedToolManifest =
        serde_json::from_slice(&fs::read(package.join("manifest.json")).unwrap()).unwrap();
    let store = Arc::new(FilesystemManagedToolStore::new(
        root.path().join("tools"),
        BUILD_TARGET,
    ));
    let process = Arc::new(WindowsProcessSupervisor);
    ToolBootstrapService::new(
        store.as_ref(),
        &NativeToolVersionProbe::new(process.as_ref()),
        BUILD_TARGET,
    )
    .bootstrap(&package, &manifest)
    .unwrap();
    let ffmpeg = package.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let ffprobe = package.join("ffprobe-x86_64-pc-windows-msvc.exe");
    let processor = FfmpegMediaProcessor::new(Arc::clone(&store), Arc::clone(&process));
    let verifier = FfprobeOutputVerifier::new(Arc::clone(&store), Arc::clone(&process));
    // Literal expectations: safe H.264 is copied; other approved video is converted.
    for (index, (fixture, video, audio, copy_video, copy_audio, clip)) in [
        ("av1", true, true, false, false, false),
        ("h264", true, true, true, true, false),
        ("high10", true, true, false, true, false),
        ("high444", true, false, false, false, false),
        ("av1", true, false, false, false, true),
        ("av1", false, true, false, false, true),
        ("smpte2084", true, true, false, true, false),
        ("arib-std-b67", true, false, false, false, false),
    ]
    .into_iter()
    .enumerate()
    {
        let id = (800 + index).to_string();
        let staging = root.path().join(&id);
        let source_dir = staging.join("source");
        let slot = source_dir.join("input-0");
        fs::create_dir_all(&slot).unwrap();
        let source = slot.join("source.mkv");
        let codec = match fixture {
            "high10" | "high444" => "h264",
            "smpte2084" | "arib-std-b67" => "hevc",
            other => other,
        };
        let audio_codec = if fixture == "av1" { "opus" } else { "aac" };
        if matches!(
            fixture,
            "h264" | "high10" | "high444" | "smpte2084" | "arib-std-b67"
        ) {
            let hdr = codec == "hevc";
            let pixels = if fixture == "h264" {
                "yuv420p"
            } else if fixture == "high444" {
                "yuv444p"
            } else {
                "yuv420p10le"
            };
            let mut cmd = Command::new(&ffmpeg);
            cmd.args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size=160x90:rate=30:duration=3,format={pixels}"),
            ]);
            if audio {
                cmd.args([
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=3:sample_rate=48000",
                ]);
            }
            cmd.args([
                "-c:v",
                if hdr { "libx265" } else { "libx264" },
                "-preset",
                "ultrafast",
                "-pix_fmt",
                pixels,
                "-color_range",
                "tv",
                "-colorspace",
                if hdr { "bt2020nc" } else { "bt709" },
                "-color_primaries",
                if hdr { "bt2020" } else { "bt709" },
                "-color_trc",
                if hdr { fixture } else { "bt709" },
            ]);
            if hdr {
                cmd.args(["-x265-params",&format!("pools=1:frame-threads=1:log-level=error:colorprim=bt2020:transfer={fixture}:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400")]);
            } else {
                cmd.args([
                    "-x264-params",
                    "colorprim=bt709:transfer=bt709:colormatrix=bt709",
                ]);
            }
            if audio {
                cmd.args(["-c:a", "aac"]);
            }
            real_media::checked(cmd.arg("-n").arg(&source).output().unwrap());
        } else {
            real_media::generate_source(&ffmpeg, &source, Some(codec), Some(audio_codec));
            if codec == "av1" {
                // libaom needs explicit bitstream color controls; generic flags alone
                // leave this fixture's decoded transfer/primaries unspecified.
                let replacement = slot.join("colored.mkv");
                real_media::checked(
                    Command::new(&ffmpeg)
                        .args([
                            "-v",
                            "error",
                            "-f",
                            "lavfi",
                            "-i",
                            "testsrc2=size=160x90:rate=30:duration=3",
                            "-f",
                            "lavfi",
                            "-i",
                            "sine=frequency=1000:sample_rate=48000:duration=3",
                            "-map",
                            "0:v:0",
                            "-c:v",
                            "libaom-av1",
                            "-threads",
                            "1",
                            "-cpu-used",
                            "8",
                            "-aom-params",
                            "color-primaries=1:transfer-characteristics=1:matrix-coefficients=1",
                            "-color_range",
                            "tv",
                            "-colorspace",
                            "bt709",
                            "-color_trc",
                            "bt709",
                            "-color_primaries",
                            "bt709",
                            "-map",
                            "1:a:0",
                            "-c:a",
                            "libopus",
                            "-n",
                        ])
                        .arg(&replacement)
                        .output()
                        .unwrap(),
                );
                fs::remove_file(&source).unwrap();
                fs::rename(replacement, &source).unwrap();
            }
        }
        let mut metadata = policy_fixture::video_format("local", codec);
        metadata.width = Some(160);
        metadata.height = Some(90);
        metadata.audio_codec = Some(audio_codec.into());
        metadata.audio_channels = Some(1);
        metadata.audio_sample_rate_hz = Some(48000);
        let clip = clip.then_some(ClipRange {
            start_ms: 1000,
            end_ms: 2000,
        });
        let plan = SelectionPlan {
            compatibility_mode: true,
            container_selection: ContainerSelection::Compatibility,
            video_processing: video.then_some(StreamProcessing::Transcode),
            output_video_codec: video.then(|| "h264".into()),
            mode: if !video {
                SelectionMode::AudioOnly
            } else if !audio {
                SelectionMode::VideoOnly
            } else {
                SelectionMode::BestAvailable
            },
            source_format_ids: vec!["local".into()],
            video_format_id: video.then(|| "local".into()),
            audio_format_id: audio.then(|| "local".into()),
            operation: if !video {
                SelectionOperation::ExtractAudio
            } else if !audio {
                SelectionOperation::ExtractVideo
            } else {
                SelectionOperation::Keep
            },
            output_container: Some(if video { "mp4" } else { "m4a" }.into()),
            output_audio_codec: audio.then_some(TargetAudioCodec::Aac),
            audio_processing: audio.then_some(if audio_codec == "aac" {
                StreamProcessing::Copy
            } else {
                StreamProcessing::Transcode
            }),
            clip,
            estimated_size_bytes: None,
            warnings: vec![],
        };
        let result = processor
            .process(
                LocalProcessingRequest {
                    job_id: id.clone(),
                    staging_dir: staging.clone(),
                    source_dir: source_dir.clone(),
                    plan: plan.clone(),
                    video_source: video.then(|| metadata.clone()),
                    audio_source: audio.then_some(metadata),
                    sources: policy_fixture::slots(&plan.source_format_ids),
                },
                CancellationToken::new(),
            )
            .unwrap_or_else(|e| panic!("{fixture}/{video}/{audio}: {e:?}"));
        assert_eq!(
            result.evidence.video_processing,
            video.then_some(if copy_video {
                StreamProcessing::Copy
            } else {
                StreamProcessing::Transcode
            })
        );
        assert_eq!(
            result.evidence.audio_processing,
            audio.then_some(if copy_audio {
                StreamProcessing::Copy
            } else {
                StreamProcessing::Transcode
            })
        );
        let output = staging.join(if video { "stage.mp4" } else { "stage.m4a" });
        let actual = real_media::probe(&ffprobe, &output);
        let streams = actual["streams"].as_array().unwrap();
        assert_eq!(streams.len(), usize::from(video) + usize::from(audio));
        if video {
            let v = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
            assert_eq!(v["codec_name"], "h264");
            assert_eq!(v["width"], 160);
            assert_eq!(v["height"], 90);
            assert_eq!(v["avg_frame_rate"], "30/1");
            assert_eq!(v["pix_fmt"], "yuv420p");
            assert_eq!(v["color_transfer"], "bt709");
            if copy_video && clip.is_none() {
                real_media::assert_copy_equivalent(
                    &ffmpeg,
                    &source,
                    &output,
                    real_media::StreamRole::Video,
                );
            }
        }
        if audio {
            let a = streams.iter().find(|s| s["codec_type"] == "audio").unwrap();
            assert_eq!(a["codec_name"], "aac");
            assert_eq!(a["sample_rate"], "48000");
            assert_eq!(a["channels"], 1);
            assert_eq!(a["channel_layout"], "mono");
            if copy_audio && clip.is_none() {
                real_media::assert_copy_equivalent(
                    &ffmpeg,
                    &source,
                    &output,
                    real_media::StreamRole::Audio,
                );
            }
        }
        fs::remove_file(&source).unwrap();
        fs::remove_dir(&slot).unwrap();
        fs::remove_dir(&source_dir).unwrap();
        let verified = verifier
            .verify(
                StagedDownload {
                    job_id: id,
                    staging_dir: staging,
                    expected_operation: plan.operation,
                    expected_container: plan.output_container.clone(),
                    requested_clip: clip,
                    evidence: result.evidence,
                },
                &plan,
                CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(verified.has_video, video);
        assert_eq!(verified.has_audio, audio);
        println!(
            "approved real compatibility: {fixture}, video={video}, audio={audio}, clip={clip:?}"
        );
    }
    // Cancel an actual encoder after its private output appears, not before launch.
    use local_media_downloader_lib::application::process::{
        ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest, ProcessSupervisor,
    };
    struct CancelEncoder;
    impl ProcessSupervisor for CancelEncoder {
        fn run(
            &self,
            request: ProcessRequest,
            token: CancellationToken,
            sink: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            if request.args.iter().any(|a| a == "libx264") {
                let path = PathBuf::from(request.args.last().unwrap());
                let canceled = token.clone();
                std::thread::scope(|scope| {
                    let monitor = scope.spawn(move || {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(10);
                        while !path.exists() && std::time::Instant::now() < deadline {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        assert!(
                            path.exists(),
                            "encoder never started producing private output"
                        );
                        canceled.cancel();
                    });
                    let result = WindowsProcessSupervisor.run(request, token, sink);
                    monitor.join().unwrap();
                    result
                })
            } else {
                WindowsProcessSupervisor.run(request, token, sink)
            }
        }
    }
    let staging = root.path().join("899");
    let source_dir = staging.join("source");
    fs::create_dir_all(source_dir.join("input-0")).unwrap();
    let source = source_dir.join("input-0/source.mkv");
    real_media::checked(
        Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1280x720:rate=30:duration=5",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p10le",
                "-x264-params",
                "colorprim=bt709:transfer=bt709:colormatrix=bt709",
                "-threads",
                "1",
                "-color_range",
                "tv",
                "-colorspace",
                "bt709",
                "-color_trc",
                "bt709",
                "-color_primaries",
                "bt709",
                "-n",
            ])
            .arg(&source)
            .output()
            .unwrap(),
    );
    let plan = SelectionPlan {
        compatibility_mode: true,
        container_selection: ContainerSelection::Compatibility,
        video_processing: Some(StreamProcessing::Transcode),
        output_video_codec: Some("h264".into()),
        mode: SelectionMode::VideoOnly,
        source_format_ids: vec!["cancel".into()],
        video_format_id: Some("cancel".into()),
        audio_format_id: None,
        operation: SelectionOperation::ExtractVideo,
        output_container: Some("mp4".into()),
        output_audio_codec: None,
        audio_processing: None,
        clip: None,
        estimated_size_bytes: None,
        warnings: vec![],
    };
    let token = CancellationToken::new();
    let result = FfmpegMediaProcessor::new(store, CancelEncoder).process(
        LocalProcessingRequest {
            job_id: "899".into(),
            staging_dir: staging.clone(),
            source_dir,
            video_source: Some(policy_fixture::video_format("cancel", "h264")),
            audio_source: None,
            sources: policy_fixture::slots(&plan.source_format_ids),
            plan,
        },
        token.clone(),
    );
    assert_eq!(
        result.err(),
        Some(local_media_downloader_lib::domain::download::DownloadErrorCode::ProcessFailed)
    );
    assert!(token.is_canceled());
    assert!(
        root.path()
            .read_dir()
            .unwrap()
            .all(|e| e.unwrap().path().extension().is_none()),
        "nothing published outside private staging"
    );
}

// Wrong role mapping, re-encoding, or a container-driven downgrade must fail this test.
#[test]
#[ignore = "requires pinned portable tools; generates tiny local fixtures, no network"]
fn real_output_matrix_preserves_original_video_and_audio() {
    use local_media_downloader_lib::domain::output_policy::{
        ContainerSelection, OutputContainer, StreamProcessing, TargetAudioCodec,
    };
    let package = real_media::tools();
    let manifest: ManagedToolManifest =
        serde_json::from_slice(&fs::read(package.join("manifest.json")).unwrap()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(FilesystemManagedToolStore::new(
        root.path().join("managed-tools"),
        BUILD_TARGET,
    ));
    let process = Arc::new(WindowsProcessSupervisor);
    ToolBootstrapService::new(
        store.as_ref(),
        &NativeToolVersionProbe::new(process.as_ref()),
        BUILD_TARGET,
    )
    .bootstrap(&package, &manifest)
    .unwrap();
    let ffmpeg = package.join("ffmpeg-x86_64-pc-windows-msvc.exe");
    let processor = FfmpegMediaProcessor::new(Arc::clone(&store), Arc::clone(&process));
    let verifier = FfprobeOutputVerifier::new(Arc::clone(&store), Arc::clone(&process));
    let audios = [
        ("aac", TargetAudioCodec::Aac, "m4a"),
        ("opus", TargetAudioCodec::Opus, "webm"),
        ("vorbis", TargetAudioCodec::Vorbis, "webm"),
        ("mp3", TargetAudioCodec::Mp3, "mp3"),
        ("flac", TargetAudioCodec::Flac, "flac"),
        ("alac", TargetAudioCodec::Alac, "m4a"),
    ];
    let mut cases = Vec::new();
    for video in ["h264", "hevc", "av1", "vp9", "vp8"] {
        for (audio, codec, _) in audios {
            let container = if video == "vp8" {
                if audio == "opus" || audio == "vorbis" {
                    "webm"
                } else {
                    "mkv"
                }
            } else {
                "mp4"
            };
            cases.push((Some(video), Some((audio, codec)), container, false, false));
        }
        cases.push((
            Some(video),
            None,
            if video == "vp8" { "webm" } else { "mp4" },
            false,
            false,
        ));
    }
    for (audio, codec, container) in audios {
        cases.push((None, Some((audio, codec)), container, false, false));
    }
    for video in ["vp9", "av1"] {
        for audio in [
            ("opus", TargetAudioCodec::Opus),
            ("vorbis", TargetAudioCodec::Vorbis),
        ] {
            cases.push((Some(video), Some(audio), "webm", true, false));
        }
    }
    cases.push((
        Some("h264"),
        Some(("opus", TargetAudioCodec::Opus)),
        "mp4",
        false,
        true,
    ));
    cases.push((Some("vp9"), None, "webm", true, true));
    for (index, (video, audio, container, explicit, clipped)) in cases.into_iter().enumerate() {
        let id = (index + 10).to_string();
        let staging = root.path().join(&id);
        let source_dir = staging.join("source");
        let slot = source_dir.join("input-0");
        fs::create_dir_all(&slot).unwrap();
        let source = slot.join("source.mkv");
        real_media::generate_source(&ffmpeg, &source, video, audio.map(|a| a.0));
        let mut metadata = policy_fixture::video_format("local", video.unwrap_or("h264"));
        metadata.width = Some(160);
        metadata.height = Some(90);
        metadata.fps = Some(30.0);
        metadata.audio_codec = audio.map(|a| a.0.into());
        metadata.audio_channels = audio.map(|_| 1);
        metadata.audio_sample_rate_hz = audio.map(|_| 48000);
        let clip = clipped.then_some(ClipRange {
            start_ms: 1000,
            end_ms: 2000,
        });
        let plan = SelectionPlan {
            compatibility_mode: false,
            container_selection: if explicit {
                ContainerSelection::Explicit
            } else {
                ContainerSelection::Automatic
            },
            video_processing: video.map(|_| StreamProcessing::Copy),
            output_video_codec: video.map(str::to_owned),
            mode: if video.is_none() {
                SelectionMode::AudioOnly
            } else if audio.is_none() {
                SelectionMode::VideoOnly
            } else {
                SelectionMode::BestAvailable
            },
            source_format_ids: vec!["local".into()],
            video_format_id: video.map(|_| "local".into()),
            audio_format_id: audio.map(|_| "local".into()),
            operation: SelectionOperation::Keep,
            output_container: Some(container.into()),
            output_audio_codec: audio.map(|a| a.1),
            audio_processing: audio.map(|_| StreamProcessing::Copy),
            clip,
            estimated_size_bytes: None,
            warnings: vec![],
        };
        let processed = processor
            .process(
                LocalProcessingRequest {
                    job_id: id.clone(),
                    staging_dir: staging.clone(),
                    source_dir: source_dir.clone(),
                    plan: plan.clone(),
                    video_source: video.map(|_| metadata.clone()),
                    audio_source: audio.map(|_| metadata.clone()),
                    sources: policy_fixture::slots(&plan.source_format_ids),
                },
                CancellationToken::new(),
            )
            .unwrap_or_else(|error| panic!("processor {video:?}/{audio:?}/{container}: {error:?}"));
        assert_eq!(
            processed.evidence.actual_container,
            OutputContainer::parse(container).unwrap()
        );
        let output = staging.join(format!("stage.{container}"));
        if !clipped {
            if video.is_some() {
                real_media::assert_copy_equivalent(
                    &ffmpeg,
                    &source,
                    &output,
                    real_media::StreamRole::Video,
                );
            }
            if audio.is_some() {
                real_media::assert_copy_equivalent(
                    &ffmpeg,
                    &source,
                    &output,
                    real_media::StreamRole::Audio,
                );
            }
        } else {
            for role in [real_media::StreamRole::Video, real_media::StreamRole::Audio] {
                if matches!(role, real_media::StreamRole::Audio) && audio.is_none() {
                    continue;
                }
                let hashes = |path: &std::path::Path| {
                    let mut cmd = std::process::Command::new(&ffmpeg);
                    cmd.args(["-v", "error", "-i"]).arg(path).args([
                        "-map",
                        if matches!(role, real_media::StreamRole::Video) {
                            "0:v:0"
                        } else {
                            "0:a:0"
                        },
                    ]);
                    if matches!(role, real_media::StreamRole::Video) {
                        cmd.args(["-an", "-fps_mode", "passthrough"]);
                    } else {
                        cmd.args(["-vn", "-c:a", "copy"]);
                    }
                    let result = cmd.args(["-f", "framemd5", "-"]).output().unwrap();
                    assert!(result.status.success());
                    String::from_utf8(result.stdout)
                        .unwrap()
                        .lines()
                        .filter(|l| !l.starts_with('#'))
                        // Column 5 is the payload hash. Later columns describe side data,
                        // including legitimate clip-boundary discard padding.
                        .map(|l| l.split(',').nth(5).unwrap().trim().to_owned())
                        .collect::<Vec<_>>()
                };
                let original = hashes(&source);
                let actual = hashes(&output);
                assert!(!actual.is_empty() && !original.is_empty());
                assert!(
                    original.windows(actual.len()).any(|s| s == actual),
                    "clip content changed {video:?}/{audio:?}/{container}"
                );
            }
        }
        fs::remove_file(&source).unwrap();
        fs::remove_dir(&slot).unwrap();
        fs::remove_dir(&source_dir).unwrap();
        let verified = verifier
            .verify(
                StagedDownload {
                    job_id: id,
                    staging_dir: staging,
                    expected_operation: plan.operation,
                    expected_container: Some(container.into()),
                    requested_clip: clip,
                    evidence: processed.evidence,
                },
                &plan,
                CancellationToken::new(),
            )
            .unwrap_or_else(|error| panic!("verifier {video:?}/{audio:?}/{container}: {error:?}"));
        assert_eq!(verified.has_video, video.is_some());
        assert_eq!(verified.has_audio, audio.is_some());
        assert_eq!(verified.container, container);
        println!(
            "original copy proof: {video:?}/{audio:?} -> {container}, clip={clipped}, bytes={}",
            verified.byte_length
        );
    }
}

#[test]
#[ignore = "requires the pinned portable tools and a previously downloaded combined video; no network"]
fn real_combined_media_full_and_clipped_video_only_have_zero_audio() {
    let package =
        PathBuf::from(env::var_os("LMD_REAL_TOOL_PACKAGE").expect("tool package required"));
    let input =
        PathBuf::from(env::var_os("LMD_REAL_LOCAL_MEDIA").expect("local combined media required"));
    let manifest: ManagedToolManifest =
        serde_json::from_slice(&fs::read(package.join("manifest.json")).unwrap()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(FilesystemManagedToolStore::new(
        root.path().join("managed-tools"),
        BUILD_TARGET,
    ));
    let process = Arc::new(WindowsProcessSupervisor);
    ToolBootstrapService::new(
        store.as_ref(),
        &NativeToolVersionProbe::new(process.as_ref()),
        BUILD_TARGET,
    )
    .bootstrap(&package, &manifest)
    .unwrap();
    let clipper = FfmpegMediaProcessor::new(Arc::clone(&store), Arc::clone(&process));
    let verifier = FfprobeOutputVerifier::new(Arc::clone(&store), Arc::clone(&process));
    for (index, video_only, clip) in [
        (1, true, None),
        (
            2,
            true,
            Some(ClipRange {
                start_ms: 10000,
                end_ms: 30000,
            }),
        ),
        (
            3,
            false,
            Some(ClipRange {
                start_ms: 10000,
                end_ms: 30000,
            }),
        ),
    ] {
        let id = index.to_string();
        let staging = root.path().join(&id);
        let source = staging.join("source");
        fs::create_dir_all(source.join("input-0")).unwrap();
        fs::copy(&input, source.join("input-0/source.mkv")).unwrap();
        let operation = if video_only {
            SelectionOperation::ExtractVideo
        } else {
            SelectionOperation::Keep
        };
        let plan = SelectionPlan {
            compatibility_mode: false,
            container_selection:
                local_media_downloader_lib::domain::output_policy::ContainerSelection::Automatic,
            video_processing: Some(
                local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy,
            ),
            output_video_codec: Some("av1".into()),
            mode: if video_only {
                SelectionMode::VideoOnly
            } else {
                SelectionMode::Manual
            },
            source_format_ids: vec!["combined-local".into()],
            video_format_id: Some("combined-local".into()),
            audio_format_id: if video_only {
                None
            } else {
                Some("combined-local".into())
            },
            operation,
            output_container: Some("mp4".into()),
            output_audio_codec: (!video_only).then_some(
                local_media_downloader_lib::domain::output_policy::TargetAudioCodec::Aac,
            ),
            audio_processing: (!video_only).then_some(
                local_media_downloader_lib::domain::output_policy::AudioProcessing::Copy,
            ),
            clip,
            estimated_size_bytes: None,
            warnings: vec![],
        };
        let mut video_source = policy_fixture::video_format("combined-local", "av1");
        video_source.width = Some(3146);
        video_source.height = Some(2160);
        video_source.fps = None;
        // Literal properties of the synthetic media fixture.
        video_source.audio_sample_rate_hz = Some(44100);
        let processed = clipper
            .process(
                LocalProcessingRequest {
                    job_id: id.clone(),
                    staging_dir: staging.clone(),
                    source_dir: source.clone(),
                    plan: plan.clone(),
                    video_source: Some(video_source.clone()),
                    audio_source: (!video_only).then_some(video_source),
                    sources: policy_fixture::slots(&plan.source_format_ids),
                },
                CancellationToken::new(),
            )
            .unwrap();
        fs::remove_file(source.join("input-0/source.mkv")).unwrap();
        fs::remove_dir(source.join("input-0")).unwrap();
        fs::remove_dir(&source).unwrap();
        let verified = verifier
            .verify(
                StagedDownload {
                    job_id: id,
                    staging_dir: staging,
                    expected_operation: operation,
                    expected_container: Some("mp4".into()),
                    requested_clip: clip,
                    evidence: processed.evidence,
                },
                &plan,
                CancellationToken::new(),
            )
            .unwrap();
        assert!(verified.has_video);
        assert_eq!(verified.has_audio, !video_only);
        if let Some(clip) = clip {
            assert!(
                verified.duration_ms <= clip.end_ms - clip.start_ms + CLIP_DURATION_TOLERANCE_MS
            );
        } else {
            assert!(verified.duration_ms > 30000);
        }
        println!(
            "local proof: video_only={video_only}, clipped={}, duration_ms={}, bytes={}",
            clip.is_some(),
            verified.duration_ms,
            verified.byte_length
        );
    }
}
