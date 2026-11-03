#[path = "support/output_policy.rs"]
mod policy;
use local_media_downloader_lib::{
    application::{
        local_processing::{LocalMediaProcessor, LocalProcessingRequest},
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::{SelectionMode, SelectionOperation, SelectionPlan},
        managed_tools::{ManagedToolError, ToolName},
        output_policy::{ContainerSelection, StreamProcessing, TargetAudioCodec},
    },
    infrastructure::ffmpeg::FfmpegMediaProcessor,
};
use std::{fs, path::PathBuf, sync::Mutex};

struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixed-test".into(),
            path: PathBuf::from(format!("C:/managed/{}.exe", name.as_str())),
        })
    }
}
struct Process {
    document: serde_json::Value,
    frames: Option<serde_json::Value>,
    calls: Mutex<Vec<ProcessRequest>>,
}
impl ProcessSupervisor for Process {
    fn run(
        &self,
        request: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        if request.args.iter().any(|a| a == "-show_entries") {
            return Ok(ProcessExit {
                code: 0,
                stdout: serde_json::to_vec(if request.args.iter().any(|a| a == "-show_frames") {
                    self.frames.as_ref().unwrap_or(&self.document)
                } else {
                    &self.document
                })
                .unwrap(),
                stderr: vec![],
            });
        }
        fs::write(PathBuf::from(request.args.last().unwrap()), b"output").unwrap();
        self.calls.lock().unwrap().push(request);
        Ok(ProcessExit {
            code: 0,
            stdout: vec![],
            stderr: vec![],
        })
    }
}
fn fixture(
    compatible: bool,
    video: bool,
    audio: bool,
    channels: u8,
) -> (tempfile::TempDir, LocalProcessingRequest, Process) {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("42");
    let source = staging.join("source");
    fs::create_dir_all(source.join("input-0")).unwrap();
    fs::write(source.join("input-0/source.mkv"), b"source").unwrap();
    let mut row = policy::video_format("original", "av1");
    row.audio_codec = Some("opus".into());
    row.audio_channels = Some(channels);
    let plan = SelectionPlan {
        compatibility_mode: compatible,
        container_selection: if compatible {
            ContainerSelection::Compatibility
        } else {
            ContainerSelection::Automatic
        },
        video_processing: video.then_some(if compatible {
            StreamProcessing::Transcode
        } else {
            StreamProcessing::Copy
        }),
        output_video_codec: video.then(|| if compatible { "h264" } else { "av1" }.into()),
        mode: if !video {
            SelectionMode::AudioOnly
        } else if !audio {
            SelectionMode::VideoOnly
        } else {
            SelectionMode::BestAvailable
        },
        source_format_ids: vec!["original".into()],
        video_format_id: video.then(|| "original".into()),
        audio_format_id: audio.then(|| "original".into()),
        operation: if !video {
            SelectionOperation::ExtractAudio
        } else if !audio {
            SelectionOperation::ExtractVideo
        } else {
            SelectionOperation::Keep
        },
        output_container: Some(
            if video {
                "mp4"
            } else if compatible {
                "m4a"
            } else {
                "webm"
            }
            .into(),
        ),
        output_audio_codec: audio.then_some(if compatible {
            TargetAudioCodec::Aac
        } else {
            TargetAudioCodec::Opus
        }),
        audio_processing: audio.then_some(if compatible {
            StreamProcessing::Transcode
        } else {
            StreamProcessing::Copy
        }),
        clip: None,
        estimated_size_bytes: None,
        warnings: vec![],
    };
    let mut streams = vec![];
    if video {
        streams.push(serde_json::json!({"codec_type":"video","codec_name":"av1","profile":"Main","width":1280,"height":720,"avg_frame_rate":"30/1","pix_fmt":"yuv420p","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709"}));
    }
    if audio {
        streams.push(serde_json::json!({"codec_type":"audio","codec_name":"opus","channels":channels,"sample_rate":"48000","channel_layout":match channels{1=>"mono",2=>"stereo",_=>"5.1"}}));
    }
    let request = LocalProcessingRequest {
        job_id: "42".into(),
        staging_dir: staging,
        source_dir: source,
        plan: plan.clone(),
        video_source: video.then(|| row.clone()),
        audio_source: audio.then_some(row),
        sources: policy::slots(&plan.source_format_ids),
    };
    (
        root,
        request,
        Process {
            frames: None,
            document: serde_json::json!({"programs":[],"stream_groups":[],"format":{},"streams":streams}),
            calls: Mutex::new(vec![]),
        },
    )
}

#[test]
fn missing_av1_summary_colors_must_be_proven_by_bounded_frames_before_conversion() {
    let (_root, request, mut process) = fixture(true, true, true, 2);
    process.document["streams"][0]["color_transfer"] = serde_json::Value::Null;
    process.document["streams"][0]["color_primaries"] = serde_json::Value::Null;
    process.frames = Some(
        serde_json::json!({"frames":[{"pix_fmt":"yuv420p","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709"}]}),
    );
    let result =
        FfmpegMediaProcessor::new(Resolver, &process).process(request, CancellationToken::new());
    assert!(
        result.is_ok(),
        "verified decoded color properties should authorize conversion"
    );
    assert!(pair(&arguments(&process), "-c:v", "libx264"));
}
fn arguments(process: &Process) -> Vec<String> {
    process.calls.lock().unwrap()[0]
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}
fn pair(args: &[String], name: &str, value: &str) -> bool {
    args.windows(2).any(|p| p == [name, value])
}

#[test]
fn mp4_stream_copy_preserves_known_container_colors_without_inventing_unknowns() {
    for (range, known_matrix) in [
        (Some("tv"), false),
        (Some("pc"), false),
        (None, false),
        (None, true),
    ] {
        let (_root, request, mut process) = fixture(false, true, true, 2);
        for key in ["color_space", "color_transfer", "color_primaries"] {
            process.document["streams"][0]
                .as_object_mut()
                .unwrap()
                .remove(key);
        }
        if let Some(range) = range {
            process.document["streams"][0]["color_range"] = range.into();
        } else {
            process.document["streams"][0]
                .as_object_mut()
                .unwrap()
                .remove("color_range");
        }
        if known_matrix {
            process.document["streams"][0]["color_space"] = "bt709".into();
        }
        FfmpegMediaProcessor::new(Resolver, &process)
            .process(request, CancellationToken::new())
            .unwrap();
        let args = arguments(&process);
        assert!(pair(&args, "-c:v", "copy"));
        assert!(pair(
            &args,
            "-movflags",
            if range.is_some() {
                "+faststart+write_colr"
            } else {
                "+faststart"
            }
        ));
        assert!(!args.iter().any(|a| matches!(
            a.as_str(),
            "-color_range" | "-colorspace" | "-color_trc" | "-color_primaries" | "-bsf:v"
        )));
    }
}

#[test]
fn compatibility_requires_opt_in_and_rejects_unverified_color_or_dimensions() {
    for compatible in [false, true] {
        for channels in [2, 6] {
            let (_root, request, process) = fixture(compatible, true, true, channels);
            let output = FfmpegMediaProcessor::new(Resolver, &process)
                .process(request, CancellationToken::new())
                .unwrap();
            let args = arguments(&process);
            assert!(pair(
                &args,
                "-c:v",
                if compatible { "libx264" } else { "copy" }
            ));
            assert!(pair(&args, "-c:a", if compatible { "aac" } else { "copy" }));
            if compatible {
                assert!(pair(&args, "-crf", "18"));
                assert!(pair(&args, "-preset", "medium"));
                assert!(pair(&args, "-pix_fmt", "yuv420p"));
                assert!(pair(
                    &args,
                    "-b:a",
                    if channels == 2 { "192k" } else { "512k" }
                ));
                assert_eq!(
                    output.evidence.video_processing,
                    Some(StreamProcessing::Transcode)
                );
            } else {
                assert!(!args
                    .iter()
                    .any(|a| matches!(a.as_str(), "-crf" | "-b:a" | "-vf")));
            }
            assert!(!args
                .iter()
                .any(|a| matches!(a.as_str(), "-r" | "-s" | "-ac" | "-b:v")));
        }
    }
    for (field, value) in [
        ("width", serde_json::json!(1279)),
        ("color_transfer", serde_json::Value::Null),
        ("pix_fmt", serde_json::json!("unknown")),
    ] {
        let (_root, request, mut process) = fixture(true, true, true, 2);
        process.document["streams"][0][field] = value;
        assert!(FfmpegMediaProcessor::new(Resolver, &process)
            .process(request, CancellationToken::new())
            .is_err());
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn compatibility_audio_only_and_video_only_do_not_invent_tracks() {
    for (video, audio) in [(false, true), (true, false)] {
        let (_root, request, process) = fixture(true, video, audio, 2);
        FfmpegMediaProcessor::new(Resolver, &process)
            .process(request, CancellationToken::new())
            .unwrap();
        let args = arguments(&process);
        assert_eq!(args.iter().any(|a| a == "-c:v"), video);
        assert_eq!(args.iter().any(|a| a == "-c:a"), audio);
        assert_eq!(args.iter().any(|a| a == "-vn"), !video);
        assert_eq!(args.iter().any(|a| a == "-an"), !audio);
    }
}

#[test]
fn h264_high10_and_444_are_converted_but_supported_sdr_profiles_are_copied() {
    use local_media_downloader_lib::infrastructure::ffmpeg::compatibility::decide_compatibility;
    let mut video = policy::video_proof();
    video.color_range = Some("tv".into());
    video.color_transfer = Some("bt709".into());
    video.color_space = Some("bt709".into());
    video.color_primaries = Some("bt709".into());
    for (profile, pixels, expected) in [
        ("High", "yuv420p", StreamProcessing::Copy),
        ("Main", "yuv420p", StreamProcessing::Copy),
        ("Constrained Baseline", "yuv420p", StreamProcessing::Copy),
        ("High 10", "yuv420p10le", StreamProcessing::Transcode),
        (
            "High 4:4:4 Predictive",
            "yuv444p",
            StreamProcessing::Transcode,
        ),
    ] {
        video.profile = Some(profile.into());
        video.pixel_format = Some(pixels.into());
        assert_eq!(
            decide_compatibility(Some(&video), None)
                .unwrap()
                .video_processing,
            Some(expected)
        );
    }
    video.width = 1279;
    assert_eq!(
        decide_compatibility(Some(&video), None).err(),
        Some(DownloadErrorCode::FormatUnavailable)
    );
}

#[test]
fn compatibility_audio_rejects_unverified_sample_rates_and_layouts() {
    for (field, value) in [
        ("channels", serde_json::json!(8)),
        ("sample_rate", serde_json::json!("96000")),
        ("channel_layout", serde_json::Value::Null),
    ] {
        let (_root, request, mut process) = fixture(true, false, true, 2);
        process.document["streams"][0][field] = value;
        assert!(FfmpegMediaProcessor::new(Resolver, &process)
            .process(request, CancellationToken::new())
            .is_err());
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn verified_pq_and_hlg_use_real_tone_mapping_and_unknown_hdr_is_rejected() {
    use local_media_downloader_lib::infrastructure::ffmpeg::compatibility::{
        compatibility_filter, decide_compatibility,
    };
    let mut source = policy::video_proof();
    source.codec = "hevc".into();
    source.profile = Some("Main 10".into());
    source.pixel_format = Some("yuv420p10le".into());
    source.color_range = Some("tv".into());
    source.color_space = Some("bt2020nc".into());
    source.color_primaries = Some("bt2020".into());
    for transfer in ["smpte2084", "arib-std-b67"] {
        source.color_transfer = Some(transfer.into());
        let decision = decide_compatibility(Some(&source), None).unwrap();
        assert!(decision.hdr_to_sdr);
        assert_eq!(decision.video_processing, Some(StreamProcessing::Transcode));
        let filter = compatibility_filter(&source).unwrap().unwrap();
        assert!(filter.contains("zscale=t=linear:npl=100"));
        assert!(filter.contains("tonemap=tonemap=hable:desat=0"));
        assert!(!filter.split(',').any(|entry| entry.starts_with("scale=")
            || entry.starts_with("fps=")
            || entry.contains(":w=")
            || entry.contains(":h=")));
    }
    source.color_primaries = None;
    assert!(decide_compatibility(Some(&source), None).is_err());
    source.color_primaries = Some("bt2020".into());
    source.hdr_side_data = vec![serde_json::json!({"side_data_type":"DOVI configuration record"})];
    assert!(decide_compatibility(Some(&source), None).is_err());
    let (_root, mut request, process) = fixture(true, true, true, 2);
    request.video_source.as_mut().unwrap().dynamic_range = Some("DV".into());
    assert!(FfmpegMediaProcessor::new(Resolver, &process)
        .process(request, CancellationToken::new())
        .is_err());
    assert!(process.calls.lock().unwrap().is_empty());
}
