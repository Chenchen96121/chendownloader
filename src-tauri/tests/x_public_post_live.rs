//! Opt-in live diagnostics. Captures stay in an explicitly supplied local
//! diagnostic directory; signed media URLs never become test/IPC log output.
#[path = "support/real_media.rs"]
mod real_media;

use local_media_downloader_lib::{
    application::{
        download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        output_verifier::OutputVerifier,
        portable_package::EMBEDDED_TOOL_MANIFEST,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
        x_source::{XInventoryExtractor, XRequestContext},
        x_video::XVideoAnalyzer,
    },
    domain::{
        download::DownloadProgress,
        managed_tools::{ManagedToolError, ManagedToolManifest, ToolName, BUILD_TARGET},
        x::{XItemAvailability, XMediaKind},
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        process::WindowsProcessSupervisor,
        x::{
            hls_local::{
                HlsByteRange, HlsSegmentTransport, HlsTransferError, HttpHlsSegmentTransport,
            },
            manifest::{HlsManifestTransport, HttpXManifestTransport},
            GalleryDlXAdapter,
        },
        ytdlp::{download::YtDlpDownloadExecutor, x::YtDlpXVideoAnalyzer},
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
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
struct Capture {
    directory: PathBuf,
    sequence: AtomicUsize,
}
struct Progress;
struct ManifestCapture {
    directory: PathBuf,
    sequence: AtomicUsize,
}
impl HlsManifestTransport for ManifestCapture {
    fn read(
        &self,
        url: &url::Url,
        context: &XRequestContext,
    ) -> Result<Vec<u8>, local_media_downloader_lib::domain::media::AnalysisErrorCode> {
        let slot = self.sequence.fetch_add(1, Ordering::SeqCst);
        fs::write(
            self.directory.join(format!("manifest-{slot}.request.txt")),
            url.as_str(),
        )
        .unwrap();
        let started = Instant::now();
        let result = HttpXManifestTransport.read(url, context);
        eprintln!(
            "X manifest {slot}: elapsedMs={}, bytes={:?}, error={:?}",
            started.elapsed().as_millis(),
            result.as_ref().ok().map(Vec::len),
            result.as_ref().err()
        );
        result
    }
}
struct SegmentCapture {
    directory: PathBuf,
    sequence: AtomicUsize,
}
impl HlsSegmentTransport for SegmentCapture {
    fn fetch_probe(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        path: &std::path::Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError> {
        let slot = self.sequence.fetch_add(1, Ordering::SeqCst);
        fs::write(
            self.directory.join(format!("segment-{slot}.request.txt")),
            url.as_str(),
        )
        .unwrap();
        let started = Instant::now();
        let result = HttpHlsSegmentTransport.fetch_probe(url, range, path, maximum, context);
        eprintln!(
            "X segment {slot} probe: elapsedMs={}, bytes={:?}, error={:?}",
            started.elapsed().as_millis(),
            result.as_ref().ok(),
            result.as_ref().err()
        );
        result
    }
    fn fetch(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        path: &std::path::Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError> {
        let slot = self.sequence.fetch_add(1, Ordering::SeqCst);
        fs::write(
            self.directory.join(format!("segment-{slot}.request.txt")),
            url.as_str(),
        )
        .unwrap();
        let started = Instant::now();
        let result = HttpHlsSegmentTransport.fetch(url, range, path, maximum, context);
        eprintln!(
            "X segment {slot}: elapsedMs={}, bytes={:?}, error={:?}",
            started.elapsed().as_millis(),
            result.as_ref().ok(),
            result.as_ref().err()
        );
        result
    }
}
impl DownloadProgressSink for Progress {
    fn on_progress(&self, _: DownloadProgress) {}
}
impl ProcessSupervisor for Capture {
    fn run(
        &self,
        request: ProcessRequest,
        cancel: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let slot = self.sequence.fetch_add(1, Ordering::SeqCst);
        let name = request
            .program
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if name.starts_with("ffprobe") {
            eprintln!(
                "X probe explicitProxy={}",
                request.args.iter().any(|arg| arg == "-http_proxy")
            );
            if let Some(index) = request.args.iter().position(|arg| arg == "-i") {
                let input = std::path::Path::new(&request.args[index + 1]);
                if input.is_file() && input.extension().is_some_and(|ext| ext == "m3u8") {
                    fs::copy(
                        input,
                        self.directory.join(format!("{slot}-sealed-playlist.m3u8")),
                    )
                    .unwrap();
                }
            }
        }
        let result = WindowsProcessSupervisor.run(request, cancel, sink);
        match &result {
            Ok(exit) => {
                fs::write(
                    self.directory.join(format!("{slot}-{name}.stdout.json")),
                    &exit.stdout,
                )
                .unwrap();
                fs::write(
                    self.directory.join(format!("{slot}-{name}.stderr.txt")),
                    &exit.stderr,
                )
                .unwrap();
                eprintln!(
                    "X boundary {slot}: {name}, exit={}, stdoutBytes={}, stderrBytes={}",
                    exit.code,
                    exit.stdout.len(),
                    exit.stderr.len()
                );
            }
            Err(error) => eprintln!("X boundary {slot}: {name}, process={error:?}"),
        }
        result
    }
}

#[test]
#[ignore = "requires exact pinned package and explicitly supplied public X URL; real anonymous network request"]
fn real_x_public_post_analyzes_and_downloads_video_with_production_adapters() {
    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let source = validate_media_url(
        &std::env::var("LMD_X_PUBLIC_POST_URL").expect("explicit public X sample required"),
    )
    .unwrap();
    let directory = PathBuf::from(
        std::env::var_os("LMD_X_DIAGNOSTIC_DIR")
            .expect("explicit private diagnostic directory required"),
    );
    assert!(directory.is_absolute());
    fs::create_dir(&directory).expect("diagnostics must use a new directory");
    let process = Capture {
        directory: directory.clone(),
        sequence: AtomicUsize::new(0),
    };
    let resolver = Resolver(tools);
    let analysis_started = Instant::now();
    let context = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    eprintln!(
        "X captured CDN proxy route={}",
        context
            .network
            .https_proxy_for_host("video.twimg.com")
            .is_some()
    );
    let inventory = GalleryDlXAdapter::new(&resolver, &process)
        .extract_post(&source, &context)
        .unwrap_or_else(|error| panic!("X inventory stage: {error:?}"));
    eprintln!("X inventory: {} items", inventory.items.len());
    let mut videos = 0;
    for item in &inventory.items {
        eprintln!(
            "X item {}: {:?}, {:?}, reason={:?}, variants={}",
            item.ordinal,
            item.kind,
            item.availability,
            item.unavailable_reason,
            item.video_candidates.len()
        );
        if !matches!(item.kind, XMediaKind::Video | XMediaKind::AnimatedVideo) {
            continue;
        }
        videos += 1;
        assert_eq!(
            item.availability,
            XItemAvailability::Ready,
            "X inventory item {}: {:?}",
            item.ordinal,
            item.unavailable_reason
        );
        let analyzed = YtDlpXVideoAnalyzer::with_transports(
            &resolver,
            &process,
            ManifestCapture {
                directory: directory.clone(),
                sequence: AtomicUsize::new(0),
            },
            SegmentCapture {
                directory: directory.clone(),
                sequence: AtomicUsize::new(0),
            },
        )
        .analyze_item(&inventory, item.identity.as_ref().unwrap(), false, &context)
        .unwrap_or_else(|error| panic!("X video stage, item {}: {error:?}", item.ordinal));
        assert!(!analyzed.plan().source_format_ids.is_empty());
        eprintln!(
            "X analysis elapsedMs={}",
            analysis_started.elapsed().as_millis()
        );
        eprintln!(
            "X video item {}: {} formats; selected={:?}",
            item.ordinal,
            analyzed.media().formats.len(),
            analyzed.plan().source_format_ids
        );
        let plan = analyzed.plan().clone();
        let selected = |id: &Option<String>| {
            id.as_ref()
                .and_then(|id| analyzed.media().formats.iter().find(|f| &f.id == id))
                .cloned()
        };
        let video = selected(&plan.video_format_id);
        let audio = selected(&plan.audio_format_id);
        eprintln!(
            "X highest video: {:?}x{:?}, {:?} FPS, {:?}; audio={:?}; container={:?}",
            video.as_ref().and_then(|v| v.width),
            video.as_ref().and_then(|v| v.height),
            video.as_ref().and_then(|v| v.fps),
            video.as_ref().and_then(|v| v.video_codec.as_ref()),
            audio
                .as_ref()
                .and_then(|a| a.audio_codec.as_ref())
                .or_else(|| video.as_ref().and_then(|v| v.audio_codec.as_ref())),
            plan.output_container
        );
        let job_id = format!("91{}", item.ordinal);
        let staging = directory.join(&job_id);
        fs::create_dir(&staging).unwrap();
        let staged = YtDlpDownloadExecutor::new(
            &resolver,
            &process,
            FfmpegMediaProcessor::new(&resolver, &process),
        )
        .download(
            DownloadExecutionRequest {
                job_id,
                source: inventory.source.clone(),
                playlist_item: None,
                instagram_selector: None,
                x_source: Some(analyzed.source().clone()),
                bilibili_source: None,
                plan: plan.clone(),
                video_source: video,
                audio_source: audio,
                staging_dir: staging,
            },
            CancellationToken::new(),
            &Progress,
        )
        .unwrap_or_else(|error| panic!("X download stage: {error:?}"));
        let verified = FfprobeOutputVerifier::new(&resolver, &process)
            .verify(staged, &plan, CancellationToken::new())
            .unwrap_or_else(|error| panic!("X output validation: {error:?}"));
        eprintln!(
            "X verified output: {} bytes, {}",
            verified.byte_length, verified.container
        );
    }
    assert!(
        videos > 0,
        "the explicitly supplied sample must contain a video"
    );
}
