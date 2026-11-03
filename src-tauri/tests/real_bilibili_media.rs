#[path = "support/bilibili.rs"]
mod fixtures;
#[path = "support/real_media.rs"]
mod real_media;

use local_media_downloader_lib::{
    application::{
        bilibili_analysis::{BilibiliAnalysisService, BilibiliCoverLoader},
        bilibili_download::BilibiliDownloadRefresher,
        bilibili_source::{BilibiliPageEvidence, BilibiliRequestContext},
        download_executor::{
            DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink, StagedDownload,
        },
        local_processing::{LocalMediaProcessor, LocalProcessingRequest, ProcessedMedia},
        output_verifier::{OutputVerifier, VerifiedOutput},
        portable_package::EMBEDDED_TOOL_MANIFEST,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        bilibili::BilibiliCoverPayload,
        download::{DownloadErrorCode, DownloadProgress},
        format_selection::{ClipRange, SelectionIntent, SelectionMode},
        managed_tools::{ManagedToolError, ManagedToolManifest, ToolName, BUILD_TARGET},
        media::AnalysisErrorCode,
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor, ffprobe::FfprobeOutputVerifier,
        process::WindowsProcessSupervisor, ytdlp::download::YtDlpDownloadExecutor,
    },
};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

struct Resolver(PathBuf);
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        let manifest: ManagedToolManifest = serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).unwrap();
        Ok(ResolvedTool {
            name,
            version: manifest
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap()
                .version
                .clone(),
            path: self.0.join(name.artifact_name(BUILD_TARGET)),
        })
    }
}
struct Covers;
impl BilibiliCoverLoader for Covers {
    fn load(
        &self,
        _: &BilibiliPageEvidence,
        _: &BilibiliRequestContext,
    ) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        Err(AnalysisErrorCode::AnalysisUnavailable)
    }
}
struct Sink;
impl DownloadProgressSink for Sink {
    fn on_progress(&self, progress: DownloadProgress) {
        assert!(!progress.job_id.is_empty());
    }
}
fn best() -> SelectionIntent {
    SelectionIntent {
        mode: SelectionMode::BestAvailable,
        compatibility_mode: false,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}

struct BilibiliRealHarness {
    root: tempfile::TempDir,
    resolver: Resolver,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    video: PathBuf,
    audio: PathBuf,
    video_probe: Value,
    state: Arc<fixtures::FixtureState>,
}
impl BilibiliRealHarness {
    fn new(hdr: bool) -> Self {
        let package = real_media::tools();
        assert_eq!(
            fs::read(package.join("manifest.json")).unwrap(),
            EMBEDDED_TOOL_MANIFEST
        );
        let ffmpeg = package.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET));
        let ffprobe = package.join(ToolName::Ffprobe.artifact_name(BUILD_TARGET));
        let root = tempfile::tempdir().unwrap();
        let video = root.path().join("original-video.mp4");
        if hdr {
            real_media::checked(Command::new(&ffmpeg).args(["-v","error","-nostdin","-f","lavfi","-i","color=size=160x90:rate=30:duration=3,format=yuv420p10le,geq=lum='64+876*X/W':cb=512:cr=512","-c:v","libx265","-preset","ultrafast","-pix_fmt","yuv420p10le","-color_range","tv","-colorspace","bt2020nc","-color_primaries","bt2020","-color_trc","smpte2084","-x265-params","pools=1:frame-threads=1:log-level=error:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400","-n"]).arg(&video).output().unwrap());
        } else {
            let baseline = real_media::generate_source(
                &ffmpeg,
                &root.path().join("baseline.mp4"),
                Some("h264"),
                None,
            );
            // The fixed encoder may omit otherwise requested VUI primaries /
            // transfer. Build a fully specified BT.709 fixture; compatibility
            // must not invent these properties for an unknown real source.
            real_media::checked(Command::new(&ffmpeg).args(["-v", "error", "-nostdin", "-i"]).arg(&baseline)
                .args(["-map", "0:v:0", "-c:v", "copy", "-bsf:v", "h264_metadata=colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1:video_full_range_flag=0", "-movflags", "+write_colr", "-n"])
                .arg(&video).output().unwrap());
        }
        let audio = real_media::generate_source(
            &ffmpeg,
            &root.path().join("original-audio.m4a"),
            None,
            Some("aac"),
        );
        let video_probe = real_media::probe(&ffprobe, &video);
        let audio_probe = real_media::probe(&ffprobe, &audio);
        let state = Arc::new(fixtures::FixtureState::default());
        {
            let mut page = state.page.lock().unwrap();
            page["videoData"]["duration"] = json!(6);
            for part in page["videoData"]["pages"].as_array_mut().unwrap() {
                part["duration"] = json!(3);
            }
            let mut metadata = state.metadata.lock().unwrap();
            metadata["duration"] = json!(3);
            let rows = metadata["formats"].as_array_mut().unwrap();
            for row in rows.iter_mut() {
                row.as_object_mut().unwrap().remove("tbr");
            }
            rows[0]["vcodec"] = json!(if hdr { "hevc" } else { "h264" });
            rows[0]["width"] = json!(160);
            rows[0]["height"] = json!(90);
            rows[0]["dynamic_range"] = json!(if hdr { "HDR10" } else { "SDR" });
            rows[0]["filesize"] = json!(fs::metadata(&video).unwrap().len());
            rows[1]["filesize"] = json!(fs::metadata(&audio).unwrap().len());
            *state.video.lock().unwrap() = analysis_probe_view(&video_probe);
            *state.audio.lock().unwrap() = analysis_probe_view(&audio_probe);
        }
        Self {
            root,
            resolver: Resolver(package),
            ffmpeg,
            ffprobe,
            video,
            audio,
            video_probe,
            state,
        }
    }
    fn use_combined_source(&mut self) {
        let combined = self.root.path().join("original-combined.mp4");
        real_media::checked(
            Command::new(&self.ffmpeg)
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(&self.video)
                .arg("-i")
                .arg(&self.audio)
                .args([
                    "-map",
                    "0:v:0",
                    "-map",
                    "1:a:0",
                    "-c",
                    "copy",
                    "-movflags",
                    "+write_colr",
                    "-n",
                ])
                .arg(&combined)
                .output()
                .unwrap(),
        );
        let probe = real_media::probe(&self.ffprobe, &combined);
        let mut metadata = self.state.metadata.lock().unwrap();
        let mut format = metadata["formats"][0].clone();
        format["acodec"] = "aac".into();
        format["filesize"] = json!(fs::metadata(&combined).unwrap().len());
        metadata["formats"] = json!([format]);
        *self.state.video.lock().unwrap() = analysis_probe_view(&probe);
        self.video_probe = probe;
        self.video = combined.clone();
        self.audio = combined;
    }
    fn request(&self, intent: SelectionIntent, job: &str) -> DownloadExecutionRequest {
        // Only upstream transport/probe fixtures are substituted. The production
        // analyzer, consumed-page witness, frozen snapshot and refresh checks run.
        let service = BilibiliAnalysisService::new(fixtures::Analyzer(self.state.clone()), Covers);
        let now = Instant::now();
        let data = service.analyze(fixtures::URL, intent.clone(), now).unwrap();
        let snapshot = service
            .prepare_download(&data.analysis_id, intent, now)
            .unwrap();
        let source = service
            .refresh(&snapshot, CancellationToken::new())
            .unwrap();
        let stage = self.root.path().join(job);
        fs::create_dir(&stage).unwrap();
        DownloadExecutionRequest {
            job_id: job.into(),
            source: snapshot.source().clone(),
            playlist_item: None,
            instagram_selector: None,
            x_source: None,
            bilibili_source: Some(source),
            plan: snapshot.plan().clone(),
            video_source: snapshot
                .media()
                .formats
                .iter()
                .find(|f| Some(&f.id) == snapshot.plan().video_format_id.as_ref())
                .cloned(),
            audio_source: snapshot
                .media()
                .formats
                .iter()
                .find(|f| Some(&f.id) == snapshot.plan().audio_format_id.as_ref())
                .cloned(),
            staging_dir: stage,
        }
    }
    fn execute(
        &self,
        intent: SelectionIntent,
        process: &impl ProcessSupervisor,
        job: &str,
    ) -> Result<StagedDownload, DownloadErrorCode> {
        YtDlpDownloadExecutor::new(
            &self.resolver,
            process,
            RecordingProcessor(FfmpegMediaProcessor::new(
                &self.resolver,
                WindowsProcessSupervisor,
            )),
        )
        .download(self.request(intent, job), CancellationToken::new(), &Sink)
    }
    fn download_and_verify(&self, intent: SelectionIntent, job: &str) -> VerifiedOutput {
        let request = self.request(intent, job);
        let plan = request.plan.clone();
        let process = LocalCopyProcess {
            video: &self.video,
            audio: &self.audio,
        };
        let staged = YtDlpDownloadExecutor::new(
            &self.resolver,
            &process,
            RecordingProcessor(FfmpegMediaProcessor::new(&self.resolver, WindowsProcessSupervisor)),
        )
        .download(request, CancellationToken::new(), &Sink)
        .unwrap_or_else(|error| panic!("real Bili transfer job={job}, mode={:?}, compatibility={}, clip={}, sourceColors={}/{}/{}: {error:?}", plan.mode, plan.compatibility_mode, plan.clip.is_some(), self.video_probe["streams"][0]["color_transfer"], self.video_probe["streams"][0]["color_space"], self.video_probe["streams"][0]["color_primaries"]));
        FfprobeOutputVerifier::new(&self.resolver, WindowsProcessSupervisor)
            .verify(staged, &plan, CancellationToken::new())
            .unwrap()
    }
}
struct RecordingProcessor<P>(P);
impl<P: LocalMediaProcessor> LocalMediaProcessor for RecordingProcessor<P> {
    fn process(
        &self,
        request: LocalProcessingRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode> {
        let result = self.0.process(request, cancellation);
        match &result {
            Ok(processed) => eprintln!(
                "Bili local processing evidence: video={:?}, audio={:?}, sourceVideo={:?}",
                processed.evidence.video_processing,
                processed.evidence.audio_processing,
                processed.evidence.source_video
            ),
            Err(error) => eprintln!("Bili local processor error: {error:?}"),
        }
        result
    }
}
fn analysis_probe_view(probe: &Value) -> Value {
    // Match the actual production -show_entries projection, not unrestricted
    // ffprobe -show_streams output (which intentionally fails closed).
    let fields = [
        "codec_type",
        "codec_name",
        "profile",
        "width",
        "height",
        "avg_frame_rate",
        "pix_fmt",
        "color_range",
        "color_space",
        "color_transfer",
        "color_primaries",
        "channels",
        "sample_rate",
        "channel_layout",
        "bit_rate",
        "side_data_list",
    ];
    let streams: Vec<_> = probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let projected: serde_json::Map<_, _> = row
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| fields.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            Value::Object(projected)
        })
        .collect();
    json!({ "format": { "format_name": probe["format"]["format_name"], "duration": probe["format"]["duration"], "size": probe["format"]["size"] }, "streams": streams })
}
struct LocalCopyProcess<'a> {
    video: &'a Path,
    audio: &'a Path,
}
impl ProcessSupervisor for LocalCopyProcess<'_> {
    fn run(
        &self,
        request: ProcessRequest,
        cancel: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        if request
            .program
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("yt-dlp")
        {
            let value = transfer_info(&request);
            let row = &value["formats"][0];
            let source = if row["format_id"] == "100026" {
                self.video
            } else {
                assert_eq!(row["format_id"], "30280");
                self.audio
            };
            fs::copy(
                source,
                request
                    .working_dir
                    .join(format!("source.{}", row["ext"].as_str().unwrap())),
            )
            .unwrap();
            Ok(ProcessExit {
                code: 0,
                stdout: vec![],
                stderr: vec![],
            })
        } else {
            WindowsProcessSupervisor.run(request, cancel, sink)
        }
    }
}
fn transfer_info(request: &ProcessRequest) -> Value {
    assert!(request
        .args
        .windows(2)
        .any(|p| p[0] == "--downloader" && p[1] == "ffmpeg"));
    let index = request
        .args
        .iter()
        .position(|a| a == "--load-info-json")
        .unwrap();
    let value: Value =
        serde_json::from_slice(&fs::read(Path::new(&request.args[index + 1])).unwrap()).unwrap();
    assert_eq!(value["formats"].as_array().unwrap().len(), 1);
    assert!(value.get("webpage_url").is_none() && value.get("http_headers").is_none());
    assert!(value["formats"][0]["url"]
        .as_str()
        .unwrap()
        .starts_with("https://fixture.bilivideo."));
    value
}
fn packet_hashes(ffprobe: &Path, file: &Path, role: &str) -> Vec<String> {
    let value: Value = serde_json::from_slice(&real_media::checked(
        Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                role,
                "-show_packets",
                "-show_entries",
                "packet=data_hash",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .arg(file)
            .output()
            .unwrap(),
    ))
    .unwrap();
    value["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["data_hash"].as_str().unwrap().into())
        .collect()
}
fn assert_packet_subset(ffprobe: &Path, source: &Path, output: &Path, role: &str) {
    let original = packet_hashes(ffprobe, source, role);
    let clipped = packet_hashes(ffprobe, output, role);
    assert!(!clipped.is_empty() && clipped.len() < original.len());
    assert!(
        original
            .windows(clipped.len())
            .any(|window| window == clipped),
        "clip changed encoded content"
    );
}

struct RedirectFixture {
    origin: String,
    origin_hits: Arc<AtomicUsize>,
    target_hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl RedirectFixture {
    fn start() -> Self {
        let origin = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = format!("http://{}/start.mp4", origin.local_addr().unwrap());
        let location = format!("http://{}/trap.mp4", target.local_addr().unwrap());
        origin.set_nonblocking(true).unwrap();
        target.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let origin_hits = Arc::new(AtomicUsize::new(0));
        let target_hits = Arc::new(AtomicUsize::new(0));
        let (done, origins, targets) = (stop.clone(), origin_hits.clone(), target_hits.clone());
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(25);
            while !done.load(Ordering::Acquire) && Instant::now() < deadline {
                for (listener, first) in [(&origin, true), (&target, false)] {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                        Err(e) => panic!("fixture accept: {e}"),
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_millis(250)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_millis(250)))
                        .unwrap();
                    let mut headers = vec![];
                    while headers.len() < 16_384
                        && !done.load(Ordering::Acquire)
                        && Instant::now() < deadline
                    {
                        let mut bytes = [0; 1024];
                        match stream.read(&mut bytes) {
                            Ok(0) => break,
                            Ok(count) => {
                                headers.extend_from_slice(&bytes[..count]);
                                if headers.windows(4).any(|b| b == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                continue
                            }
                            Err(e) => panic!("fixture read: {e}"),
                        }
                    }
                    assert!(headers.starts_with(b"GET /"));
                    if first {
                        origins.fetch_add(1, Ordering::AcqRel);
                        let _ = stream.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
                    } else {
                        targets.fetch_add(1, Ordering::AcqRel);
                        let _ = stream.write_all(b"HTTP/1.1 500 Controlled Trap\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        Self {
            origin: endpoint,
            origin_hits,
            target_hits,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for RedirectFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
struct RedirectProcess {
    origin: String,
    allow_control: bool,
}
impl ProcessSupervisor for RedirectProcess {
    fn run(
        &self,
        mut request: ProcessRequest,
        cancel: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        assert!(request
            .program
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("yt-dlp"));
        let mut value = transfer_info(&request);
        value["formats"][0]["url"] = json!(self.origin);
        value["formats"][0]["protocol"] = json!("http");
        let local = request.working_dir.join("controlled-info.json");
        fs::write(&local, serde_json::to_vec(&value).unwrap()).unwrap();
        let info = request
            .args
            .iter()
            .position(|a| a == "--load-info-json")
            .unwrap();
        request.args[info + 1] = local.into_os_string();
        let index = request
            .args
            .iter()
            .position(|a| a == "--downloader-args")
            .unwrap()
            + 1;
        let production = request.args[index].to_str().unwrap();
        assert!(production.starts_with("ffmpeg_i:-max_redirects 0 -protocol_whitelist https,http,tcp,tls,httpproxy -format_whitelist mov,matroska,webm,flac,aac,mp3,ogg -rw_timeout 8000000 -http_proxy '"));
        // Test-only local HTTP endpoint. No production URL/protocol policy changes.
        // Allow-one control must reach the trap, proving zero hits is meaningful.
        let fixed = production.split(" -http_proxy ").next().unwrap();
        let fixed = if self.allow_control {
            fixed.replacen("-max_redirects 0", "-max_redirects 1", 1)
        } else {
            fixed.to_owned()
        };
        request.args[index] = format!("{fixed} -http_proxy ''").into();
        request
            .args
            .extend([OsString::from("--proxy"), OsString::from("")]);
        request.timeout = Some(Duration::from_secs(20));
        WindowsProcessSupervisor.run(request, cancel, sink)
    }
}

#[test]
#[ignore = "requires exact pinned tools; real local 302 guard/control, no external Bilibili request"]
fn real_bilibili_progressive_redirect_is_blocked() {
    let harness = BilibiliRealHarness::new(false);
    for allow_control in [false, true] {
        let fixture = RedirectFixture::start();
        let process = RedirectProcess {
            origin: fixture.origin.clone(),
            allow_control,
        };
        let result = harness.execute(best(), &process, if allow_control { "92" } else { "91" });
        assert!(result.is_err(), "302 / control trap must not produce media");
        assert!(fixture.origin_hits.load(Ordering::Acquire) > 0);
        assert_eq!(
            fixture.target_hits.load(Ordering::Acquire) > 0,
            allow_control
        );
    }
    let mut proxy = ConnectTrap::start();
    assert!(harness
        .execute(best(), &ConnectProcess(proxy.endpoint.clone()), "93")
        .is_err());
    assert!(
        proxy
            .finish()
            .starts_with("CONNECT fixture.bilivideo.com:443 HTTP/1.1\r\n"),
        "real FFmpeg must use the explicit non-forwarding CONNECT proxy"
    );
}

struct ConnectTrap {
    endpoint: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<String>>,
}
impl ConnectTrap {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done.load(Ordering::Acquire) && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("CONNECT fixture accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(250)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(250)))
                    .unwrap();
                let mut headers = vec![];
                while headers.len() < 16384
                    && !done.load(Ordering::Acquire)
                    && Instant::now() < deadline
                {
                    let mut bytes = [0; 1024];
                    match stream.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(count) => {
                            headers.extend_from_slice(&bytes[..count]);
                            if headers.windows(4).any(|b| b == b"\r\n\r\n") {
                                let _ = stream.write_all(b"HTTP/1.1 502 Controlled Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                return String::from_utf8(headers).unwrap();
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            continue
                        }
                        Err(error) => panic!("CONNECT fixture read: {error}"),
                    }
                }
                break;
            }
            String::new()
        });
        Self {
            endpoint,
            stop,
            worker: Some(worker),
        }
    }
    fn finish(&mut self) -> String {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap()
    }
}
impl Drop for ConnectTrap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct ConnectProcess(String);
impl ProcessSupervisor for ConnectProcess {
    fn run(
        &self,
        mut request: ProcessRequest,
        cancel: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let _ = transfer_info(&request);
        let index = request
            .args
            .iter()
            .position(|arg| arg == "--downloader-args")
            .unwrap()
            + 1;
        let original = request.args[index].to_str().unwrap();
        assert!(
            original.contains("-max_redirects 0 -protocol_whitelist https,http,tcp,tls,httpproxy")
        );
        let fixed = original.split(" -http_proxy ").next().unwrap();
        request.args[index] = format!("{fixed} -http_proxy '{}'", self.0).into();
        request
            .args
            .extend([OsString::from("--proxy"), self.0.clone().into()]);
        request.timeout = Some(Duration::from_secs(20));
        WindowsProcessSupervisor.run(request, cancel, sink)
    }
}

#[test]
#[ignore = "requires exact pinned tools; generated original SDR/HDR streams, real copy/clip/compatibility/verification, no network"]
fn real_bilibili_copy_preserves_source_facts_and_content() {
    for (hdr, combined) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut harness = BilibiliRealHarness::new(hdr);
        if combined {
            harness.use_combined_source();
        }
        let mut cases = vec![best()];
        for mode in [
            SelectionMode::Manual,
            SelectionMode::VideoOnly,
            SelectionMode::AudioOnly,
        ] {
            let mut selected = best();
            selected.mode = mode;
            if mode != SelectionMode::AudioOnly {
                selected.video_format_id = Some("100026".into());
                selected.output_container =
                    Some(local_media_downloader_lib::domain::output_policy::VideoContainer::Mp4);
            }
            cases.push(selected);
        }
        let mut clipped = best();
        clipped.clip = Some(ClipRange {
            start_ms: 0,
            end_ms: 1000,
        });
        cases.push(clipped);
        let mut compatible = best();
        compatible.compatibility_mode = true;
        cases.push(compatible);
        for (index, intent) in cases.into_iter().enumerate() {
            let verified = harness.download_and_verify(intent.clone(), &format!("{}", 100 + index));
            assert!(verified.duration_ms > 0);
            let after = real_media::probe(&harness.ffprobe, &verified.path);
            let streams = after["streams"].as_array().unwrap();
            let videos: Vec<_> = streams
                .iter()
                .filter(|s| s["codec_type"] == "video")
                .collect();
            let audios: Vec<_> = streams
                .iter()
                .filter(|s| s["codec_type"] == "audio")
                .collect();
            assert_eq!(
                videos.len(),
                usize::from(intent.mode != SelectionMode::AudioOnly)
            );
            assert_eq!(
                audios.len(),
                usize::from(intent.mode != SelectionMode::VideoOnly)
            );
            if intent.compatibility_mode {
                assert_eq!(videos[0]["codec_name"], "h264");
                assert_eq!(audios[0]["codec_name"], "aac");
                assert_eq!(verified.path.extension().unwrap(), "mp4");
            } else {
                if !videos.is_empty() {
                    for key in [
                        "codec_name",
                        "profile",
                        "width",
                        "height",
                        "avg_frame_rate",
                        "pix_fmt",
                        "color_range",
                        "color_space",
                        "color_transfer",
                        "color_primaries",
                    ] {
                        assert_eq!(
                            videos[0][key], harness.video_probe["streams"][0][key],
                            "original {key}, HDR={hdr}"
                        );
                    }
                    if intent.clip.is_none() {
                        real_media::assert_copy_equivalent(
                            &harness.ffmpeg,
                            &harness.video,
                            &verified.path,
                            real_media::StreamRole::Video,
                        );
                    } else {
                        assert_packet_subset(
                            &harness.ffprobe,
                            &harness.video,
                            &verified.path,
                            "v:0",
                        );
                    }
                    if hdr {
                        assert_eq!(
                            real_media::probe_video_frame(&harness.ffprobe, &verified.path)
                                ["side_data_list"],
                            real_media::probe_video_frame(&harness.ffprobe, &harness.video)
                                ["side_data_list"]
                        );
                    }
                }
                if !audios.is_empty() {
                    if intent.clip.is_none() {
                        real_media::assert_copy_equivalent(
                            &harness.ffmpeg,
                            &harness.audio,
                            &verified.path,
                            real_media::StreamRole::Audio,
                        );
                    } else {
                        assert_packet_subset(
                            &harness.ffprobe,
                            &harness.audio,
                            &verified.path,
                            "a:0",
                        );
                    }
                }
                if intent.clip.is_none() {
                    assert!((verified.duration_ms as i64 - 3000).abs() <= 200);
                }
            }
        }
    }
}
