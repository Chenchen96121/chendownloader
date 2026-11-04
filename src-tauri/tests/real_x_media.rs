#[path = "support/real_media.rs"]
mod real_media;
use local_media_downloader_lib::{
    application::{
        download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        output_verifier::{OutputVerifier, VerifiedOutput},
        portable_package::EMBEDDED_TOOL_MANIFEST,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
        x_download_job::{XImageDownloader, XOutputPublisher, XPublicationRequest, XVerifiedAsset},
        x_source::{
            XAttachmentIdentity, XImageCandidate, XInventory, XRequestContext, XSourceItem,
            XVideoCandidate,
        },
        x_video::XVideoAnalyzer,
    },
    domain::{
        download::DownloadProgress,
        managed_tools::{ManagedToolError, ManagedToolManifest, ToolName, BUILD_TARGET},
        media::AnalysisErrorCode,
        output_policy::StreamProcessing,
        x::{XItemAvailability, XMediaItem, XMediaKind},
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        output::x::FilesystemXOutputPublisher,
        process::WindowsProcessSupervisor,
        x::assets::{XAssetAdapter, XAssetTransport},
        ytdlp::{download::YtDlpDownloadExecutor, x::YtDlpXVideoAnalyzer},
    },
};
use serde_json::json;
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
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
                .find(|t| t.name == name)
                .unwrap()
                .version
                .clone(),
            path: self.0.join(name.artifact_name(BUILD_TARGET)),
        })
    }
}
fn context() -> XRequestContext {
    XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}
fn inventory(ordinal: u16, kind: XMediaKind) -> XInventory {
    let identity = XAttachmentIdentity {
        post_id: "12345678901234567890".into(),
        media_id: ordinal.to_string(),
        media_key: None,
    };
    let image = kind == XMediaKind::Image;
    XInventory {
        source: validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap(),
        title: "Controlled local X media".into(),
        creator: Some("本地作者".into()),
        items: vec![XSourceItem {
            identity: Some(identity),
            ordinal,
            kind,
            image_candidates: if image {
                vec![XImageCandidate {
                    url: "https://pbs.twimg.com/media/fixture?format=jpg&name=orig"
                        .parse()
                        .unwrap(),
                    width: Some(128),
                    height: Some(96),
                }]
            } else {
                vec![]
            },
            video_candidates: if image {
                vec![]
            } else {
                vec![XVideoCandidate {
                    url: format!(
                        "https://video.twimg.com/ext_tw_video/fixture{ordinal}/stream.mp4"
                    )
                    .parse()
                    .unwrap(),
                    content_type: "video/mp4".into(),
                    bitrate: None,
                }]
            },
            preview: None,
            width: Some(if image { 128 } else { 160 }),
            height: Some(if image { 96 } else { 90 }),
            duration_seconds: if image { None } else { Some(3.0) },
            availability: XItemAvailability::Ready,
            unavailable_reason: None,
        }],
    }
}
fn metadata(inventory: &XInventory) -> Vec<u8> {
    let item = &inventory.items[0];
    let id = item.identity.as_ref().unwrap();
    serde_json::to_vec(&json!({"id":id.media_id,"display_id":id.post_id,"title":inventory.title,"duration":item.duration_seconds,
        "formats":[{"format_id":"0","ext":"mp4","protocol":"https","vcodec":null,"acodec":null,"width":160,"height":90,"url":item.video_candidates[0].url.as_str()}]})).unwrap()
}
struct LocalOnly {
    fixture: PathBuf,
    metadata: Vec<u8>,
}
impl ProcessSupervisor for LocalOnly {
    fn run(
        &self,
        mut request: ProcessRequest,
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
            if request.args.iter().any(|a| a == "--simulate") {
                return Ok(ProcessExit {
                    code: 0,
                    stdout: self.metadata.clone(),
                    stderr: vec![],
                });
            }
            let info = request
                .args
                .iter()
                .position(|a| a == "--load-info-json")
                .expect("native minimal info JSON required");
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(Path::new(&request.args[info + 1])).unwrap())
                    .unwrap();
            assert_eq!(value["formats"][0]["format_id"], "0");
            assert!(value.get("webpage_url").is_none() && value.get("http_headers").is_none());
            assert!(!request
                .args
                .iter()
                .any(|a| a.to_string_lossy().contains("https://x.com/i/web/status/")));
            fs::copy(&self.fixture, request.working_dir.join("source.mp4")).unwrap();
            return Ok(ProcessExit {
                code: 0,
                stdout: vec![],
                stderr: vec![],
            });
        }
        assert!(request
            .program
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ffprobe"));
        // Local-media proof only. The separate CONNECT test below preserves all
        // production network/protocol flags instead of this local substitution.
        let mut args = vec![];
        let mut n = 0;
        while n < request.args.len() {
            let option = request.args[n].to_string_lossy();
            if matches!(
                option.as_ref(),
                "-http_proxy"
                    | "-user_agent"
                    | "-referer"
                    | "-max_redirects"
                    | "-rw_timeout"
                    | "-protocol_whitelist"
            ) {
                n += 2;
                continue;
            }
            if option == "-i" {
                args.extend([OsString::from("-i"), self.fixture.as_os_str().to_owned()]);
                n += 2;
                continue;
            }
            args.push(request.args[n].clone());
            n += 1;
        }
        request.args = args;
        WindowsProcessSupervisor.run(request, cancel, sink)
    }
}
struct ImageBytes(Vec<u8>);
impl XAssetTransport for ImageBytes {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        maximum: u64,
        _: &XRequestContext,
    ) -> Result<(), AnalysisErrorCode> {
        assert!(url
            .query_pairs()
            .any(|(key, value)| key == "name" && value == "orig"));
        assert!(self.0.len() as u64 <= maximum);
        fs::write(destination, &self.0).unwrap();
        Ok(())
    }
}
struct Sink;

/// Both listeners stay local; the target is an observable redirect trap, not a
/// media server. Closing this fixture always joins its finite worker.
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
        let url = format!("http://{}/start.mp4", origin.local_addr().unwrap());
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
                for (listener, is_origin) in [(&origin, true), (&target, false)] {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                        Err(error) => panic!("redirect fixture accept: {error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_millis(250)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_millis(250)))
                        .unwrap();
                    let mut headers = vec![];
                    while headers.len() < 16_384
                        && Instant::now() < deadline
                        && !done.load(Ordering::Acquire)
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
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                continue
                            }
                            Err(error) => panic!("redirect fixture read: {error}"),
                        }
                    }
                    assert!(headers.starts_with(b"GET /"));
                    if is_origin {
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
            origin: url,
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

struct LocalRedirectProcess {
    origin: String,
    allow_redirect_control: bool,
    stderr: Mutex<Vec<u8>>,
}
impl ProcessSupervisor for LocalRedirectProcess {
    fn run(
        &self,
        mut request: ProcessRequest,
        cancel: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        assert!(request.program.path().to_string_lossy().contains("yt-dlp"));
        assert!(request
            .args
            .windows(2)
            .any(|p| p[0] == "--downloader" && p[1] == "ffmpeg"));
        let index = request
            .args
            .iter()
            .position(|a| a == "--load-info-json")
            .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(Path::new(&request.args[index + 1])).unwrap())
                .unwrap();
        assert!(value.get("webpage_url").is_none() && value.get("http_headers").is_none());
        assert!(value["formats"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://video.twimg.com/"));
        value["formats"][0]["url"] = json!(self.origin);
        value["formats"][0]["protocol"] = json!("http");
        let local_info = request.working_dir.join("controlled-local-info.json");
        fs::write(&local_info, serde_json::to_vec(&value).unwrap()).unwrap();
        request.args[index + 1] = local_info.into_os_string();
        let guards = request
            .args
            .iter()
            .position(|a| a == "--downloader-args")
            .unwrap()
            + 1;
        let production = request.args[guards].to_str().unwrap();
        assert!(production.starts_with("ffmpeg_i:-max_redirects 0 -protocol_whitelist https,http,tcp,tls,httpproxy -format_whitelist mov,matroska,webm -rw_timeout 8000000 -http_proxy '"));
        // Only the transport endpoint/proxy is substituted. The control run
        // deliberately allows one redirect to prove the trap can observe it.
        let fixed = production.split(" -http_proxy ").next().unwrap();
        let fixed = if self.allow_redirect_control {
            fixed.replacen("-max_redirects 0", "-max_redirects 1", 1)
        } else {
            fixed.into()
        };
        request.args[guards] = format!("{fixed} -http_proxy ''").into();
        request
            .args
            .extend([OsString::from("--proxy"), OsString::from("")]);
        request.timeout = Some(Duration::from_secs(20));
        let result = WindowsProcessSupervisor.run(request, cancel, sink);
        if let Ok(exit) = &result {
            *self.stderr.lock().unwrap() = exit.stderr.clone();
        }
        result
    }
}

#[test]
#[ignore = "requires pinned real yt-dlp + FFmpeg; local 302 guard/control, no X or external network"]
fn real_x_progressive_transfer_blocks_redirect_before_target_request() {
    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let root = tempfile::tempdir().unwrap();
    let resolver = Resolver(tools);
    let inventory = inventory(2, XMediaKind::Video);
    let mut raw: serde_json::Value = serde_json::from_slice(&metadata(&inventory)).unwrap();
    let row = &mut raw["formats"][0];
    row["vcodec"] = json!("h264");
    row["acodec"] = json!("aac");
    row["fps"] = json!(30);
    for allow in [false, true] {
        let fixture = RedirectFixture::start();
        let process = LocalRedirectProcess {
            origin: fixture.origin.clone(),
            allow_redirect_control: allow,
            stderr: Mutex::new(vec![]),
        };
        let resolved = local_media_downloader_lib::application::x_video::resolve_x_video(
            local_media_downloader_lib::application::x_video::match_x_video(
                &inventory,
                &inventory.items[0],
                &raw,
            )
            .unwrap(),
            Arc::new(NetworkProfile::current()),
            false,
        )
        .unwrap();
        let (media, plan, source) = resolved.into_parts();
        let job = if allow { "72" } else { "71" };
        let stage = root.path().join(job);
        fs::create_dir(&stage).unwrap();
        let result = YtDlpDownloadExecutor::new(
            &resolver,
            &process,
            FfmpegMediaProcessor::new(&resolver, WindowsProcessSupervisor),
        )
        .download(
            DownloadExecutionRequest {
                job_id: job.into(),
                source: inventory.source.clone(),
                playlist_item: None,
                instagram_selector: None,
                tiktok_source: None,
                x_source: Some(source),
                bilibili_source: None,
                video_source: Some(media.formats[0].clone()),
                audio_source: Some(media.formats[0].clone()),
                plan,
                staging_dir: stage.clone(),
            },
            CancellationToken::new(),
            &Sink,
        );
        let stderr = String::from_utf8_lossy(&process.stderr.lock().unwrap()).into_owned();
        assert!(result.is_err(), "302 / control 500 must not publish media");
        assert!(
            fixture.origin_hits.load(Ordering::Acquire) > 0,
            "real downloader did not reach local origin: {stderr}"
        );
        assert_eq!(
            fixture.target_hits.load(Ordering::Acquire) > 0,
            allow,
            "redirect guard / control: {stderr}"
        );
        assert!(!stage.join("stage.mp4").exists());
    }
}

/// This proxy never opens an upstream connection. Its controlled response lets
/// the real FFprobe prove that production HTTPS transport options reach CONNECT.
struct ConnectProxyFixture {
    endpoint: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Result<String, String>>>,
}

impl ConnectProxyFixture {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => return Err(format!("accept: {error}")),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(250)))
                    .map_err(|error| format!("read timeout: {error}"))?;
                stream
                    .set_write_timeout(Some(Duration::from_millis(250)))
                    .map_err(|error| format!("write timeout: {error}"))?;
                let mut request = Vec::new();
                while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                    let mut buffer = [0_u8; 1024];
                    match stream.read(&mut buffer) {
                        Ok(0) => return Err("proxy client closed before CONNECT headers".into()),
                        Ok(count) => {
                            request.extend_from_slice(&buffer[..count]);
                            if request.len() > 16_384 {
                                return Err("CONNECT headers exceeded fixture limit".into());
                            }
                            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                                stream
                                    .write_all(b"HTTP/1.1 502 Controlled Fixture Response\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                                    .map_err(|error| format!("controlled response: {error}"))?;
                                return String::from_utf8(request)
                                    .map_err(|error| format!("CONNECT encoding: {error}"));
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(error) => return Err(format!("CONNECT read: {error}")),
                    }
                }
                return Err("CONNECT headers were not completed before cancellation".into());
            }
            Ok(String::new())
        });
        Self {
            endpoint,
            stop,
            worker: Some(worker),
        }
    }

    fn finish(&mut self) -> String {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap().unwrap()
    }
}

impl Drop for ConnectProxyFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl DownloadProgressSink for Sink {
    fn on_progress(&self, p: DownloadProgress) {
        assert!(!p.job_id.is_empty());
    }
}
fn public(ordinal: u16, kind: XMediaKind) -> XMediaItem {
    serde_json::from_value(json!({"itemId":ordinal.to_string(),"ordinal":ordinal,"kind":kind,"container":if kind==XMediaKind::Image{"jpg"}else{"mp4"},"previewAvailable":false,"availability":"ready"})).unwrap()
}

struct ProxyOnly {
    endpoint: String,
    metadata: Vec<u8>,
    stderr: Mutex<Vec<u8>>,
}
impl ProcessSupervisor for ProxyOnly {
    fn run(
        &self,
        mut request: ProcessRequest,
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
            assert!(request.args.iter().any(|arg| arg == "--simulate"));
            return Ok(ProcessExit {
                code: 0,
                stdout: self.metadata.clone(),
                stderr: vec![],
            });
        }
        assert!(request
            .program
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ffprobe"));
        for (option, value) in [
            ("-protocol_whitelist", "https,http,tcp,tls,httpproxy"),
            ("-format_whitelist", "mov,matroska,webm"),
            ("-max_redirects", "0"),
            ("-rw_timeout", "8000000"),
            ("-referer", "https://x.com/"),
        ] {
            assert!(
                request
                    .args
                    .windows(2)
                    .any(|pair| pair[0] == option && pair[1] == value),
                "missing production guard {option}"
            );
        }
        if let Some(index) = request.args.iter().position(|a| a == "-http_proxy") {
            request.args[index + 1] = self.endpoint.clone().into();
        } else {
            request.args.splice(
                0..0,
                [OsString::from("-http_proxy"), self.endpoint.clone().into()],
            );
        }
        let result = WindowsProcessSupervisor.run(request, cancel, sink);
        if let Ok(exit) = &result {
            *self.stderr.lock().unwrap() = exit.stderr.clone();
        }
        result
    }
}

#[test]
#[ignore = "requires exact pinned tools; real FFprobe production HTTPS options through a local non-forwarding CONNECT fixture, no X request"]
fn real_x_ffprobe_reaches_http_connect_proxy_with_production_options() {
    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let inventory = inventory(2, XMediaKind::Video);
    let identity = inventory.items[0].identity.as_ref().unwrap();
    let mut proxy = ConnectProxyFixture::start();
    let process = ProxyOnly {
        endpoint: proxy.endpoint.clone(),
        metadata: metadata(&inventory),
        stderr: Mutex::new(vec![]),
    };
    let result = YtDlpXVideoAnalyzer::new(Resolver(tools), &process).analyze_item(
        &inventory,
        identity,
        false,
        &context(),
    );
    let captured = proxy.finish();
    let stderr = String::from_utf8_lossy(&process.stderr.lock().unwrap()).into_owned();
    assert!(
        captured.starts_with("CONNECT video.twimg.com:443 HTTP/1.1\r\n"),
        "production FFprobe did not reach CONNECT: {stderr}"
    );
    assert!(result.is_err(), "controlled 502 must stop analysis");
    assert!(
        (stderr.contains("502") || stderr.to_ascii_lowercase().contains("5xx"))
            && !stderr.contains("not on whitelist"),
        "expected controlled 502, not local protocol rejection: {stderr}"
    );
}
#[test]
#[ignore = "requires exact pinned tools; real High H.264/AAC container-only color range survives production MP4 copy and strict verification, no network"]
fn real_x_stream_copy_preserves_container_only_color_range() {
    fn encoded_hashes(ffprobe: &Path, path: &Path) -> Vec<(String, String, Vec<String>)> {
        let bytes = real_media::checked(
            Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-show_packets",
                    "-show_streams",
                    "-show_entries",
                    "packet=stream_index,data_hash:stream=index,codec_type,extradata_hash",
                    "-show_data_hash",
                    "sha256",
                    "-of",
                    "json",
                ])
                .arg(path)
                .output()
                .unwrap(),
        );
        let probe: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        probe["streams"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stream| {
                let packets = probe["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|packet| packet["stream_index"] == stream["index"])
                    .map(|packet| {
                        packet["data_hash"]
                            .as_str()
                            .expect("packet SHA256")
                            .to_owned()
                    })
                    .collect::<Vec<_>>();
                assert!(!packets.is_empty());
                (
                    stream["codec_type"].as_str().unwrap().to_owned(),
                    stream["extradata_hash"]
                        .as_str()
                        .expect("codec extradata SHA256")
                        .to_owned(),
                    packets,
                )
            })
            .collect()
    }

    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let resolver = Resolver(tools.clone());
    let ffmpeg = tools.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET));
    let ffprobe = tools.join(ToolName::Ffprobe.artifact_name(BUILD_TARGET));
    let root = tempfile::tempdir().unwrap();
    let baseline = root.path().join("baseline.mp4");
    real_media::checked(
        Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
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
                "-map",
                "1:a:0",
                "-threads",
                "1",
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-profile:v",
                "high",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
            ])
            .arg(&baseline)
            .output()
            .unwrap(),
    );
    // The encoded H.264 declares no color facts. Copying it into Matroska
    // attaches only a container range tag, matching the failing X intermediate.
    let fixture = root.path().join("container-range.mkv");
    real_media::checked(
        Command::new(&ffmpeg)
            .args(["-v", "error", "-nostdin", "-n", "-i"])
            .arg(&baseline)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0",
                "-c",
                "copy",
                "-color_range",
                "tv",
            ])
            .arg(&fixture)
            .output()
            .unwrap(),
    );
    let baseline_probe = real_media::probe(&ffprobe, &baseline);
    let source_probe = real_media::probe(&ffprobe, &fixture);
    let source_video = &source_probe["streams"][0];
    assert_eq!(source_video["codec_type"], "video");
    assert_eq!(source_video["codec_name"], "h264");
    assert_eq!(source_video["profile"], "High");
    assert!(baseline_probe["streams"][0]["color_range"].is_null());
    assert_eq!(source_video["color_range"], "tv");
    for key in ["color_space", "color_transfer", "color_primaries"] {
        assert!(
            baseline_probe["streams"][0][key].is_null(),
            "baseline {key}"
        );
        assert!(source_video[key].is_null(), "source must not invent {key}");
    }
    let expected_hashes = encoded_hashes(&ffprobe, &baseline);
    assert_eq!(encoded_hashes(&ffprobe, &fixture), expected_hashes);
    // A real default MP4 copy loses the container-only range despite keeping
    // every encoded packet and codec header, proving this fixture catches it.
    let control = root.path().join("default-copy.mp4");
    real_media::checked(
        Command::new(&ffmpeg)
            .args(["-v", "error", "-nostdin", "-n", "-i"])
            .arg(&fixture)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0",
                "-c",
                "copy",
                "-movflags",
                "+faststart",
            ])
            .arg(&control)
            .output()
            .unwrap(),
    );
    let control_probe = real_media::probe(&ffprobe, &control);
    assert!(
        control_probe["streams"][0]["color_range"].is_null(),
        "default copy must expose the original loss"
    );
    assert_eq!(encoded_hashes(&ffprobe, &control), expected_hashes);
    let inventory = inventory(2, XMediaKind::Video);
    let process = LocalOnly {
        fixture: fixture.clone(),
        metadata: metadata(&inventory),
    };
    let resolved = YtDlpXVideoAnalyzer::new(&resolver, &process)
        .analyze_item(
            &inventory,
            inventory.items[0].identity.as_ref().unwrap(),
            false,
            &context(),
        )
        .unwrap();
    let (media, plan, source) = resolved.into_parts();
    assert_eq!(plan.video_processing, Some(StreamProcessing::Copy));
    assert_eq!(plan.audio_processing, Some(StreamProcessing::Copy));
    let stage = root.path().join("84");
    fs::create_dir(&stage).unwrap();
    let staged = YtDlpDownloadExecutor::new(
        &resolver,
        &process,
        FfmpegMediaProcessor::new(&resolver, WindowsProcessSupervisor),
    )
    .download(
        DownloadExecutionRequest {
            job_id: "84".into(),
            source: inventory.source.clone(),
            playlist_item: None,
            instagram_selector: None,
            tiktok_source: None,
            x_source: Some(source),
            bilibili_source: None,
            video_source: media
                .formats
                .iter()
                .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
                .cloned(),
            audio_source: media
                .formats
                .iter()
                .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
                .cloned(),
            plan: plan.clone(),
            staging_dir: stage.clone(),
        },
        CancellationToken::new(),
        &Sink,
    )
    .unwrap();
    assert!(!stage.join("source").exists());
    let after = real_media::probe(&ffprobe, &stage.join("stage.mp4"));
    let verified = FfprobeOutputVerifier::new(&resolver, WindowsProcessSupervisor)
        .verify(staged, &plan, CancellationToken::new())
        .unwrap_or_else(|error| {
            panic!(
                "strict copy verification failed: {error:?}; source={source_video}; output={}",
                after["streams"][0]
            )
        });
    assert!(verified.has_video && verified.has_audio);
    let output_video = &after["streams"][0];
    assert_eq!(output_video["color_range"], "tv");
    for key in ["color_space", "color_transfer", "color_primaries"] {
        assert!(output_video[key].is_null(), "output must not invent {key}");
    }
    for key in ["codec_name", "profile", "width", "height", "pix_fmt"] {
        assert_eq!(output_video[key], source_video[key], "copied video {key}");
    }
    assert_eq!(encoded_hashes(&ffprobe, &verified.path), expected_hashes);
    real_media::assert_copy_equivalent(
        &ffmpeg,
        &baseline,
        &verified.path,
        real_media::StreamRole::Video,
    );
    real_media::assert_copy_equivalent(
        &ffmpeg,
        &baseline,
        &verified.path,
        real_media::StreamRole::Audio,
    );
}

#[test]
#[ignore = "requires exact pinned tools; generated real fMP4 HLS, file-only FFprobe/FFmpeg analysis and full stream-copy download, no network"]
fn real_x_fmp4_hls_probe_and_download_use_materialized_local_files() {
    use local_media_downloader_lib::infrastructure::x::{
        hls_local::{HlsByteRange, HlsSegmentTransport, HlsTransferError},
        manifest::HlsManifestTransport,
    };
    use std::collections::HashMap;

    fn frame_rate(stream: &serde_json::Value) -> f64 {
        let raw = stream["avg_frame_rate"].as_str().expect("video frame rate");
        let (numerator, denominator) = raw.split_once('/').expect("rational frame rate");
        let rate = numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap();
        assert!(
            rate.is_finite() && rate > 0.0,
            "valid video frame rate: {raw}"
        );
        rate
    }

    // Only remote extraction/transport is substituted. All media bytes below
    // come from the pinned FFmpeg, and media processes run without alteration.
    struct GeneratedHls {
        document: Vec<u8>,
        assets: HashMap<String, Vec<u8>>,
        network: Arc<NetworkProfile>,
        destinations: Mutex<Vec<PathBuf>>,
    }
    impl HlsManifestTransport for &GeneratedHls {
        fn read(
            &self,
            url: &url::Url,
            context: &XRequestContext,
        ) -> Result<Vec<u8>, AnalysisErrorCode> {
            assert_eq!(url.as_str(), "https://video.twimg.com/fixture/leaf.m3u8");
            assert!(Arc::ptr_eq(&context.network, &self.network));
            Ok(self.document.clone())
        }
    }
    impl HlsSegmentTransport for &GeneratedHls {
        fn fetch_probe(
            &self,
            url: &url::Url,
            range: Option<HlsByteRange>,
            destination: &Path,
            maximum: u64,
            context: &XRequestContext,
        ) -> Result<u64, HlsTransferError> {
            assert!(Arc::ptr_eq(&context.network, &self.network));
            assert!(!context.cancellation.is_canceled());
            assert_eq!(range, None);
            assert_ne!(url.path(), "/fixture/init.mp4", "MAP must be complete");
            let bytes = self.assets.get(url.path()).unwrap();
            let count = bytes.len().min(1_048_576);
            assert!(count as u64 <= maximum);
            fs::write(destination, &bytes[..count]).unwrap();
            self.destinations.lock().unwrap().push(destination.into());
            Ok(count as u64)
        }
        fn fetch(
            &self,
            url: &url::Url,
            range: Option<HlsByteRange>,
            destination: &Path,
            maximum: u64,
            context: &XRequestContext,
        ) -> Result<u64, HlsTransferError> {
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("video.twimg.com"));
            assert!(Arc::ptr_eq(&context.network, &self.network));
            assert!(!context.cancellation.is_canceled());
            assert_eq!(range, None, "fixture uses separate init/fragment files");
            let bytes = self.assets.get(url.path()).expect("generated HLS asset");
            assert!(bytes.len() as u64 <= maximum);
            fs::write(destination, bytes).unwrap();
            self.destinations.lock().unwrap().push(destination.into());
            Ok(bytes.len() as u64)
        }
    }
    struct HlsInvocation {
        probe: bool,
        input: PathBuf,
        segments: usize,
    }
    struct RealMediaProcess {
        metadata: Vec<u8>,
        init: Vec<u8>,
        segments: Vec<Vec<u8>>,
        hls: Mutex<Vec<HlsInvocation>>,
        extraction_directory: Mutex<Option<PathBuf>>,
    }
    impl ProcessSupervisor for RealMediaProcess {
        fn run(
            &self,
            request: ProcessRequest,
            cancel: CancellationToken,
            sink: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            let name = request
                .program
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy();
            if name.starts_with("yt-dlp") {
                assert!(request.args.iter().any(|arg| arg == "--simulate"));
                *self.extraction_directory.lock().unwrap() = Some(request.working_dir);
                return Ok(ProcessExit {
                    code: 0,
                    stdout: self.metadata.clone(),
                    stderr: vec![],
                });
            }
            let probe = name.starts_with("ffprobe");
            assert!(probe || name.starts_with("ffmpeg"));
            assert!(
                request.args.iter().all(|arg| {
                    let arg = arg.to_string_lossy();
                    !arg.contains("https://") && !arg.contains("http://")
                }),
                "real media tools must receive only local input"
            );
            for pair in request.args.windows(2).filter(|pair| pair[0] == "-i") {
                let input = PathBuf::from(&pair[1]);
                let input = if input.is_absolute() {
                    input
                } else {
                    request.working_dir.join(input)
                };
                assert!(input.is_file(), "local input missing: {}", input.display());
                if input.extension().and_then(|ext| ext.to_str()) != Some("m3u8") {
                    continue;
                }
                assert!(request
                    .args
                    .windows(2)
                    .any(|pair| { pair[0] == "-protocol_whitelist" && pair[1] == "file" }));
                assert!(!request.args.iter().any(|arg| matches!(
                    arg.to_str(),
                    Some("-http_proxy" | "-referer" | "-user_agent" | "-max_redirects")
                )));
                if !probe {
                    assert!(request
                        .args
                        .windows(2)
                        .any(|pair| pair[0] == "-c" && pair[1] == "copy"));
                }
                let document = fs::read_to_string(&input).unwrap();
                assert!(!document.contains("http://") && !document.contains("https://"));
                let parent = input.parent().unwrap().canonicalize().unwrap();
                let local_asset = |uri: &str| {
                    let native = uri.strip_prefix("file:").expect("local asset URI");
                    #[cfg(windows)]
                    assert!(
                        native.len() >= 3
                            && native.as_bytes()[0].is_ascii_alphabetic()
                            && native.as_bytes()[1] == b':'
                            && native.as_bytes()[2] == b'/',
                        "FFmpeg requires a native drive path, not a verbatim/UNC URI: {uri}"
                    );
                    let path = Path::new(native);
                    assert!(path.is_absolute(), "local asset must be absolute: {uri}");
                    let path = path.canonicalize().unwrap();
                    assert_eq!(path.parent(), Some(parent.as_path()));
                    path
                };
                let maps = document
                    .lines()
                    .filter_map(|line| line.strip_prefix("#EXT-X-MAP:URI=\"")?.strip_suffix('"'))
                    .collect::<Vec<_>>();
                assert_eq!(
                    maps.len(),
                    1,
                    "fMP4 initialization map must survive localization"
                );
                let map = local_asset(maps[0]);
                assert_eq!(fs::read(map).unwrap(), self.init);
                let segments = document
                    .lines()
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(|line| fs::read(local_asset(line)).unwrap())
                    .collect::<Vec<_>>();
                assert!(
                    !segments.is_empty() && (probe || segments.len() >= 2),
                    "probe must retain a playable prefix and transfer must retain multiple fragments"
                );
                if probe {
                    assert!(request
                        .timeout
                        .is_some_and(|n| n <= Duration::from_secs(15)));
                    for (actual, complete) in segments.iter().zip(&self.segments) {
                        assert_eq!(actual, &complete[..complete.len().min(1_048_576)]);
                    }
                } else {
                    assert_eq!(segments, self.segments[..segments.len()]);
                }
                self.hls.lock().unwrap().push(HlsInvocation {
                    probe,
                    input,
                    segments: segments.len(),
                });
            }
            WindowsProcessSupervisor.run(request, cancel, sink)
        }
    }

    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let resolver = Resolver(tools.clone());
    let ffmpeg = tools.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET));
    let ffprobe = tools.join(ToolName::Ffprobe.artifact_name(BUILD_TARGET));
    let root = tempfile::tempdir().unwrap();
    let generated = root.path().join("generated");
    fs::create_dir(&generated).unwrap();
    let playlist = generated.join("leaf.m3u8");
    let replay_source = std::env::var_os("LMD_REAL_X_HLS_SOURCE").map(PathBuf::from);
    let mut generation = Command::new(&ffmpeg);
    generation
        .current_dir(&generated)
        .args(["-v", "error", "-nostdin", "-n"]);
    if let Some(source) = &replay_source {
        assert!(
            source.is_absolute() && source.is_file(),
            "LMD_REAL_X_HLS_SOURCE must name an existing absolute local media file"
        );
        generation
            .args(["-protocol_whitelist", "file", "-i"])
            .arg(source)
            .args(["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy"]);
    } else {
        generation.args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=30:duration=5",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000:duration=5",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-threads",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-sc_threshold",
            "0",
            "-b:v",
            "24M",
            "-minrate",
            "24M",
            "-maxrate",
            "24M",
            "-bufsize",
            "24M",
            "-x264-params",
            "nal-hrd=cbr",
            "-c:a",
            "aac",
        ]);
    }
    generation.args([
        "-f",
        "hls",
        "-hls_time",
        "1",
        "-hls_list_size",
        "0",
        "-hls_playlist_type",
        "vod",
        "-hls_segment_type",
        "fmp4",
        "-hls_fmp4_init_filename",
        "init.mp4",
        "-hls_segment_filename",
        "segment-%03d.m4s",
        "leaf.m3u8",
    ]);
    real_media::checked(generation.output().unwrap());
    let original = real_media::probe(&ffprobe, replay_source.as_deref().unwrap_or(&playlist));
    let original_streams = original["streams"].as_array().unwrap();
    let original_video = original_streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    let original_audio = original_streams
        .iter()
        .find(|stream| stream["codec_type"] == "audio");
    let expected_width = u32::try_from(original_video["width"].as_u64().unwrap()).unwrap();
    let expected_height = u32::try_from(original_video["height"].as_u64().unwrap()).unwrap();
    let expected_rate = frame_rate(original_video);
    let expected_duration = original["format"]["duration"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!(expected_duration.is_finite() && expected_duration > 0.0);
    let document = fs::read_to_string(&playlist).unwrap();
    assert!(document.contains("#EXT-X-MAP:URI=\"init.mp4\""));
    let names = document
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert!(
        names.len() >= 2,
        "HLS fixture must contain multiple real fragments"
    );
    let init = fs::read(generated.join("init.mp4")).unwrap();
    let segments = names
        .iter()
        .map(|name| fs::read(generated.join(name)).unwrap())
        .collect::<Vec<_>>();
    assert!(
        segments[0].len() > 1_048_576,
        "fixture must actually truncate a media fragment during probing"
    );
    let ctx = context();
    let mut assets = HashMap::from([("/fixture/init.mp4".into(), init.clone())]);
    for (name, bytes) in names.iter().zip(&segments) {
        assets.insert(format!("/fixture/{name}"), bytes.clone());
    }
    let transport = GeneratedHls {
        document: document.into_bytes(),
        assets,
        network: ctx.network.clone(),
        destinations: Mutex::new(vec![]),
    };
    let mut inventory = inventory(2, XMediaKind::Video);
    inventory.items[0].width = Some(expected_width);
    inventory.items[0].height = Some(expected_height);
    inventory.items[0].duration_seconds = Some(expected_duration);
    inventory.items[0].video_candidates[0].url =
        "https://video.twimg.com/fixture/leaf.m3u8".parse().unwrap();
    inventory.items[0].video_candidates[0].content_type = "application/x-mpegURL".into();
    let mut raw: serde_json::Value = serde_json::from_slice(&metadata(&inventory)).unwrap();
    raw["formats"][0]["protocol"] = json!("m3u8_native");
    raw["formats"][0].as_object_mut().unwrap().remove("width");
    raw["formats"][0].as_object_mut().unwrap().remove("height");
    let process = RealMediaProcess {
        metadata: serde_json::to_vec(&raw).unwrap(),
        init,
        segments,
        hls: Mutex::new(vec![]),
        extraction_directory: Mutex::new(None),
    };
    let resolved =
        YtDlpXVideoAnalyzer::with_transports(&resolver, &process, &transport, &transport)
            .analyze_item(
                &inventory,
                inventory.items[0].identity.as_ref().unwrap(),
                false,
                &ctx,
            )
            .unwrap();
    let (media, plan, source) = resolved.into_parts();
    let row = &media.formats[0];
    assert_eq!(media.duration_seconds, Some(expected_duration));
    assert!(
        row.file_size.is_none(),
        "sample size is not complete media size"
    );
    assert_eq!(
        (row.width, row.height),
        (Some(expected_width), Some(expected_height))
    );
    assert!(
        (row.fps.unwrap() - expected_rate).abs() < 0.05,
        "probe frame rate must match source timing"
    );
    assert_eq!(
        row.video_codec.as_deref(),
        original_video["codec_name"].as_str()
    );
    assert_eq!(
        row.audio_codec.as_deref(),
        original_audio.and_then(|stream| stream["codec_name"].as_str())
    );
    assert_eq!(plan.video_processing, Some(StreamProcessing::Copy));
    assert_eq!(
        plan.audio_processing,
        original_audio.map(|_| StreamProcessing::Copy)
    );
    let snapshot_directory = source
        .hls_input(&plan.source_format_ids[0])
        .unwrap()
        .unwrap()
        .input()
        .parent()
        .unwrap()
        .to_owned();
    assert!(snapshot_directory.is_dir());
    assert!(!process
        .extraction_directory
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .exists());
    assert!(process
        .hls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.input.parent().unwrap().exists()));
    let stage = root.path().join("83");
    fs::create_dir(&stage).unwrap();
    let staged = YtDlpDownloadExecutor::with_hls_transport(
        &resolver,
        &process,
        FfmpegMediaProcessor::new(&resolver, &process),
        &transport,
    )
    .download(
        DownloadExecutionRequest {
            job_id: "83".into(),
            source: inventory.source.clone(),
            playlist_item: None,
            instagram_selector: None,
            tiktok_source: None,
            x_source: Some(source),
            bilibili_source: None,
            video_source: media
                .formats
                .iter()
                .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
                .cloned(),
            audio_source: media
                .formats
                .iter()
                .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
                .cloned(),
            plan: plan.clone(),
            staging_dir: stage.clone(),
        },
        CancellationToken::new(),
        &Sink,
    )
    .unwrap();
    assert!(
        !stage.join("source").exists(),
        "owned source workspace must be removed"
    );
    assert!(
        !snapshot_directory.exists(),
        "sealed remote snapshot owner must be released"
    );
    let verified = FfprobeOutputVerifier::new(&resolver, &process)
        .verify(staged, &plan, CancellationToken::new())
        .unwrap();
    assert_eq!(verified.has_audio, original_audio.is_some());
    let after = real_media::probe(&ffprobe, &verified.path);
    let streams = after["streams"].as_array().unwrap();
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    let audio = streams
        .iter()
        .find(|stream| stream["codec_type"] == "audio");
    assert_eq!(streams.len(), 1 + usize::from(original_audio.is_some()));
    assert_eq!(video["codec_name"], original_video["codec_name"]);
    assert_eq!(video["width"], expected_width);
    assert_eq!(video["height"], expected_height);
    assert!(
        (frame_rate(video) - expected_rate).abs() < 0.05,
        "output frame rate must match source timing"
    );
    assert_eq!(video["pix_fmt"], original_video["pix_fmt"]);
    if let Some(original_audio) = original_audio {
        let audio = audio.unwrap();
        for key in ["codec_name", "sample_rate", "channels"] {
            assert_eq!(audio[key], original_audio[key], "audio property {key}");
        }
    } else {
        assert!(audio.is_none());
    }
    let duration = after["format"]["duration"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!(
        (duration - expected_duration).abs() < 0.15,
        "complete HLS duration: {duration}, source: {expected_duration}"
    );
    let content_reference = replay_source.as_deref().unwrap_or(&playlist);
    real_media::assert_copy_equivalent(
        &ffmpeg,
        content_reference,
        &verified.path,
        real_media::StreamRole::Video,
    );
    if original_audio.is_some() {
        real_media::assert_copy_equivalent(
            &ffmpeg,
            content_reference,
            &verified.path,
            real_media::StreamRole::Audio,
        );
    }
    let calls = process.hls.lock().unwrap();
    assert_eq!(calls.len(), 2, "real HLS probe and transfer must both run");
    assert!(calls[0].probe && !calls[1].probe);
    assert!(
        calls[0].segments > 0 && calls[0].segments < names.len(),
        "probe must remain a bounded prefix"
    );
    assert_eq!(
        calls[1].segments,
        names.len(),
        "download must include later fragments"
    );
    assert!(
        calls
            .iter()
            .all(|call| !call.input.parent().unwrap().exists()),
        "owned local HLS directories must be removed"
    );
    assert!(
        transport
            .destinations
            .lock()
            .unwrap()
            .iter()
            .all(|path| !path.exists()),
        "materialized init and fragment files must be removed"
    );
}

fn publish(asset: XVerifiedAsset, root: &Path, job: &str, item: XMediaItem) -> PathBuf {
    let publisher = FilesystemXOutputPublisher::new();
    let request = XPublicationRequest::new(
        job.into(),
        "12345678901234567890".into(),
        "Controlled local X media".into(),
        Some("本地作者".into()),
        vec![item.clone()],
    )
    .unwrap();
    let publication = publisher.prepare(root, &request).unwrap();
    let name = publisher
        .publish(asset, &publication, &item, &CancellationToken::new())
        .unwrap();
    assert!(name.starts_with(&format!("{:02}.", item.ordinal)));
    publication.output_directory().join(name)
}

#[test]
#[ignore = "requires exact pinned tools; controlled local original image/video/HDR/compatibility pipeline, no X request"]
fn real_x_original_media_pipeline() {
    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let resolver = Resolver(tools.clone());
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("下载 目录");
    fs::create_dir(&output).unwrap();
    let ffmpeg = tools.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET));
    let ffprobe = tools.join(ToolName::Ffprobe.artifact_name(BUILD_TARGET));
    let picture = root.path().join("original.jpg");
    real_media::checked(
        Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=128x96",
                "-frames:v",
                "1",
                "-threads",
                "1",
                "-c:v",
                "mjpeg",
            ])
            .arg(&picture)
            .output()
            .unwrap(),
    );
    let original = fs::read(&picture).unwrap();
    let stage = root.path().join("41/01");
    fs::create_dir_all(&stage).unwrap();
    let image_inventory = inventory(1, XMediaKind::Image);
    let asset = XAssetAdapter::with_transport(
        &resolver,
        WindowsProcessSupervisor,
        ImageBytes(original.clone()),
    )
    .download(&image_inventory.items[0], &stage, &context())
    .unwrap();
    let published = publish(asset, &output, "41", public(1, XMediaKind::Image));
    assert_eq!(fs::read(published).unwrap(), original);
    for (ordinal, audio, hdr) in [
        (2, Some("aac"), false),
        (3, None, false),
        (4, Some("aac"), true),
    ] {
        let fixture = root.path().join(format!("video-{ordinal}.mp4"));
        if hdr {
            let parameters="pools=1:frame-threads=1:log-level=error:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400";
            real_media::checked(Command::new(&ffmpeg).args(["-v","error","-nostdin","-n","-f","lavfi","-i","color=size=160x90:rate=30:duration=1,format=yuv420p10le,geq=lum='64+876*X/W':cb=512:cr=512","-f","lavfi","-i","sine=frequency=440:duration=1:sample_rate=48000","-c:v","libx265","-preset","ultrafast","-pix_fmt","yuv420p10le","-color_range","tv","-colorspace","bt2020nc","-color_primaries","bt2020","-color_trc","smpte2084","-x265-params",parameters,"-c:a","aac"]).arg(&fixture).output().unwrap());
        } else {
            // Explicit H.264 VUI gives compatibility mode certified SDR facts;
            // merely tagging the MP4 container may omit transfer/primaries.
            let mut command = Command::new(&ffmpeg);
            command.args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=30:duration=3",
            ]);
            if audio.is_some() {
                command.args([
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=1000:sample_rate=48000:duration=3",
                ]);
            }
            command.args([
                "-map",
                "0:v:0",
                "-threads",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-x264-params",
                "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=limited",
            ]);
            if audio.is_some() {
                command.args(["-map", "1:a:0", "-c:a", "aac"]);
            }
            real_media::checked(command.arg(&fixture).output().unwrap());
        }
        let kind = if audio.is_none() {
            XMediaKind::AnimatedVideo
        } else {
            XMediaKind::Video
        };
        let mut inventory = inventory(ordinal, kind);
        if hdr {
            inventory.items[0].duration_seconds = Some(1.0);
        }
        let identity = inventory.items[0].identity.as_ref().unwrap();
        let process = LocalOnly {
            fixture: fixture.clone(),
            metadata: metadata(&inventory),
        };
        for compatibility in [false, true] {
            let resolved = YtDlpXVideoAnalyzer::new(&resolver, &process)
                .analyze_item(&inventory, identity, compatibility, &context())
                .unwrap();
            let (media, plan, source) = resolved.into_parts();
            let row = &media.formats[0];
            assert_eq!(
                (row.width, row.height, row.fps),
                (Some(160), Some(90), Some(30.0))
            );
            assert_eq!(row.audio_codec.as_deref(), audio);
            assert_eq!(
                row.video_codec.as_deref(),
                Some(if hdr { "hevc" } else { "h264" })
            );
            assert_eq!(
                plan.video_processing,
                Some(if compatibility {
                    StreamProcessing::Transcode
                } else {
                    StreamProcessing::Copy
                })
            );
            assert!(plan.clip.is_none());
            assert_eq!(plan.output_container.as_deref(), Some("mp4"));
            let job = (42 + u32::from(ordinal) * 2 + u32::from(compatibility)).to_string();
            let child = format!("{job}{ordinal:02}");
            let item_dir = root.path().join(&job).join(format!("{ordinal:02}"));
            let work = item_dir.join(&child);
            fs::create_dir_all(&work).unwrap();
            let executor = YtDlpDownloadExecutor::new(
                &resolver,
                &process,
                FfmpegMediaProcessor::new(&resolver, WindowsProcessSupervisor),
            );
            let staged = executor
                .download(
                    DownloadExecutionRequest {
                        job_id: child,
                        source: inventory.source.clone(),
                        playlist_item: None,
                        instagram_selector: None,
                        tiktok_source: None,
                        x_source: Some(source),
                        bilibili_source: None,
                        video_source: media
                            .formats
                            .iter()
                            .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
                            .cloned(),
                        audio_source: media
                            .formats
                            .iter()
                            .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
                            .cloned(),
                        plan: plan.clone(),
                        staging_dir: work.clone(),
                    },
                    CancellationToken::new(),
                    &Sink,
                )
                .unwrap_or_else(|error| panic!("ordinal={ordinal}, compatibility={compatibility}: {error:?}; plan={plan:?}; input={}",real_media::probe(&ffprobe,&fixture)));
            assert!(
                !work.join("source").exists(),
                "private info JSON/source workspace cleaned"
            );
            let verified = FfprobeOutputVerifier::new(&resolver, WindowsProcessSupervisor)
                .verify(staged, &plan, CancellationToken::new())
                .unwrap();
            assert_eq!(verified.has_audio, audio.is_some());
            let path = item_dir.join(format!("stage.{}", verified.container));
            fs::rename(&verified.path, &path).unwrap();
            let asset = XVerifiedAsset::Video(VerifiedOutput {
                path: path.canonicalize().unwrap(),
                ..verified
            });
            let published = publish(asset, &output, &job, public(ordinal, kind));
            let after = real_media::probe(&ffprobe, &published);
            assert_eq!(after["streams"][0]["width"], 160);
            assert_eq!(after["streams"][0]["height"], 90);
            assert_eq!(after["streams"][0]["avg_frame_rate"], "30/1");
            assert_eq!(
                after["streams"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| s["codec_type"] == "audio")
                    .count(),
                usize::from(audio.is_some())
            );
            if compatibility {
                assert_eq!(after["streams"][0]["codec_name"], "h264");
            } else {
                real_media::assert_copy_equivalent(
                    &ffmpeg,
                    &fixture,
                    &published,
                    real_media::StreamRole::Video,
                );
                if audio.is_some() {
                    real_media::assert_copy_equivalent(
                        &ffmpeg,
                        &fixture,
                        &published,
                        real_media::StreamRole::Audio,
                    );
                }
                if hdr {
                    assert_eq!(
                        real_media::probe_video_frame(&ffprobe, &published),
                        real_media::probe_video_frame(&ffprobe, &fixture)
                    );
                }
            }
        }
    }
}
