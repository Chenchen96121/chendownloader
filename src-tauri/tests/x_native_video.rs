use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
        x_source::XRequestContext,
        x_video::XVideoAnalyzer,
    },
    domain::managed_tools::{ManagedToolError, ToolName},
    infrastructure::{x::inventory::parse_x_inventory, ytdlp::x::YtDlpXVideoAnalyzer},
};
use serde_json::{json, Value};
#[path = "support/x_hls.rs"]
mod hls_fixture;

#[test]
fn x_hls_unknown_codec_probe_accepts_one_consistent_program_without_using_playlist_size() {
    use local_media_downloader_lib::infrastructure::x::manifest::HlsManifestTransport;
    struct Manifest;
    impl HlsManifestTransport for Manifest {
        fn read(
            &self,
            _: &url::Url,
            _: &XRequestContext,
        ) -> Result<Vec<u8>, local_media_downloader_lib::domain::media::AnalysisErrorCode> {
            Ok(b"#EXTM3U\n#EXTINF:1,\npart.ts\n#EXT-X-ENDLIST\n".to_vec())
        }
    }
    struct ProbeProcess {
        calls: Mutex<Vec<ProcessRequest>>,
    }
    impl ProcessSupervisor for ProbeProcess {
        fn run(
            &self,
            request: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let value = if request.program.path().ends_with("yt-dlp.exe") {
                let mut raw: Value =
                    serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json"))
                        .unwrap();
                raw["entries"][1]["formats"].as_array_mut().unwrap().push(json!({"format_id":"hls","url":"https://video.twimg.com/fixture/high.m3u8","manifest_url":"https://video.twimg.com/fixture/master.m3u8","protocol":"m3u8_native","ext":"mp4","width":3840,"height":2160,"fps":60,"vcodec":null,"acodec":null,"vbr":16000,"abr":160}));
                raw
            } else {
                let input = request.args.last().unwrap();
                assert!(!std::fs::read_to_string(input).unwrap().contains("https://"));
                let streams = json!([{"codec_type":"video","codec_name":"av1","width":3840,"height":2160,"avg_frame_rate":"60/1","side_data_list":[{}]},{"codec_type":"audio","codec_name":"opus","sample_rate":"48000","channels":2}]);
                json!({"programs":[{"streams":streams}],"stream_groups":[],"streams":streams,"format":{"duration":"12","size":"118"}})
            };
            self.calls.lock().unwrap().push(request);
            Ok(ProcessExit {
                code: 0,
                stdout: serde_json::to_vec(&value).unwrap(),
                stderr: vec![],
            })
        }
    }
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../../fixtures/x/mixed-post.json")).unwrap();
    raw[0][1]["legacy"]["extended_entities"]["media"][1]["video_info"]["variants"].as_array_mut().unwrap().push(json!({"content_type":"application/x-mpegURL","url":"https://video.twimg.com/fixture/master.m3u8"}));
    let inventory = parse_x_inventory(&serde_json::to_vec(&raw).unwrap(), &source).unwrap();
    let process = ProbeProcess {
        calls: Mutex::new(vec![]),
    };
    let analyzer =
        YtDlpXVideoAnalyzer::with_transports(Resolver, &process, Manifest, hls_fixture::Segments);
    let context = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let video = analyzer
        .analyze_item(
            &inventory,
            inventory.items[1].identity.as_ref().unwrap(),
            false,
            &context,
        )
        .unwrap();
    assert_eq!(video.plan().video_format_id.as_deref(), Some("hls"));
    assert_ne!(video.plan().estimated_size_bytes, Some(118));
    let calls = process.calls.lock().unwrap();
    let args = calls[1]
        .args
        .iter()
        .map(|s| s.to_str().unwrap())
        .collect::<Vec<_>>();
    assert!(args
        .windows(2)
        .any(|p| p == ["-protocol_whitelist", "file"]));
    assert!(!args.contains(&"-http_proxy") && !args.contains(&"-max_redirects"));
    assert!(!std::path::Path::new(args.last().unwrap()).exists());
    assert!(!args.last().unwrap().starts_with("https://"));
}

#[test]
fn x_higher_hls_candidate_has_a_sealed_snapshot_instead_of_lower_mp4_fallback() {
    use local_media_downloader_lib::infrastructure::x::manifest::HlsManifestTransport;
    struct Manifest;
    impl HlsManifestTransport for Manifest {
        fn read(
            &self,
            _: &url::Url,
            _: &XRequestContext,
        ) -> Result<Vec<u8>, local_media_downloader_lib::domain::media::AnalysisErrorCode> {
            Ok(b"#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\npart.ts\n#EXT-X-ENDLIST\n".to_vec())
        }
    }
    struct HlsProcess(usize);
    impl ProcessSupervisor for HlsProcess {
        fn run(
            &self,
            _: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let mut raw: Value =
                serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json"))
                    .unwrap();
            for n in 0..self.0 {
                let id = if n == 0 {
                    "hls".to_owned()
                } else {
                    format!("hls-{n}")
                };
                raw["entries"][1]["formats"].as_array_mut().unwrap().push(json!({"format_id":id,"url":format!("https://video.twimg.com/fixture/high{n}.m3u8"),"manifest_url":"https://video.twimg.com/fixture/master.m3u8","protocol":"m3u8_native","ext":"mp4","width":3840,"height":2160,"fps":60,"vcodec":"av1","acodec":"opus","vbr":16000,"abr":160,"audio_channels":2,"asr":48000}));
            }
            Ok(ProcessExit {
                code: 0,
                stdout: serde_json::to_vec(&raw).unwrap(),
                stderr: vec![],
            })
        }
    }
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../../fixtures/x/mixed-post.json")).unwrap();
    raw[0][1]["legacy"]["extended_entities"]["media"][1]["video_info"]["variants"].as_array_mut().unwrap().push(json!({"content_type":"application/x-mpegURL","url":"https://video.twimg.com/fixture/master.m3u8"}));
    let inventory = parse_x_inventory(&serde_json::to_vec(&raw).unwrap(), &source).unwrap();
    let analyzer = YtDlpXVideoAnalyzer::with_transports(
        Resolver,
        HlsProcess(1),
        Manifest,
        hls_fixture::Segments,
    );
    let ctx = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let video = analyzer
        .analyze_item(
            &inventory,
            inventory.items[1].identity.as_ref().unwrap(),
            false,
            &ctx,
        )
        .unwrap();
    assert_eq!(video.plan().video_format_id.as_deref(), Some("hls"));
    let snapshot = video.source().hls_input("hls").unwrap().unwrap();
    snapshot.revalidate().unwrap();
    assert!(snapshot.input().is_file());
    let excessive =
        YtDlpXVideoAnalyzer::with_manifest_transport(Resolver, HlsProcess(17), Manifest);
    assert!(matches!(
        excessive.analyze_item(
            &inventory,
            inventory.items[1].identity.as_ref().unwrap(),
            false,
            &ctx
        ),
        Err(local_media_downloader_lib::domain::media::AnalysisErrorCode::InvalidMetadata)
    ));
    struct Master;
    impl HlsManifestTransport for Master {
        fn read(
            &self,
            url: &url::Url,
            _: &XRequestContext,
        ) -> Result<Vec<u8>, local_media_downloader_lib::domain::media::AnalysisErrorCode> {
            Ok(if url.path().contains("high") {
                b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nchild.m3u8\n".to_vec()
            } else {
                b"#EXTM3U\n#EXTINF:1,\na.ts\n#EXT-X-ENDLIST\n".to_vec()
            })
        }
    }
    let master = YtDlpXVideoAnalyzer::with_manifest_transport(Resolver, HlsProcess(1), Master);
    assert!(matches!(
        master.analyze_item(
            &inventory,
            inventory.items[1].identity.as_ref().unwrap(),
            false,
            &ctx
        ),
        Err(local_media_downloader_lib::domain::media::AnalysisErrorCode::FormatUnavailable)
    ));
}
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixture".into(),
            path: PathBuf::from(if name == ToolName::YtDlp {
                "C:/managed/yt-dlp.exe"
            } else {
                "C:/managed/ffprobe.exe"
            }),
        })
    }
}
struct Process {
    calls: Mutex<Vec<ProcessRequest>>,
}
impl ProcessSupervisor for Process {
    fn run(
        &self,
        r: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let output = if r.program.path().ends_with("yt-dlp.exe") {
            let mut raw: Value =
                serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json"))
                    .unwrap();
            raw["entries"][1]["formats"][1]["vcodec"] = Value::Null;
            raw
        } else {
            json!({"streams":[{"codec_type":"video","codec_name":"vp9","width":1920,"height":1080,"avg_frame_rate":"60/1","color_transfer":"smpte2084","bit_rate":"8000000"},{"codec_type":"audio","codec_name":"opus","bit_rate":"160000","sample_rate":"48000","channels":2}],"format":{"duration":"12","size":"12240000"}})
        };
        self.calls.lock().unwrap().push(r);
        Ok(ProcessExit {
            code: 0,
            stdout: serde_json::to_vec(&output).unwrap(),
            stderr: vec![],
        })
    }
}
#[test]
fn x_native_analyzer_hydrates_unknown_high_candidate_with_restricted_ffprobe() {
    let process = Process {
        calls: Mutex::new(vec![]),
    };
    let analyzer = YtDlpXVideoAnalyzer::new(Resolver, &process);
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    let inventory =
        parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), &source).unwrap();
    let ctx = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let video = analyzer
        .analyze_item(
            &inventory,
            inventory.items[1].identity.as_ref().unwrap(),
            false,
            &ctx,
        )
        .unwrap();
    let selected = video
        .media()
        .formats
        .iter()
        .find(|f| Some(&f.id) == video.plan().video_format_id.as_ref())
        .unwrap();
    assert_eq!(selected.width, Some(1920));
    assert_eq!(selected.height, Some(1080));
    assert_eq!(selected.fps, Some(60.0));
    assert_eq!(selected.video_codec.as_deref(), Some("vp9"));
    assert_eq!(selected.audio_codec.as_deref(), Some("opus"));
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let args = calls[1]
        .args
        .iter()
        .map(|s| s.to_str().unwrap())
        .collect::<Vec<_>>();
    assert!(args
        .windows(2)
        .any(|p| p == ["-protocol_whitelist", "https,http,tcp,tls,httpproxy"]));
    assert!(args.windows(2).any(|p| p == ["-max_redirects", "0"]));
    assert!(calls[1].timeout.unwrap() <= Duration::from_secs(15));
}

#[test]
fn x_comparison_facts_are_probed_before_selecting_the_winner_without_repeated_probes() {
    struct QualityProcess {
        missing: &'static str,
        unresolved: bool,
        calls: Mutex<Vec<ProcessRequest>>,
    }
    impl ProcessSupervisor for QualityProcess {
        fn run(
            &self,
            request: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let value = if request.program.path().ends_with("yt-dlp.exe") {
                let mut raw: Value =
                    serde_json::from_slice(include_bytes!("../../fixtures/x/video-formats.json"))
                        .unwrap();
                for row in raw["entries"][1]["formats"].as_array_mut().unwrap() {
                    row["width"] = json!(1280);
                    row["height"] = json!(720);
                    row["fps"] = json!(30);
                    row["vcodec"] = json!("h264");
                    row["acodec"] = json!("aac");
                    row["dynamic_range"] = json!("SDR");
                    if self.missing == "asr" {
                        row["abr"] = json!(128);
                    }
                }
                raw["entries"][1]["formats"][1][self.missing] = Value::Null;
                raw
            } else {
                assert_eq!(
                    request.args.last().unwrap().to_str().unwrap(),
                    "https://video.twimg.com/ext_tw_video/fixtureB/high.mp4"
                );
                let mut value = json!({"streams":[{"codec_type":"video","codec_name":"h264","width":1280,"height":720,"avg_frame_rate":"30/1","color_transfer":"bt709","bit_rate":"8000000"},{"codec_type":"audio","codec_name":"aac","bit_rate":"160000","sample_rate":"48000","channels":2}],"format":{"duration":"12"}});
                if self.missing == "asr" {
                    value["streams"][1]["bit_rate"] = json!("128000");
                    value["streams"][1]["sample_rate"] = json!("96000");
                }
                if self.unresolved {
                    value["streams"][0]["color_transfer"] = Value::Null;
                }
                value
            };
            self.calls.lock().unwrap().push(request);
            Ok(ProcessExit {
                code: 0,
                stdout: serde_json::to_vec(&value).unwrap(),
                stderr: vec![],
            })
        }
    }
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    let inventory =
        parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), &source).unwrap();
    for (missing, unresolved) in [
        ("dynamic_range", false),
        ("vbr", false),
        ("audio_channels", false),
        ("abr", false),
        ("asr", false),
        ("dynamic_range", true),
    ] {
        let process = QualityProcess {
            missing,
            unresolved,
            calls: Mutex::new(vec![]),
        };
        let analyzer = YtDlpXVideoAnalyzer::new(Resolver, &process);
        let context = XRequestContext::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(120),
            Arc::new(NetworkProfile::current()),
        );
        let result = analyzer.analyze_item(
            &inventory,
            inventory.items[1].identity.as_ref().unwrap(),
            false,
            &context,
        );
        if unresolved {
            assert!(matches!(
                result,
                Err(
                    local_media_downloader_lib::domain::media::AnalysisErrorCode::FormatUnavailable
                )
            ));
        } else {
            let selected = result.unwrap_or_else(|error| panic!("{missing}: {error:?}"));
            assert_eq!(selected.plan().video_format_id.as_deref(), Some("high"));
            assert_eq!(selected.plan().audio_format_id.as_deref(), Some("high"));
        }
        assert_eq!(
            process.calls.lock().unwrap().len(),
            2,
            "{missing}: probe once only"
        );
    }
}
