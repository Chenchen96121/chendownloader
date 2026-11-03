#[path = "support/real_media.rs"]
mod real_media;
use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        managed_tools::{ManagedToolError, ToolName, BUILD_TARGET},
        media::StreamKind,
        output_policy::StreamProcessing,
    },
    infrastructure::{process::WindowsProcessSupervisor, ytdlp::YtDlpInstagramVideoAnalyzer},
};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
struct Resolver(PathBuf);
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "pinned fixture".into(),
            path: self.0.join(name.artifact_name(BUILD_TARGET)),
        })
    }
}
struct LocalOnly {
    fixture: PathBuf,
}
impl ProcessSupervisor for LocalOnly {
    fn run(
        &self,
        mut request: ProcessRequest,
        cancellation: CancellationToken,
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
            if !request.args.iter().any(|arg| arg == "--simulate") {
                assert!(request
                    .args
                    .windows(2)
                    .any(|pair| pair[0] == "--format" && pair[1] == "0"));
                fs::copy(&self.fixture, request.working_dir.join("source.mp4")).unwrap();
                return Ok(ProcessExit {
                    code: 0,
                    stdout: vec![],
                    stderr: vec![],
                });
            }
            return Ok(ProcessExit { code: 0, stdout: serde_json::to_vec(&serde_json::json!({
                "extractor_key":"Instagram", "id":"B", "title":"Local video fixture", "playlist_id":"Post_01", "playlist_index":2, "playlist_count":2,
                "formats":[{"format_id":"0", "ext":"mp4", "width":160, "height":90, "vcodec":null, "acodec":null, "url":"https://scontent.cdninstagram.com/private.mp4"}]
            })).unwrap(), stderr: vec![] });
        }
        // Boundary fixture only: production has already validated the CDN URL.
        // Replace it with a local file and remove network-only options. The real
        // supervisor/FFprobe/JSON projection still execute, with no site request.
        assert!(request
            .program
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ffprobe"));
        let mut args: Vec<OsString> = vec![];
        let mut index = 0;
        while index < request.args.len() {
            let option = request.args[index].to_string_lossy();
            if matches!(
                option.as_ref(),
                "-http_proxy"
                    | "-user_agent"
                    | "-referer"
                    | "-max_redirects"
                    | "-rw_timeout"
                    | "-protocol_whitelist"
            ) {
                index += 2;
                continue;
            }
            if option == "-i" {
                args.extend(["-i".into(), self.fixture.as_os_str().to_owned()]);
                index += 2;
                continue;
            }
            args.push(request.args[index].clone());
            index += 1;
        }
        request.args = args;
        let result = WindowsProcessSupervisor.run(request, cancellation, sink);
        if let Ok(exit) = &result {
            println!(
                "Local bounded stream probe: {}",
                String::from_utf8_lossy(&exit.stdout)
            );
        }
        result
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

struct ProxyOnly {
    endpoint: String,
    stderr: Mutex<Vec<u8>>,
}

impl ProcessSupervisor for ProxyOnly {
    fn run(
        &self,
        mut request: ProcessRequest,
        cancellation: CancellationToken,
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
                stdout: serde_json::to_vec(&serde_json::json!({
                    "extractor_key":"Instagram", "id":"B", "title":"Proxy fixture",
                    "playlist_id":"Post_01", "playlist_index":2, "playlist_count":2,
                    "formats":[{"format_id":"0", "ext":"mp4", "vcodec":null,
                        "acodec":null, "width":160, "height":90,
                        "url":"https://scontent.cdninstagram.com/private.mp4"}]
                }))
                .unwrap(),
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
        // Preserve the approved URL and every production network option. Only
        // replace the proxy route, so no request can reach a platform server.
        if let Some(index) = request.args.iter().position(|arg| arg == "-http_proxy") {
            request.args[index + 1] = self.endpoint.clone().into();
        } else {
            request.args.splice(
                0..0,
                [OsString::from("-http_proxy"), self.endpoint.clone().into()],
            );
        }
        let result = WindowsProcessSupervisor.run(request, cancellation, sink);
        if let Ok(exit) = &result {
            *self.stderr.lock().unwrap() = exit.stderr.clone();
        }
        result
    }
}

#[test]
#[ignore = "requires pinned portable tools; real FFprobe through a local CONNECT proxy, no site request"]
fn real_ffprobe_reaches_http_connect_proxy_with_production_network_options() {
    // Removing httpproxy from the production protocol whitelist must fail this
    // behavior check before the real process can contact the local fixture.
    let tools = real_media::tools();
    let mut proxy = ConnectProxyFixture::start();
    let process = ProxyOnly {
        endpoint: proxy.endpoint.clone(),
        stderr: Mutex::new(vec![]),
    };
    let result = YtDlpInstagramVideoAnalyzer::new(Resolver(tools), &process).analyze_post_item(
        &validate_media_url("https://www.instagram.com/p/Post_01/").unwrap(),
        2,
        "1",
        false,
        &CancellationToken::new(),
    );
    let request = proxy.finish();
    let stderr = String::from_utf8_lossy(&process.stderr.lock().unwrap()).into_owned();
    assert!(
        request.starts_with("CONNECT scontent.cdninstagram.com:443 HTTP/1.1\r\n"),
        "real FFprobe did not reach the local CONNECT proxy: {stderr}"
    );
    assert!(result.is_err(), "the controlled 502 must stop analysis");
    assert!(
        (stderr.contains("502") || stderr.to_ascii_lowercase().contains("5xx"))
            && !stderr.contains("not on whitelist"),
        "expected the controlled proxy response, not a local transport rejection: {stderr}"
    );
}

#[test]
#[ignore = "requires pinned portable tools; real FFprobe on local H.264/AAC and silent MP4 fixtures, no site request"]
fn real_ffprobe_hydrates_original_instagram_video_and_audio_facts() {
    let tools = real_media::tools();
    let root = tempfile::tempdir().unwrap();
    let ffmpeg = tools.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET));
    for audio in [Some("aac"), None] {
        let fixture = root.path().join(if audio.is_some() {
            "combined.mp4"
        } else {
            "silent.mp4"
        });
        real_media::generate_source(&ffmpeg, &fixture, Some("h264"), audio);
        let analyzer = YtDlpInstagramVideoAnalyzer::new(
            Resolver(tools.clone()),
            LocalOnly {
                fixture: fixture.clone(),
            },
        );
        let resolved = analyzer
            .analyze_post_item(
                &validate_media_url("https://www.instagram.com/p/Post_01/").unwrap(),
                2,
                "1",
                false,
                &CancellationToken::new(),
            )
            .unwrap();
        let row = &resolved.media().formats[0];
        assert_eq!(row.video_codec.as_deref(), Some("h264"));
        assert_eq!(row.audio_codec.as_deref(), audio);
        assert_eq!(
            (row.width, row.height, row.fps),
            (Some(160), Some(90), Some(30.0))
        );
        if let Some(size) = &row.file_size {
            assert_eq!(size.bytes, fs::metadata(&fixture).unwrap().len());
        }
        assert_eq!(
            row.kind,
            if audio.is_some() {
                StreamKind::Combined
            } else {
                StreamKind::VideoOnly
            }
        );
        assert_eq!(
            resolved.plan().video_processing,
            Some(StreamProcessing::Copy)
        );
        assert_eq!(resolved.plan().output_container.as_deref(), Some("mp4"));

        // Exercise the production re-analysis -> exact stream download -> real
        // lossless FFmpeg/FFprobe -> safe publication pipeline on local bytes.
        use local_media_downloader_lib::{
            application::{
                download_executor::DownloadProgressSink,
                instagram_download_job::{
                    InstagramOutputPublisher, InstagramPublicationRequest,
                    InstagramVideoDownloadPipeline, InstagramVideoDownloader,
                },
            },
            domain::{
                download::DownloadProgress,
                instagram::{
                    InstagramItemAvailability, InstagramMediaItem, InstagramMediaKind,
                    InstagramSourceKind,
                },
            },
            infrastructure::{
                ffmpeg::FfmpegMediaProcessor, ffprobe::FfprobeOutputVerifier,
                output::instagram::FilesystemInstagramOutputPublisher,
                ytdlp::download::YtDlpDownloadExecutor,
            },
        };
        struct Sink;
        impl DownloadProgressSink for Sink {
            fn on_progress(&self, progress: DownloadProgress) {
                assert_eq!(progress.job_id, "42");
            }
        }
        let staging = root
            .path()
            .join(if audio.is_some() {
                "combined-jobs"
            } else {
                "silent-jobs"
            })
            .join("42")
            .join("02");
        fs::create_dir_all(&staging).unwrap();
        let resolver = Resolver(tools.clone());
        let local = LocalOnly {
            fixture: fixture.clone(),
        };
        let pipeline = InstagramVideoDownloadPipeline::new(
            YtDlpInstagramVideoAnalyzer::new(&resolver, &local),
            YtDlpDownloadExecutor::new(
                &resolver,
                &local,
                FfmpegMediaProcessor::new(&resolver, WindowsProcessSupervisor),
            ),
            FfprobeOutputVerifier::new(&resolver, WindowsProcessSupervisor),
        );
        let asset = pipeline
            .download(
                &validate_media_url("https://www.instagram.com/p/Post_01/").unwrap(),
                resolved.selector(),
                false,
                "42",
                &staging,
                CancellationToken::new(),
                &Sink,
            )
            .unwrap();
        let destination = root.path().join(if audio.is_some() {
            "combined-output"
        } else {
            "silent-output"
        });
        fs::create_dir(&destination).unwrap();
        let publisher = FilesystemInstagramOutputPublisher::new();
        let publication = publisher
            .prepare(
                &destination,
                &InstagramPublicationRequest::new(
                    "42",
                    InstagramSourceKind::Post,
                    "Post_01",
                    "Local fixture",
                    None,
                    2,
                )
                .unwrap(),
            )
            .unwrap();
        let item = InstagramMediaItem {
            item_id: "2".into(),
            ordinal: 2,
            kind: InstagramMediaKind::Video,
            width: row.width,
            height: row.height,
            container: "mp4".into(),
            file_size: row.file_size,
            fps: row.fps,
            video_bitrate_kbps: row.video_bitrate_kbps,
            audio_bitrate_kbps: row.audio_bitrate_kbps,
            video_codec: row.video_codec.clone(),
            audio_codec: row.audio_codec.clone(),
            dynamic_range: row.dynamic_range.clone(),
            preview_available: false,
            availability: InstagramItemAvailability::Ready,
            unavailable_reason: None,
        };
        let name = publisher
            .publish(asset, &publication, &item, &CancellationToken::new())
            .unwrap();
        let output = publication.output_directory().join(name);
        assert_eq!(
            real_media::content_hash(&ffmpeg, &fixture, real_media::StreamRole::Video),
            real_media::content_hash(&ffmpeg, &output, real_media::StreamRole::Video)
        );
        if audio.is_some() {
            assert_eq!(
                real_media::content_hash(&ffmpeg, &fixture, real_media::StreamRole::Audio),
                real_media::content_hash(&ffmpeg, &output, real_media::StreamRole::Audio)
            );
        }
    }
}
