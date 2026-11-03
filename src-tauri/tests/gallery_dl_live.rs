use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use local_media_downloader_lib::{
    application::{
        download_activity::DownloadActivityGate,
        download_job::{CompletedLocationOpener, JobStaging},
        instagram_analysis::InstagramAnalysisService,
        instagram_download_job::{
            InstagramDownloadEventEmitter, InstagramDownloadJobManager,
            InstagramDownloadJobServices, InstagramVideoDownloadPipeline,
        },
        tool_bootstrap::ToolBootstrapService,
    },
    domain::{
        download::DownloadErrorCode,
        instagram::{
            InstagramDownloadIntent, InstagramDownloadProgress, InstagramItemAvailability,
            InstagramJobState, InstagramMediaKind, InstagramSourceKind,
        },
        managed_tools::{ManagedToolManifest, BUILD_TARGET},
        media::AnalysisErrorCode,
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        gallery_dl::GalleryDlInstagramAdapter,
        output::{instagram::FilesystemInstagramOutputPublisher, FilesystemJobStaging},
        process::WindowsProcessSupervisor,
        tools::{FilesystemManagedToolStore, NativeToolVersionProbe},
        ytdlp::{download::YtDlpDownloadExecutor, YtDlpInstagramVideoAnalyzer},
    },
};

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

type Store = Arc<FilesystemManagedToolStore>;
type Process = Arc<WindowsProcessSupervisor>;
type GalleryAdapter = GalleryDlInstagramAdapter<Store, Process>;
type VideoAnalyzer = YtDlpInstagramVideoAnalyzer<Store, Process>;
type RealAnalysisService = InstagramAnalysisService<GalleryAdapter, VideoAnalyzer, GalleryAdapter>;

struct RealInstagramHarness {
    _root: tempfile::TempDir,
    store: Store,
    analysis: RealAnalysisService,
}

impl RealInstagramHarness {
    fn new() -> Self {
        let package = required_path("LMD_REAL_TOOL_PACKAGE");
        let manifest_bytes = fs::read(package.join("manifest.json"))
            .unwrap_or_else(|_| panic!("real-tool manifest could not be read"));
        let manifest: ManagedToolManifest = serde_json::from_slice(&manifest_bytes)
            .unwrap_or_else(|_| panic!("real-tool manifest is invalid"));
        let root = tempfile::tempdir()
            .unwrap_or_else(|_| panic!("Instagram live-test runtime unavailable"));
        let store = Arc::new(FilesystemManagedToolStore::new(
            root.path().join("managed-tools"),
            BUILD_TARGET,
        ));
        let process = WindowsProcessSupervisor;
        let probe = NativeToolVersionProbe::new(&process);
        ToolBootstrapService::new(store.as_ref(), &probe, BUILD_TARGET)
            .bootstrap(&package, &manifest)
            .unwrap_or_else(|_| panic!("real-tool bootstrap failed"));
        let process = Arc::new(WindowsProcessSupervisor);
        let analysis = InstagramAnalysisService::new_in(
            GalleryDlInstagramAdapter::new(Arc::clone(&store), Arc::clone(&process)),
            YtDlpInstagramVideoAnalyzer::new(Arc::clone(&store), Arc::clone(&process)),
            GalleryDlInstagramAdapter::new(Arc::clone(&store), process),
            root.path().join("instagram-analysis"),
        );
        Self {
            _root: root,
            store,
            analysis,
        }
    }
}

struct NoopInstagramEventEmitter;

impl InstagramDownloadEventEmitter for NoopInstagramEventEmitter {
    fn emit(&self, _progress: InstagramDownloadProgress) -> Result<(), DownloadErrorCode> {
        Ok(())
    }
}

struct NoopCompletedLocationOpener;

impl CompletedLocationOpener for NoopCompletedLocationOpener {
    fn open(&self, _path: &Path) -> Result<(), DownloadErrorCode> {
        Ok(())
    }
}

fn required_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("required Instagram live-test setting {name} is missing"))
}

fn required_url(name: &str) -> String {
    env::var(name)
        .unwrap_or_else(|_| panic!("required Instagram live-test setting {name} is missing"))
}

fn file_count(path: &Path) -> usize {
    fs::read_dir(path)
        .unwrap_or_else(|_| panic!("download output could not be inspected"))
        .map(|entry| {
            let path = entry
                .unwrap_or_else(|_| panic!("download output entry is unavailable"))
                .path();
            if path.is_dir() {
                file_count(&path)
            } else if path.is_file() {
                1
            } else {
                0
            }
        })
        .sum()
}

#[test]
#[ignore = "requires pinned tools and an explicit Instagram post URL currently redirected to sign-in"]
fn real_instagram_login_redirect_reports_authentication_required() {
    let harness = RealInstagramHarness::new();
    let result = harness.analysis.analyze(
        &required_url("LMD_INSTAGRAM_LOGIN_REDIRECT_URL"),
        Instant::now(),
    );
    assert_eq!(
        result.err(),
        Some(AnalysisErrorCode::AuthenticationRequired)
    );
}

#[test]
#[ignore = "requires a provisioned real tool package and an explicit public Instagram single-image URL"]
fn real_public_instagram_single_image_analysis_is_exact() {
    let harness = RealInstagramHarness::new();
    let preview = harness
        .analysis
        .analyze(
            &required_url("LMD_INSTAGRAM_SINGLE_IMAGE_URL"),
            Instant::now(),
        )
        .unwrap_or_else(|_| panic!("public Instagram single-image analysis failed"));

    assert_eq!(preview.source_kind, InstagramSourceKind::Post);
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].kind, InstagramMediaKind::Image);
    assert_eq!(
        preview.items[0].availability,
        InstagramItemAvailability::Ready
    );
}

#[test]
#[ignore = "requires a provisioned real tool package and an explicit public Instagram mixed-post URL"]
fn real_public_instagram_mixed_post_preserves_image_and_video_inventory() {
    let harness = RealInstagramHarness::new();
    let preview = harness
        .analysis
        .analyze(
            &required_url("LMD_INSTAGRAM_MIXED_POST_URL"),
            Instant::now(),
        )
        .unwrap_or_else(|_| panic!("public Instagram mixed-post analysis failed"));

    assert_eq!(preview.source_kind, InstagramSourceKind::Post);
    assert!(preview.items.len() >= 2);
    assert!(preview.items.iter().any(|item| {
        item.kind == InstagramMediaKind::Image
            && item.availability == InstagramItemAvailability::Ready
    }));
    assert!(preview.items.iter().any(|item| {
        item.kind == InstagramMediaKind::Video
            && item.availability == InstagramItemAvailability::Ready
    }));
}

#[test]
#[ignore = "requires a provisioned real tool package and an explicit public Instagram reel URL"]
fn real_public_instagram_reel_analysis_returns_one_ready_video() {
    let harness = RealInstagramHarness::new();
    let preview = harness
        .analysis
        .analyze(&required_url("LMD_INSTAGRAM_REEL_URL"), Instant::now())
        .unwrap_or_else(|_| panic!("public Instagram reel analysis failed"));

    assert_eq!(preview.source_kind, InstagramSourceKind::Reel);
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].kind, InstagramMediaKind::Video);
    assert_eq!(
        preview.items[0].availability,
        InstagramItemAvailability::Ready
    );
}

#[test]
#[ignore = "requires a provisioned real tool package and an explicit public Instagram single-image URL"]
fn real_public_instagram_single_image_download_uses_production_runtime() {
    let harness = RealInstagramHarness::new();
    let preview = harness
        .analysis
        .analyze(
            &required_url("LMD_INSTAGRAM_SINGLE_IMAGE_URL"),
            Instant::now(),
        )
        .unwrap_or_else(|_| panic!("public Instagram single-image analysis failed"));
    let item = preview
        .items
        .iter()
        .find(|item| {
            item.kind == InstagramMediaKind::Image
                && item.availability == InstagramItemAvailability::Ready
        })
        .unwrap_or_else(|| panic!("single-image sample did not expose a ready image"));
    let resolved = harness
        .analysis
        .resolve_download(
            &preview.analysis_id,
            InstagramDownloadIntent {
                item_ids: vec![item.item_id.clone()],
                compatibility_mode: false,
            },
            Instant::now(),
        )
        .unwrap_or_else(|_| panic!("public Instagram image selection failed"));
    let output = tempfile::tempdir()
        .unwrap_or_else(|_| panic!("Instagram live-test output directory unavailable"));
    let process = Arc::new(WindowsProcessSupervisor);
    let image = GalleryDlInstagramAdapter::new(Arc::clone(&harness.store), Arc::clone(&process));
    let analyzer =
        YtDlpInstagramVideoAnalyzer::new(Arc::clone(&harness.store), Arc::clone(&process));
    let processor = FfmpegMediaProcessor::new(Arc::clone(&harness.store), Arc::clone(&process));
    let executor =
        YtDlpDownloadExecutor::new(Arc::clone(&harness.store), Arc::clone(&process), processor);
    let verifier = FfprobeOutputVerifier::new(Arc::clone(&harness.store), process);
    let video = InstagramVideoDownloadPipeline::new(analyzer, executor, verifier);
    let staging: Arc<dyn JobStaging> = Arc::new(FilesystemJobStaging::new(
        harness._root.path().join("instagram-download-jobs"),
    ));
    let download = InstagramDownloadJobManager::new(InstagramDownloadJobServices {
        activity: Arc::new(DownloadActivityGate::new()),
        image: Arc::new(image),
        video: Arc::new(video),
        publisher: Arc::new(FilesystemInstagramOutputPublisher::new()),
        staging,
        emitter: Arc::new(NoopInstagramEventEmitter),
        opener: Arc::new(NoopCompletedLocationOpener),
    });
    let started = download
        .start(resolved, output.path().to_owned())
        .unwrap_or_else(|_| panic!("public Instagram image download did not start"));
    assert!(
        download.wait_for_idle(DOWNLOAD_TIMEOUT),
        "public Instagram image download timed out"
    );
    let result = download
        .result(&started.job_id)
        .unwrap_or_else(|error| panic!("public Instagram image download failed: {error:?}"));
    download.shutdown();

    assert_eq!(result.state, InstagramJobState::Completed);
    assert_eq!((result.completed_count, result.failed_count), (1, 0));
    assert_eq!(result.items.len(), 1);
    assert_eq!(file_count(output.path()), 1);
}
