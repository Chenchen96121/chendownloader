use local_media_downloader_lib::{
    application::{
        bilibili_source::{
            BilibiliAnalysisScope, BilibiliAnalyzer, BilibiliPageEvidence, BilibiliPageLoader,
            BilibiliRequestContext,
        },
        network_profile::NetworkProfile,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        bilibili::validate_bilibili_url,
        managed_tools::{ManagedToolError, ToolName},
        media::{AnalysisErrorCode, Platform},
    },
    infrastructure::{
        bilibili::page::parse_bilibili_page,
        ytdlp::bilibili::{match_bilibili_metadata, YtDlpBilibiliAnalyzer},
    },
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const PAGE: &[u8] = include_bytes!("../../tests/fixtures/bilibili/ordinary-page.html");
const METADATA: &[u8] = include_bytes!("../../tests/fixtures/bilibili/ordinary-metadata.json");
fn raw() -> Value {
    serde_json::from_slice(METADATA).unwrap()
}
fn request() -> local_media_downloader_lib::domain::bilibili::BilibiliVideoRequest {
    validate_bilibili_url("https://www.bilibili.com/video/av170001?p=2").unwrap()
}
fn evidence() -> BilibiliPageEvidence {
    parse_bilibili_page(&request(), PAGE).unwrap()
}
fn context() -> BilibiliRequestContext {
    BilibiliRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}
struct Pages;
impl BilibiliPageLoader for Pages {
    fn inspect(
        &self,
        request: &local_media_downloader_lib::domain::bilibili::BilibiliVideoRequest,
        _: &BilibiliRequestContext,
    ) -> Result<BilibiliPageEvidence, AnalysisErrorCode> {
        parse_bilibili_page(request, PAGE)
    }
}
struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: if name == ToolName::YtDlp {
                "2026.08.19"
            } else {
                "9.0.2"
            }
            .into(),
            path: PathBuf::from(r"C:\managed").join(format!("{}.exe", name.as_str())),
        })
    }
}
#[derive(Default)]
struct Process {
    responses: Mutex<VecDeque<Result<ProcessExit, ProcessFailureKind>>>,
    requests: Mutex<Vec<ProcessRequest>>,
    witness: Mutex<Option<Vec<u8>>>,
}
impl ProcessSupervisor for Process {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        assert!(!cancellation.is_canceled());
        if request.program.path().file_name().unwrap() == "yt-dlp.exe" {
            if let Some(bytes) = self.witness.lock().unwrap().as_ref() {
                std::fs::write(request.working_dir.join("consumed-page.dump"), bytes).unwrap();
            }
        }
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected process")
    }
}
fn exit(value: &Value, stderr: &[u8]) -> Result<ProcessExit, ProcessFailureKind> {
    Ok(ProcessExit {
        code: 0,
        stdout: serde_json::to_vec(value).unwrap(),
        stderr: stderr.into(),
    })
}
fn audio_probe() -> Value {
    json!({"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"90","size":"100"},"streams":[{"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2,"channel_layout":"stereo","bit_rate":"999999"}]})
}
fn video_probe() -> Value {
    json!({"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"90","size":"100"},"streams":[{"codec_type":"video","codec_name":"h264","profile":"High","width":1920,"height":1080,"avg_frame_rate":"30/1","pix_fmt":"yuv420p","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","bit_rate":"111"}]})
}
fn process(responses: Vec<Result<ProcessExit, ProcessFailureKind>>) -> Arc<Process> {
    Arc::new(Process {
        responses: Mutex::new(responses.into()),
        witness: Mutex::new(Some(PAGE.to_vec())),
        ..Default::default()
    })
}

#[test]
fn bilibili_requires_the_page_actually_consumed_by_the_extractor() {
    for witness in [
        None,
        Some(b"not an ordinary page".to_vec()),
        Some(vec![b'x'; 4 * 1024 * 1024 + 1]),
        Some(
            String::from_utf8(PAGE.to_vec())
                .unwrap()
                .replace("279787", "999999")
                .into_bytes(),
        ),
    ] {
        let p = process(vec![exit(&raw(), b""), exit(&audio_probe(), b"")]);
        *p.witness.lock().unwrap() = witness;
        assert!(YtDlpBilibiliAnalyzer::new(Resolver, p.clone(), Pages)
            .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
            .is_err());
        assert_eq!(p.requests.lock().unwrap().len(), 1, "no wrong-media probe");
    }
}

#[test]
fn bilibili_metadata_requires_exact_extractor_and_both_part_witnesses() {
    assert!(match_bilibili_metadata(&evidence(), &raw()).is_ok());
    for (key, value) in [
        ("id", json!("BV1xx411c7mD_p1")),
        ("extractor_key", json!("BiliBiliBangumi")),
        ("_old_archive_ids", json!(["bilibili 170002_part2"])),
        ("entries", json!([])),
        ("_type", json!("multi_video")),
        ("is_live", json!(true)),
    ] {
        let mut r = raw();
        r[key] = value;
        assert!(match_bilibili_metadata(&evidence(), &r).is_err(), "{key}");
    }
    let mut r = raw();
    r["duration"] = 30.into();
    assert!(match_bilibili_metadata(&evidence(), &r).is_err());
    r["duration"] = 88.into();
    assert!(match_bilibili_metadata(&evidence(), &r).is_ok());
    r["duration"] = 87.9.into();
    assert!(match_bilibili_metadata(&evidence(), &r).is_err());
}

#[test]
fn bilibili_absent_role_zero_bitrates_are_not_real_stream_properties() {
    let mut r = raw();
    r["formats"][0]["abr"] = json!(0);
    r["formats"][1]["vbr"] = json!(0);
    let p = process(vec![
        exit(&r, b""),
        exit(&video_probe(), b""),
        exit(&audio_probe(), b""),
    ]);
    let result = YtDlpBilibiliAnalyzer::new(Resolver, p, Pages).analyze(
        &request(),
        BilibiliAnalysisScope::Discover,
        &context(),
    );
    assert!(result.is_ok(), "fixed extractor zero sentinels for explicitly absent roles must not reject normal DASH streams");
    let media = result.unwrap();
    assert!(media
        .media()
        .formats
        .iter()
        .find(|f| f.id == "100026")
        .unwrap()
        .audio_bitrate_kbps
        .is_none());
    assert!(media
        .media()
        .formats
        .iter()
        .find(|f| f.id == "30280")
        .unwrap()
        .video_bitrate_kbps
        .is_none());
    for (index, field, value) in [
        (0, "vbr", 0),
        (1, "abr", 0),
        (0, "abr", -1),
        (1, "vbr", 100),
    ] {
        let mut r = raw();
        r["formats"][index][field] = json!(value);
        assert!(
            match_bilibili_metadata(&evidence(), &r).is_err(),
            "present-role zero / contradictory absent-role bitrate must remain invalid"
        );
    }
}

#[test]
fn bilibili_mcdn_port_format_is_probed_and_retained_without_lower_quality_fallback() {
    let mut r = raw();
    let signed = "https://mirror.mcdn.bilivideo.cn:8082/video.m4s?sign=SYNTHETIC%2f%2B";
    r["formats"][0]["url"] = signed.into();
    let p = process(vec![
        exit(&r, b""),
        exit(&video_probe(), b""),
        exit(&audio_probe(), b""),
    ]);
    let result = YtDlpBilibiliAnalyzer::new(Resolver, p.clone(), Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .unwrap();
    assert_eq!(result.media().formats[0].id, "100026");
    assert_eq!(result.media().formats[0].width, Some(1920));
    assert_eq!(result.media().formats.len(), 2);
    assert_eq!(
        p.requests.lock().unwrap()[1].args.last().unwrap().to_str(),
        Some(signed)
    );
}

#[test]
fn bilibili_trial_success_is_rejected() {
    for stderr in [
        b"WARNING: only preview video is available".as_slice(),
        "WARNING: 试看".as_bytes(),
        b"[BiliBili] Dropping incompatible flv format(s) 1, 2 since mp4 is available".as_slice(),
    ] {
        let p = process(vec![exit(&raw(), stderr)]);
        assert!(YtDlpBilibiliAnalyzer::new(Resolver, p.clone(), Pages)
            .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
            .is_err());
        assert_eq!(p.requests.lock().unwrap().len(), 1);
    }
    let p=process(vec![exit(&raw(),b"[BiliBili] Format(s) 1080P missing; you have to become premium member to download them"),exit(&video_probe(),b""),exit(&audio_probe(),b"")]);
    assert!(YtDlpBilibiliAnalyzer::new(Resolver, p, Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .is_ok());
}

#[test]
fn bilibili_unknown_top_candidate_blocks_downgrade() {
    let mut r = raw();
    let mut top = r["formats"][0].clone();
    top["format_id"] = "unknown-top".into();
    top.as_object_mut().unwrap().remove("width");
    top.as_object_mut().unwrap().remove("height");
    r["formats"].as_array_mut().unwrap().insert(0, top);
    let p = process(vec![exit(&r, b""), Err(ProcessFailureKind::TimedOut)]);
    assert!(matches!(
        YtDlpBilibiliAnalyzer::new(Resolver, p, Pages).analyze(
            &request(),
            BilibiliAnalysisScope::Discover,
            &context()
        ),
        Err(AnalysisErrorCode::AnalysisTimeout)
    ));
    for protocol in ["m3u8_native", "http_dash_segments", "http", "unknown"] {
        let mut r = raw();
        r["formats"][0]["protocol"] = protocol.into();
        assert!(
            match_bilibili_metadata(&evidence(), &r).is_err(),
            "{protocol}"
        );
    }
    let mut r = raw();
    r["formats"][0]["fragments"] = json!([]);
    assert!(match_bilibili_metadata(&evidence(), &r).is_err());
}

#[test]
fn bilibili_projects_correct_bandwidth_and_preserves_whole_stream_size() {
    let p = process(vec![
        exit(&raw(), b""),
        exit(&video_probe(), b""),
        exit(&audio_probe(), b""),
    ]);
    let result = YtDlpBilibiliAnalyzer::new(Resolver, p, Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .unwrap();
    let m = result.media();
    assert_eq!(m.platform, Platform::Bilibili);
    assert_eq!(m.media_id, "BV1xx411c7mD_p2");
    let v = &m.formats[0];
    let a = &m.formats[1];
    assert_eq!(v.video_bitrate_kbps, Some(3500.0));
    assert_eq!(v.audio_bitrate_kbps, None);
    assert_eq!(a.audio_bitrate_kbps, Some(192.0));
    assert_eq!(a.video_bitrate_kbps, None);
    assert_eq!(a.file_size.unwrap().bytes, 2160000);
    assert_eq!(v.file_size.unwrap().bytes, 39375000);
    assert_eq!(a.audio_sample_rate_hz, Some(48000));
    assert_eq!(a.audio_channels, Some(2));
    let text = serde_json::to_string(m).unwrap();
    assert!(!text.contains("bilivideo"));
    assert!(!text.contains("SYNTHETIC"));
}

#[test]
fn bilibili_unknown_hdr_is_hydrated_instead_of_assumed_sdr() {
    let mut r = raw();
    r["formats"][0]
        .as_object_mut()
        .unwrap()
        .remove("dynamic_range");
    let mut probe = video_probe();
    probe["streams"][0]["color_transfer"] = "smpte2084".into();
    let p = process(vec![
        exit(&r, b""),
        exit(&probe, b""),
        exit(&audio_probe(), b""),
    ]);
    let result = YtDlpBilibiliAnalyzer::new(Resolver, p, Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .unwrap();
    assert_eq!(
        result.media().formats[0].dynamic_range.as_deref(),
        Some("HDR10")
    );
}

#[test]
fn bilibili_processes_have_fixed_scope_output_limits_and_per_input_network_guards() {
    let p = process(vec![
        exit(&raw(), b""),
        exit(&video_probe(), b""),
        exit(&audio_probe(), b""),
    ]);
    YtDlpBilibiliAnalyzer::new(Resolver, p.clone(), Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .unwrap();
    let requests = p.requests.lock().unwrap();
    let m = &requests[0];
    let args: Vec<_> = m.args.iter().map(|s| s.to_str().unwrap()).collect();
    for flag in [
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-remote-components",
        "--no-playlist",
        "--quiet",
        "--verbose",
        "--dump-single-json",
        "--write-pages",
        "--no-cookies",
        "--no-cookies-from-browser",
        "--no-update",
        "--no-cache-dir",
    ] {
        assert!(args.contains(&flag), "{flag}");
    }
    assert!(!args.contains(&"--no-warnings"));
    assert!(args.windows(2).any(|pair| pair == ["--encoding", "utf-8"]),
        "the native UTF-8 notice parser must not receive Windows code-page output for non-ASCII titles");
    assert_eq!(
        args.last(),
        Some(&"https://www.bilibili.com/video/BV1xx411c7mD?p=2")
    );
    assert!(m.timeout.unwrap() <= Duration::from_secs(45));
    assert!(matches!(
        m.output_mode,
        OutputMode::Capture {
            stdout_limit: 16777216,
            stderr_limit: 65536
        }
    ));
    let probe = &requests[1];
    let args: Vec<_> = probe.args.iter().map(|s| s.to_str().unwrap()).collect();
    assert!(args.windows(2).any(|s| s == ["-max_redirects", "0"]));
    assert!(args.windows(2).any(|s| s == ["-rw_timeout", "8000000"]));
    assert!(args.contains(&"-http_proxy"));
    assert!(args.contains(&"-protocol_whitelist"));
    assert!(args.contains(&"-format_whitelist"));
    assert!(probe.timeout.unwrap() <= Duration::from_secs(15));
    assert!(matches!(
        probe.output_mode,
        OutputMode::Capture {
            stdout_limit: 65536,
            stderr_limit: 65536
        }
    ));
}

#[test]
fn bilibili_refresh_only_probes_frozen_selected_streams() {
    let p = process(vec![exit(&raw(), b""), exit(&video_probe(), b"")]);
    let result = YtDlpBilibiliAnalyzer::new(Resolver, p.clone(), Pages)
        .analyze(
            &request(),
            BilibiliAnalysisScope::RefreshSelected(vec!["100026".into()]),
            &context(),
        )
        .unwrap();
    assert_eq!(result.media().formats.len(), 1);
    assert_eq!(result.media().formats[0].id, "100026");
    assert_eq!(p.requests.lock().unwrap().len(), 2);
}

#[test]
fn bilibili_rejects_duplicate_formats_hostile_urls_and_raw_output_overflows() {
    let mut r = raw();
    let duplicate = r["formats"][0].clone();
    r["formats"].as_array_mut().unwrap().push(duplicate);
    assert!(match_bilibili_metadata(&evidence(), &r).is_err());
    let mut r = raw();
    r["formats"][0]["url"] = "https://evil.test/video".into();
    assert!(match_bilibili_metadata(&evidence(), &r).is_err());
    let mut r = raw();
    r["formats"] = Value::Array(vec![raw()["formats"][0].clone(); 513]);
    assert!(match_bilibili_metadata(&evidence(), &r).is_err());
    for output in [
        ProcessExit {
            code: 0,
            stdout: vec![b' '; 16777217],
            stderr: vec![],
        },
        ProcessExit {
            code: 0,
            stdout: METADATA.into(),
            stderr: vec![b'x'; 65537],
        },
        ProcessExit {
            code: 0,
            stdout: vec![0xff],
            stderr: vec![],
        },
    ] {
        let p = process(vec![Ok(output)]);
        assert!(YtDlpBilibiliAnalyzer::new(Resolver, p, Pages)
            .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
            .is_err());
    }
}

#[test]
fn bilibili_probe_call_budget_stops_before_seventeenth_call() {
    let mut r = raw();
    r["formats"] = Value::Array(
        (0..17)
            .map(|n| {
                let mut v = raw()["formats"][0].clone();
                v["format_id"] = format!("v{n}").into();
                v.as_object_mut().unwrap().remove("fps");
                v
            })
            .collect(),
    );
    let mut responses = vec![exit(&r, b"")];
    responses.extend((0..17).map(|_| exit(&video_probe(), b"")));
    let p = process(responses);
    assert!(YtDlpBilibiliAnalyzer::new(Resolver, p.clone(), Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .is_err());
    assert!(p.requests.lock().unwrap().len() <= 17);
}

#[test]
fn bilibili_debug_urls_are_not_mistaken_for_trial_warnings() {
    let p=process(vec![exit(&raw(),b"[debug] Proxy map: {'https': 'http://trial.proxy.test:8080'}\n[debug] Command-line config: ['--', 'https://www.bilibili.com/video/BV1TrialABCd?p=2']"),exit(&video_probe(),b""),exit(&audio_probe(),b"")]);
    assert!(YtDlpBilibiliAnalyzer::new(Resolver, p, Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .is_ok());
}

#[test]
fn bilibili_invalid_known_facts_are_not_silently_dropped() {
    for (key, value) in [
        ("fps", json!(241)),
        ("width", json!(16385)),
        ("vbr", json!(1000001)),
        ("filesize", json!(9007199254740992_u64)),
    ] {
        let mut r = raw();
        r["formats"][0][key] = value;
        assert!(match_bilibili_metadata(&evidence(), &r).is_err(), "{key}");
    }
    for (key, value) in [
        ("asr", json!(768001)),
        ("audio_channels", json!(65)),
        ("abr", json!(100001)),
    ] {
        let mut r = raw();
        r["formats"][1][key] = value;
        assert!(match_bilibili_metadata(&evidence(), &r).is_err(), "{key}");
    }
}

#[test]
fn bilibili_probed_audio_cannot_contradict_known_source_properties() {
    for (key, value) in [("asr", json!(44100)), ("audio_channels", json!(6))] {
        let mut r = raw();
        r["formats"][1][key] = value;
        let p = process(vec![exit(&r, b""), exit(&audio_probe(), b"")]);
        assert!(
            YtDlpBilibiliAnalyzer::new(Resolver, p, Pages)
                .analyze(
                    &request(),
                    BilibiliAnalysisScope::RefreshSelected(vec!["30280".into()]),
                    &context()
                )
                .is_err(),
            "{key}"
        );
    }
}

#[test]
fn bilibili_missing_color_evidence_stays_unknown() {
    let mut r = raw();
    r["formats"][0]
        .as_object_mut()
        .unwrap()
        .remove("dynamic_range");
    let mut probe = video_probe();
    probe["streams"][0]
        .as_object_mut()
        .unwrap()
        .remove("color_transfer");
    let p = process(vec![
        exit(&r, b""),
        exit(&probe, b""),
        exit(&audio_probe(), b""),
    ]);
    let result = YtDlpBilibiliAnalyzer::new(Resolver, p, Pages)
        .analyze(&request(), BilibiliAnalysisScope::Discover, &context())
        .unwrap();
    assert_eq!(result.media().formats[0].dynamic_range, None);
}
