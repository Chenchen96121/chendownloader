//! Explicit opt-in real-site gate. No login, raw URL logging, or remote fallback.
#[path = "support/real_media.rs"]
mod real_media;
use local_media_downloader_lib::{
    application::{
        bilibili_analysis::BilibiliAnalysisService,
        bilibili_download::{
            BilibiliDownloadRefresher, BilibiliDownloadSnapshot, VerifiedBilibiliDownloadSource,
        },
        bilibili_source::{BilibiliPageLoader, BilibiliRequestContext},
        download_activity::{DownloadActivityGate, DownloadOwner},
        download_job::{
            CompletedLocationOpener, DownloadDestinationResolver, DownloadEventEmitter,
            DownloadInputResolver, DownloadJobManager, DownloadJobServices, ResolvedDownloadInput,
        },
        network_profile::NetworkProfile,
        portable_package::EMBEDDED_TOOL_MANIFEST,
        preview_selection::SelectionOutcome,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        bilibili::validate_bilibili_url,
        download::{DownloadErrorCode, DownloadProgress, DownloadState},
        format_selection::{ClipRange, SelectionIntent, SelectionMode},
        managed_tools::{ManagedToolError, ManagedToolManifest, ToolName, BUILD_TARGET},
        media::AnalysisErrorCode,
    },
    infrastructure::{
        bilibili::{
            cover::HttpBilibiliCoverLoader,
            page::{
                BilibiliPageResponse, BilibiliPageTransport, HttpBilibiliPageLoader,
                ReqwestBilibiliPageTransport,
            },
        },
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        output::{FilesystemJobStaging, FilesystemOutputPublisher},
        process::WindowsProcessSupervisor,
        ytdlp::{bilibili::YtDlpBilibiliAnalyzer, download::YtDlpDownloadExecutor},
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
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
struct LiveProcess {
    diagnostic: Option<PathBuf>,
}
impl ProcessSupervisor for LiveProcess {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let name = request
            .program
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let directory = request.working_dir.clone();
        let selected_request = request
            .args
            .last()
            .and_then(|arg| arg.to_str())
            .and_then(|url| validate_bilibili_url(url).ok());
        let start = Instant::now();
        let result = WindowsProcessSupervisor.run(request, cancellation, sink);
        eprintln!(
            "Bili tool boundary: {}, elapsedMs={}, exit={:?}, processError={:?}",
            name.split('-').next().unwrap(),
            start.elapsed().as_millis(),
            result.as_ref().ok().map(|r| r.code),
            result.as_ref().err()
        );
        if name.starts_with("yt-dlp") {
            if let Ok(exit) = &result {
                eprintln!(
                    "Bili captured limits: stdout={}, stderr={}, stderrUtf8={}",
                    exit.stdout.len(),
                    exit.stderr.len(),
                    std::str::from_utf8(&exit.stderr).is_ok()
                );
                let raw = serde_json::from_slice::<serde_json::Value>(&exit.stdout);
                eprintln!(
                    "Bili extractor JSON: bytes={}, valid={}",
                    exit.stdout.len(),
                    raw.is_ok()
                );
                if let Ok(raw) = &raw {
                    eprintln!("Bili extractor schema: ordinaryKey={}, archiveCount={:?}, formats={:?}, durationNumber={}", raw["extractor_key"] == "BiliBili", raw["_old_archive_ids"].as_array().map(Vec::len), raw["formats"].as_array().map(Vec::len), raw["duration"].is_number());
                    if let Some(rows) = raw["formats"].as_array() {
                        for (index, row) in rows.iter().take(16).enumerate() {
                            eprintln!("Bili format shape index={index}, ext={:?}, protocol={:?}, video={:?}, audio={:?}, nonpositiveNumericFields={:?}", row["ext"].as_str().filter(|s| s.len()<16), row["protocol"].as_str().filter(|s| s.len()<32), row["vcodec"].as_str().filter(|s| s.len()<64), row["acodec"].as_str().filter(|s| s.len()<64), ["width","height","fps","tbr","vbr","abr","asr","audio_channels","filesize","filesize_approx"].into_iter().filter(|key| row[*key].as_f64().is_some_and(|n| n<=0.0)).collect::<Vec<_>>());
                        }
                    }
                }
                if let Some(selected) = selected_request {
                    for entry in fs::read_dir(&directory).unwrap().take(16) {
                        let entry = entry.unwrap();
                        let metadata = entry.metadata().unwrap();
                        if entry.path().extension().is_some_and(|ext| ext == "dump")
                            && metadata.len() <= 4 * 1024 * 1024
                        {
                            let bytes = fs::read(entry.path()).unwrap();
                            let page = local_media_downloader_lib::infrastructure::bilibili::page::parse_bilibili_page(&selected, &bytes);
                            eprintln!("Bili consumed dump: bytes={}, gzipMagic={}, utf8={}, pageError={:?}", bytes.len(), bytes.starts_with(&[0x1f,0x8b]), std::str::from_utf8(&bytes).is_ok(), page.as_ref().err());
                            if let (Ok(page), Ok(raw)) = (page, &raw) {
                                eprintln!("Bili consumed metadata match error={:?}", local_media_downloader_lib::infrastructure::ytdlp::bilibili::match_bilibili_metadata(&page, raw).err());
                            }
                        }
                    }
                }
            }
        }
        if name.starts_with("ffprobe") {
            if let Ok(exit) = &result {
                if let Ok(probe) = serde_json::from_slice::<serde_json::Value>(&exit.stdout) {
                    if let Some(streams) = probe["streams"].as_array() {
                        for (index, stream) in streams.iter().take(2).enumerate() {
                            let mut facts = serde_json::Map::new();
                            for key in [
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
                            ] {
                                if let Some(value) = stream.get(key) {
                                    if value.is_number()
                                        || value.as_str().is_some_and(|s| {
                                            s.len() <= 64
                                                && s.bytes().all(|b| {
                                                    b.is_ascii_alphanumeric()
                                                        || b" ._+-".contains(&b)
                                                        || (key == "avg_frame_rate" && b == b'/')
                                                })
                                        })
                                    {
                                        facts.insert(key.into(), value.clone());
                                    }
                                }
                            }
                            eprintln!(
                                "Bili probe stream{index}: {}",
                                serde_json::Value::Object(facts)
                            );
                        }
                    }
                }
            }
        }
        if let (Some(directory), Ok(exit)) = (&self.diagnostic, &result) {
            use std::io::Write;
            // A bounded private stderr only. No raw JSON/media URLs go to test output.
            let mut file = tempfile::NamedTempFile::new_in(directory).unwrap();
            file.write_all(&exit.stderr[..exit.stderr.len().min(65536)])
                .unwrap();
            file.keep().unwrap();
        }
        result
    }
}
struct StatusTransport;
impl BilibiliPageTransport for StatusTransport {
    fn get(
        &self,
        url: &url::Url,
        context: &BilibiliRequestContext,
        deadline: Instant,
    ) -> Result<BilibiliPageResponse, AnalysisErrorCode> {
        let response = ReqwestBilibiliPageTransport.get(url, context, deadline)?;
        if response.status == 200 {
            let text = std::str::from_utf8(&response.body);
            eprintln!(
                "Bili page encoding: utf8={}, gzipMagic={}",
                text.is_ok(),
                response.body.starts_with(&[0x1f, 0x8b])
            );
            if let Ok(text) = text {
                eprintln!(
                    "Bili state shape: markers={}, scriptOpens={}, scriptCloses={}",
                    text.matches("window.__INITIAL_STATE__").count(),
                    text.matches("<script").count(),
                    text.matches("</script>").count()
                );
                if let Some((_, tail)) = text.split_once("window.__INITIAL_STATE__") {
                    if let Some(payload) = tail.trim_start().strip_prefix('=') {
                        let mut json = serde_json::Deserializer::from_str(payload.trim_start())
                            .into_iter::<serde_json::Value>();
                        if let Some(Ok(value)) = json.next() {
                            eprintln!("Bili state JSON: valid=true, semicolon={}, videoData={}, errorObject={}, pages={:?}, rightsFlags={:?}", payload.trim_start()[json.byte_offset()..].trim_start().starts_with(';'), value["videoData"].is_object(), value["error"].is_object(), value["videoData"]["pages"].as_array().map(Vec::len), ["pay","ugc_pay","movie","is_stein_gate"].map(|key| value["videoData"]["rights"][key].as_u64()));
                        } else {
                            eprintln!("Bili state JSON: valid=false");
                        }
                    }
                }
            }
        }
        if let Some(location) = &response.location {
            if let Ok(next) = url.join(location) {
                eprintln!("Bili redirect shape: requestSlash={}, targetSlash={}, sameHost={}, targetPart={:?}", url.path().ends_with('/'), next.path().ends_with('/'), url.host_str() == next.host_str(), validate_bilibili_url(next.as_str()).ok().map(|r| r.part()));
            }
        }
        eprintln!(
            "Bili ordinary HTTP boundary: status={}, bodyBytes={}, redirect={}",
            response.status,
            response.body.len(),
            response.location.is_some()
        );
        Ok(response)
    }
}
type Service = BilibiliAnalysisService<
    YtDlpBilibiliAnalyzer<Arc<Resolver>, Arc<LiveProcess>, HttpBilibiliPageLoader<StatusTransport>>,
    HttpBilibiliCoverLoader<Arc<Resolver>, Arc<LiveProcess>>,
>;
struct LiveInput(Arc<Service>);
impl DownloadInputResolver for LiveInput {
    fn resolve_download(
        &self,
        id: &str,
        intent: SelectionIntent,
    ) -> Result<ResolvedDownloadInput, DownloadErrorCode> {
        let snapshot = self.0.prepare_download(id, intent, Instant::now())?;
        Ok(ResolvedDownloadInput {
            source: snapshot.source().clone(),
            media: snapshot.media().clone(),
            plan: snapshot.plan().clone(),
            bilibili: Some(snapshot),
        })
    }
}
impl BilibiliDownloadRefresher for LiveInput {
    fn refresh(
        &self,
        snapshot: &BilibiliDownloadSnapshot,
        cancellation: CancellationToken,
    ) -> Result<VerifiedBilibiliDownloadSource, DownloadErrorCode> {
        self.0.refresh(snapshot, cancellation)
    }
}
struct Destination(PathBuf);
impl DownloadDestinationResolver for Destination {
    fn resolve_download_destination(&self, id: &str) -> Result<PathBuf, DownloadErrorCode> {
        if id == "1" {
            Ok(self.0.clone())
        } else {
            Err(DownloadErrorCode::InvalidRequest)
        }
    }
}
struct Events;
impl DownloadEventEmitter for Events {
    fn emit(&self, _: DownloadProgress) -> Result<(), DownloadErrorCode> {
        Ok(())
    }
}
struct NoOpen;
impl CompletedLocationOpener for NoOpen {
    fn open(&self, _: &Path) -> Result<(), DownloadErrorCode> {
        Err(DownloadErrorCode::InvalidRequest)
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
fn context() -> BilibiliRequestContext {
    BilibiliRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}
fn diagnostic_directory() -> Option<PathBuf> {
    std::env::var_os("LMD_BILIBILI_DIAGNOSTIC_DIR").map(|value| {
        let directory = PathBuf::from(value)
            .canonicalize()
            .expect("diagnostic directory must already exist");
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap();
        assert!(
            directory.starts_with(repo.join(".release/bilibili-diagnostics")),
            "diagnostics must be inside the ignored diagnostics directory"
        );
        assert!(directory.is_dir());
        directory
    })
}

#[test]
#[ignore = "requires exact pinned tools and one explicit ordinary public Bili URL; anonymous live regression"]
fn real_bilibili_single_url_analysis_and_download_preserve_original_streams() {
    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let url = std::env::var("LMD_BILIBILI_REGRESSION_URL")
        .expect("one explicit ordinary public Bili URL required");
    let request = validate_bilibili_url(&url).expect("ordinary public video URL required");
    let resolver = Arc::new(Resolver(tools));
    let process = Arc::new(LiveProcess { diagnostic: None });
    let service = Arc::new(BilibiliAnalysisService::new(
        YtDlpBilibiliAnalyzer::new(
            resolver.clone(),
            process.clone(),
            HttpBilibiliPageLoader::with_transport(StatusTransport),
        ),
        HttpBilibiliCoverLoader::new(resolver.clone(), process.clone()),
    ));
    let preview = service.analyze(&url, best(), Instant::now()).unwrap();
    assert_eq!(preview.part, request.part());
    let SelectionOutcome::Selected { plan } = &preview.outcome else {
        panic!("highest original streams must remain selectable");
    };
    assert!(!plan.compatibility_mode);
    use local_media_downloader_lib::domain::output_policy::StreamProcessing;
    assert_eq!(plan.video_processing, Some(StreamProcessing::Copy));
    assert_eq!(plan.audio_processing, Some(StreamProcessing::Copy));
    let snapshot = service
        .prepare_download(&preview.analysis_id, best(), Instant::now())
        .unwrap();
    assert_eq!(snapshot.plan(), plan);
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("published");
    fs::create_dir(&destination).unwrap();
    let input = Arc::new(LiveInput(service.clone()));
    let jobs = DownloadJobManager::for_bilibili(
        DownloadJobServices {
            activity: Arc::new(DownloadActivityGate::new()),
            input: input.clone(),
            destination: Arc::new(Destination(destination.clone())),
            executor: Arc::new(YtDlpDownloadExecutor::new(
                resolver.clone(),
                process.clone(),
                FfmpegMediaProcessor::new(resolver.clone(), process.clone()),
            )),
            verifier: Arc::new(FfprobeOutputVerifier::new(resolver, process)),
            publisher: Arc::new(FilesystemOutputPublisher::new()),
            staging: Arc::new(FilesystemJobStaging::new(root.path().join("jobs"))),
            emitter: Arc::new(Events),
            opener: Arc::new(NoOpen),
        },
        input,
    );
    let started = Instant::now();
    let job = jobs
        .start_download(&preview.analysis_id, "1", best())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(600);
    let result = loop {
        if let Ok(result) = jobs.download_result(&job.job_id) {
            break result;
        }
        if Instant::now() >= deadline {
            jobs.shutdown();
            panic!("single-URL download exceeded test deadline");
        }
        thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        result.state,
        DownloadState::Completed,
        "error={:?}",
        result.error_code
    );
    let bytes = fs::metadata(destination.join(result.display_name.unwrap()))
        .unwrap()
        .len();
    assert!(bytes > 0);
    eprintln!("Bili single-URL original-stream regression: P={}, formats={}, container={:?}, bytes={bytes}, downloadMs={}", preview.part, preview.media.formats.len(), plan.output_container, started.elapsed().as_millis());
    jobs.shutdown();
    service.shutdown();
}

#[test]
#[ignore = "requires exact pinned tools and three explicit ordinary public Bili URLs; anonymous real-network downloads"]
fn real_bilibili_bv_av_and_selected_part_use_production_pipeline() {
    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let urls = [
        "LMD_BILIBILI_BV_URL",
        "LMD_BILIBILI_AV_URL",
        "LMD_BILIBILI_PART_URL",
    ]
    .map(|name| std::env::var(name).expect("three explicit ordinary public sample URLs required"));
    let requests = urls.each_ref().map(|url| {
        validate_bilibili_url(url).expect("live URL outside approved first-version scope")
    });
    assert_eq!(requests[0].part(), 1);
    assert_eq!(requests[1].part(), 1);
    assert!(requests[2].part() > 1);
    let pages = HttpBilibiliPageLoader::with_transport(StatusTransport);
    let mut identities = vec![];
    let mut errors = vec![];
    for (index, request) in requests.iter().enumerate() {
        let started = Instant::now();
        let page = match pages.inspect(request, &context()) {
            Ok(page) => page,
            Err(error) => {
                eprintln!(
                    "live ordinary-page witness sample={index}, elapsedMs={}: {error:?}",
                    started.elapsed().as_millis()
                );
                errors.push((index, error));
                continue;
            }
        };
        eprintln!(
            "Bili page sample={index}, part={}, fullSeconds={}, elapsedMs={}",
            page.identity().part,
            page.identity().duration_seconds,
            started.elapsed().as_millis()
        );
        identities.push(page.identity().clone());
    }
    assert!(
        errors.is_empty(),
        "ordinary-page live gate failed: {errors:?}; no extractor/media download attempted"
    );
    assert!(
        identities[0] == identities[1],
        "BV and av must resolve to exactly the same ordinary P1 identity"
    );
    assert_eq!(identities[2].part, requests[2].part());
    if identities[2].bvid == identities[0].bvid {
        assert_ne!(identities[2].cid, identities[0].cid);
    }
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("published");
    fs::create_dir(&destination).unwrap();
    let resolver = Arc::new(Resolver(tools));
    let process = Arc::new(LiveProcess {
        diagnostic: diagnostic_directory(),
    });
    let service = Arc::new(BilibiliAnalysisService::new(
        YtDlpBilibiliAnalyzer::new(resolver.clone(), process.clone(), pages),
        HttpBilibiliCoverLoader::new(resolver.clone(), process.clone()),
    ));
    let input = Arc::new(LiveInput(service.clone()));
    let gate = Arc::new(DownloadActivityGate::new());
    // Controlled foreign-owner lease proves real Bili jobs share the same limit.
    // This is not a real X media download; X media has its separate real-tool gate.
    let foreign = gate
        .reserve(DownloadOwner::X, "Controlled foreign-owner lease")
        .unwrap();
    foreign.wait_turn(&CancellationToken::new()).unwrap();
    let jobs = DownloadJobManager::for_bilibili(
        DownloadJobServices {
            activity: gate.clone(),
            input: input.clone(),
            destination: Arc::new(Destination(destination.clone())),
            executor: Arc::new(YtDlpDownloadExecutor::new(
                resolver.clone(),
                process.clone(),
                FfmpegMediaProcessor::new(resolver.clone(), process.clone()),
            )),
            verifier: Arc::new(FfprobeOutputVerifier::new(resolver.clone(), process)),
            publisher: Arc::new(FilesystemOutputPublisher::new()),
            staging: Arc::new(FilesystemJobStaging::new(root.path().join("jobs"))),
            emitter: Arc::new(Events),
            opener: Arc::new(NoOpen),
        },
        input,
    );
    for case in 0..8 {
        let sample = if case < 3 { case } else { 0 };
        let started = Instant::now();
        let data = service
            .analyze(&urls[sample], best(), Instant::now())
            .unwrap_or_else(|error| panic!("live analysis case={case}: {error:?}"));
        assert_eq!(data.part, identities[sample].part);
        assert!(data.media.media_id.ends_with(&format!("_p{}", data.part)));
        let mut intent = best();
        if (3..=5).contains(&case) {
            intent.mode = [
                SelectionMode::Manual,
                SelectionMode::VideoOnly,
                SelectionMode::AudioOnly,
            ][case - 3];
            if intent.mode != SelectionMode::AudioOnly {
                let rows = service
                    .resolution_options(&data.analysis_id, intent.mode, false, Instant::now())
                    .unwrap();
                let chosen = rows
                    .options
                    .iter()
                    .rev()
                    .find(|row| matches!(row.outcome, SelectionOutcome::Selected { .. }))
                    .expect("at least one feasible native manual/video row");
                intent.video_format_id = Some(chosen.video_format_id.clone());
                intent.output_container = Some(chosen.output_container);
            }
        }
        if case == 6 {
            intent.compatibility_mode = true;
        }
        if case == 7 {
            intent.clip = Some(ClipRange {
                start_ms: 0,
                end_ms: 3000.min((identities[sample].duration_seconds * 1000.0) as u64),
            });
        }
        let selected = service
            .preview(&data.analysis_id, intent.clone(), Instant::now())
            .unwrap();
        let SelectionOutcome::Selected { plan } = selected.outcome else {
            panic!("live requested mode unavailable case={case}");
        };
        for format in selected
            .media
            .formats
            .iter()
            .filter(|format| plan.source_format_ids.contains(&format.id))
        {
            eprintln!(
                "Bili frozen safe format: {}",
                serde_json::to_string(format).unwrap()
            );
        }
        assert_eq!(plan.compatibility_mode, case == 6);
        if !plan.compatibility_mode {
            assert_ne!(
                plan.video_processing,
                Some(
                    local_media_downloader_lib::domain::output_policy::StreamProcessing::Transcode
                )
            );
            assert_ne!(
                plan.audio_processing,
                Some(
                    local_media_downloader_lib::domain::output_policy::StreamProcessing::Transcode
                )
            );
        }
        let job = jobs.start_download(&data.analysis_id, "1", intent).unwrap();
        let deadline = Instant::now() + Duration::from_secs(600);
        let result = loop {
            if let Ok(result) = jobs.download_result(&job.job_id) {
                break result;
            }
            if Instant::now() >= deadline {
                jobs.shutdown();
                panic!("live case={case} exceeded explicit test deadline");
            }
            thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(
            result.state,
            DownloadState::Completed,
            "live case={case}, error={:?}",
            result.error_code
        );
        let output = destination.join(result.display_name.unwrap());
        let length = fs::metadata(&output).unwrap().len();
        assert!(length > 0);
        eprintln!(
            "Bili complete case={case}, P={}, container={:?}, bytes={length}, elapsedMs={}",
            data.part,
            plan.output_container,
            started.elapsed().as_millis()
        );
        // Files are isolated generated test outputs and drop with TempDir, never user downloads.
        fs::remove_file(&output).unwrap();
    }
    jobs.shutdown();
    service.shutdown();
    drop(foreign);
}
