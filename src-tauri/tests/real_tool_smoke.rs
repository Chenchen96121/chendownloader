use std::{
    collections::HashSet,
    env, fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

use local_media_downloader_lib::{
    application::{
        destination::DestinationValidator,
        download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
        live_analysis::LiveAnalysisService,
        output_publisher::OutputPublisher,
        output_verifier::OutputVerifier,
        process::CancellationToken,
        tool_bootstrap::ToolBootstrapService,
    },
    domain::{
        download::{DownloadErrorCode, DownloadProgress},
        format_selection::{ClipRange, SelectionIntent, SelectionMode},
        managed_tools::{ManagedToolManifest, BUILD_TARGET},
    },
    infrastructure::{
        destination::FilesystemDestinationValidator,
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::{FfprobeOutputVerifier, CLIP_DURATION_TOLERANCE_MS},
        output::{FilesystemJobStaging, FilesystemOutputPublisher},
        process::WindowsProcessSupervisor,
        tools::{FilesystemManagedToolStore, NativeToolVersionProbe},
        ytdlp::{download::YtDlpDownloadExecutor, YtDlpMetadataAnalyzer},
    },
};

struct NoopProgress;

impl DownloadProgressSink for NoopProgress {
    fn on_progress(&self, _progress: DownloadProgress) {}
}

struct CancelOnProgress {
    cancellation: CancellationToken,
    observed: AtomicBool,
}

impl DownloadProgressSink for CancelOnProgress {
    fn on_progress(&self, _progress: DownloadProgress) {
        self.observed.store(true, Ordering::Release);
        self.cancellation.cancel();
    }
}

fn required_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("required real-tool smoke setting is missing"))
}

#[test]
#[ignore = "requires an explicitly provisioned real tool package, public media URL, and output directory"]
fn real_public_media_full_download_uses_production_services() {
    let package = required_path("LMD_REAL_TOOL_PACKAGE");
    let output = required_path("LMD_REAL_OUTPUT_DIR");
    let source_url = env::var("LMD_REAL_MEDIA_URL")
        .unwrap_or_else(|_| panic!("required real-tool smoke setting is missing"));
    let manifest_bytes = fs::read(package.join("manifest.json"))
        .unwrap_or_else(|_| panic!("real-tool manifest could not be read"));
    let manifest: ManagedToolManifest = serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|_| panic!("real-tool manifest is invalid"));

    let runtime = tempfile::tempdir().unwrap_or_else(|_| panic!("smoke runtime unavailable"));
    let store = Arc::new(FilesystemManagedToolStore::new(
        runtime.path().join("managed-tools"),
        BUILD_TARGET,
    ));
    let process = Arc::new(WindowsProcessSupervisor);
    let probe = NativeToolVersionProbe::new(process.as_ref());
    ToolBootstrapService::new(store.as_ref(), &probe, BUILD_TARGET)
        .bootstrap(&package, &manifest)
        .unwrap_or_else(|_| panic!("real-tool bootstrap failed"));

    let intent = SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::BestAvailable,
        video_format_id: None,
        output_container: None,
        clip: None,
    };
    let analysis = LiveAnalysisService::new(YtDlpMetadataAnalyzer::new(
        Arc::clone(&store),
        Arc::clone(&process),
    ));
    let preview = analysis
        .analyze(&source_url, intent.clone(), Instant::now())
        .unwrap_or_else(|_| panic!("real media analysis failed"));
    let resolved = analysis
        .resolve_download(&preview.analysis_id, intent, Instant::now())
        .unwrap_or_else(|_| panic!("real media selection failed"));
    let destination = FilesystemDestinationValidator
        .validate(&output)
        .unwrap_or_else(|_| panic!("real output destination is unavailable"));

    let staging = FilesystemJobStaging::new(runtime.path().join("download-jobs"));
    let staging_dir =
        local_media_downloader_lib::application::download_job::JobStaging::create(&staging, "1")
            .unwrap_or_else(|_| panic!("smoke staging unavailable"));
    let plan = resolved.plan.clone();
    let title = resolved.media.title.clone();
    let media_id = resolved.media.media_id.clone();
    let staged = YtDlpDownloadExecutor::new(
        Arc::clone(&store),
        Arc::clone(&process),
        FfmpegMediaProcessor::new(Arc::clone(&store), Arc::clone(&process)),
    )
    .download(
        DownloadExecutionRequest {
            job_id: "1".to_owned(),
            playlist_item: None,
            instagram_selector: None,
            tiktok_source: None,
            x_source: None,
            bilibili_source: None,
            video_source: resolved
                .media
                .formats
                .iter()
                .find(|row| Some(&row.id) == resolved.plan.video_format_id.as_ref())
                .cloned(),
            audio_source: resolved
                .media
                .formats
                .iter()
                .find(|row| Some(&row.id) == resolved.plan.audio_format_id.as_ref())
                .cloned(),
            source: resolved.source,
            plan: plan.clone(),
            staging_dir,
        },
        CancellationToken::new(),
        &NoopProgress,
    )
    .unwrap_or_else(|_| panic!("real media download failed"));
    let verified = FfprobeOutputVerifier::new(Arc::clone(&store), Arc::clone(&process))
        .verify(staged, &plan, CancellationToken::new())
        .unwrap_or_else(|_| panic!("real media verification failed"));
    let published = FilesystemOutputPublisher::new()
        .publish(
            verified,
            &destination,
            &title,
            &media_id,
            &CancellationToken::new(),
        )
        .unwrap_or_else(|_| panic!("real media publication failed"));
    assert!(
        published.final_path.is_file(),
        "real smoke output was not published"
    );
    local_media_downloader_lib::application::download_job::JobStaging::cleanup(&staging, "1")
        .unwrap_or_else(|_| panic!("smoke staging cleanup failed"));
}

#[test]
#[ignore = "requires an explicitly provisioned real tool package, public media URL, and output directory"]
fn real_public_media_clip_materializes_locally_and_publishes_verified_output() {
    let package = required_path("LMD_REAL_TOOL_PACKAGE");
    let output = required_path("LMD_REAL_OUTPUT_DIR");
    let source_url = env::var("LMD_REAL_MEDIA_URL")
        .unwrap_or_else(|_| panic!("required real-tool smoke setting is missing"));
    let manifest_bytes = fs::read(package.join("manifest.json"))
        .unwrap_or_else(|_| panic!("real-tool manifest could not be read"));
    let manifest: ManagedToolManifest = serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|_| panic!("real-tool manifest is invalid"));

    let runtime = tempfile::tempdir().unwrap_or_else(|_| panic!("smoke runtime unavailable"));
    let store = Arc::new(FilesystemManagedToolStore::new(
        runtime.path().join("managed-tools"),
        BUILD_TARGET,
    ));
    let process = Arc::new(WindowsProcessSupervisor);
    let probe = NativeToolVersionProbe::new(process.as_ref());
    ToolBootstrapService::new(store.as_ref(), &probe, BUILD_TARGET)
        .bootstrap(&package, &manifest)
        .unwrap_or_else(|_| panic!("real-tool bootstrap failed"));

    let clip = ClipRange {
        start_ms: 5_000,
        end_ms: 10_000,
    };
    let intent = SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::BestAvailable,
        video_format_id: None,
        output_container: None,
        clip: Some(clip),
    };
    let analysis = LiveAnalysisService::new(YtDlpMetadataAnalyzer::new(
        Arc::clone(&store),
        Arc::clone(&process),
    ));
    let preview = analysis
        .analyze(&source_url, intent.clone(), Instant::now())
        .unwrap_or_else(|_| panic!("real clip analysis failed"));
    let resolved = analysis
        .resolve_download(&preview.analysis_id, intent, Instant::now())
        .unwrap_or_else(|_| panic!("real clip selection failed"));
    let destination = FilesystemDestinationValidator
        .validate(&output)
        .unwrap_or_else(|_| panic!("real output destination is unavailable"));
    let destination_before: HashSet<_> = fs::read_dir(&destination)
        .unwrap_or_else(|_| panic!("real output directory could not be inspected"))
        .map(|entry| {
            entry
                .unwrap_or_else(|_| panic!("real output entry unavailable"))
                .file_name()
        })
        .collect();

    let staging = FilesystemJobStaging::new(runtime.path().join("download-jobs"));
    let staging_dir =
        local_media_downloader_lib::application::download_job::JobStaging::create(&staging, "1")
            .unwrap_or_else(|_| panic!("clip staging unavailable"));
    let plan = resolved.plan.clone();
    let title = resolved.media.title.clone();
    let media_id = resolved.media.media_id.clone();
    let staged = YtDlpDownloadExecutor::new(
        Arc::clone(&store),
        Arc::clone(&process),
        FfmpegMediaProcessor::new(Arc::clone(&store), Arc::clone(&process)),
    )
    .download(
        DownloadExecutionRequest {
            job_id: "1".to_owned(),
            playlist_item: None,
            instagram_selector: None,
            tiktok_source: None,
            x_source: None,
            bilibili_source: None,
            video_source: resolved
                .media
                .formats
                .iter()
                .find(|row| Some(&row.id) == resolved.plan.video_format_id.as_ref())
                .cloned(),
            audio_source: resolved
                .media
                .formats
                .iter()
                .find(|row| Some(&row.id) == resolved.plan.audio_format_id.as_ref())
                .cloned(),
            source: resolved.source,
            plan: plan.clone(),
            staging_dir: staging_dir.clone(),
        },
        CancellationToken::new(),
        &NoopProgress,
    )
    .unwrap_or_else(|_| panic!("real clip download failed"));
    assert!(
        !staging_dir.join("source").exists(),
        "full source remained after local clipping"
    );

    let verified = FfprobeOutputVerifier::new(Arc::clone(&store), Arc::clone(&process))
        .verify(staged, &plan, CancellationToken::new())
        .unwrap_or_else(|_| panic!("real clip verification failed"));
    assert!(verified.has_video, "verified clip has no video stream");
    assert!(verified.has_audio, "verified clip has no audio stream");
    assert!(verified.duration_ms > 0);
    assert!(verified.duration_ms <= clip.end_ms - clip.start_ms + CLIP_DURATION_TOLERANCE_MS);
    let published = FilesystemOutputPublisher::new()
        .publish(
            verified,
            &destination,
            &title,
            &media_id,
            &CancellationToken::new(),
        )
        .unwrap_or_else(|_| panic!("real clip publication failed"));
    assert!(
        published.final_path.is_file(),
        "real clip output was not published"
    );

    let destination_after: HashSet<_> = fs::read_dir(&destination)
        .unwrap_or_else(|_| panic!("real output directory could not be inspected"))
        .map(|entry| {
            entry
                .unwrap_or_else(|_| panic!("real output entry unavailable"))
                .file_name()
        })
        .collect();
    let created: Vec<_> = destination_after.difference(&destination_before).collect();
    assert_eq!(
        created.len(),
        1,
        "clip smoke must publish exactly one new file"
    );
    assert_eq!(created[0], published.final_path.file_name().unwrap());
    assert!(!created[0].to_string_lossy().starts_with("source."));

    local_media_downloader_lib::application::download_job::JobStaging::cleanup(&staging, "1")
        .unwrap_or_else(|_| panic!("clip staging cleanup failed"));
    assert!(!staging_dir.exists(), "clip staging remained after cleanup");
}

#[test]
#[ignore = "requires an explicitly provisioned real tool package, public media URL, and output directory"]
fn real_public_media_cancels_without_staging_or_published_partial_output() {
    let package = required_path("LMD_REAL_TOOL_PACKAGE");
    let output = required_path("LMD_REAL_OUTPUT_DIR");
    let source_url = env::var("LMD_REAL_MEDIA_URL")
        .unwrap_or_else(|_| panic!("required real-tool smoke setting is missing"));
    let manifest_bytes = fs::read(package.join("manifest.json"))
        .unwrap_or_else(|_| panic!("real-tool manifest could not be read"));
    let manifest: ManagedToolManifest = serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|_| panic!("real-tool manifest is invalid"));

    let runtime = tempfile::tempdir().unwrap_or_else(|_| panic!("smoke runtime unavailable"));
    let store = Arc::new(FilesystemManagedToolStore::new(
        runtime.path().join("managed-tools"),
        BUILD_TARGET,
    ));
    let process = Arc::new(WindowsProcessSupervisor);
    let probe = NativeToolVersionProbe::new(process.as_ref());
    ToolBootstrapService::new(store.as_ref(), &probe, BUILD_TARGET)
        .bootstrap(&package, &manifest)
        .unwrap_or_else(|_| panic!("real-tool bootstrap failed"));

    let analysis = LiveAnalysisService::new(YtDlpMetadataAnalyzer::new(
        Arc::clone(&store),
        Arc::clone(&process),
    ));
    let intent = SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::BestAvailable,
        video_format_id: None,
        output_container: None,
        clip: None,
    };
    let preview = analysis
        .analyze(&source_url, intent.clone(), Instant::now())
        .unwrap_or_else(|_| panic!("real media analysis failed"));
    let resolved = analysis
        .resolve_download(&preview.analysis_id, intent, Instant::now())
        .unwrap_or_else(|_| panic!("real media selection failed"));
    let published_before_cancel = fs::read_dir(&output)
        .unwrap_or_else(|_| panic!("real output directory could not be inspected"))
        .count();
    let staging = FilesystemJobStaging::new(runtime.path().join("download-jobs"));
    let staging_dir =
        local_media_downloader_lib::application::download_job::JobStaging::create(&staging, "1")
            .unwrap_or_else(|_| panic!("cancellation staging unavailable"));
    let cancellation = CancellationToken::new();
    let cancel_progress = CancelOnProgress {
        cancellation: cancellation.clone(),
        observed: AtomicBool::new(false),
    };
    let clipper = FfmpegMediaProcessor::new(Arc::clone(&store), Arc::clone(&process));
    let result = YtDlpDownloadExecutor::new(store, process, clipper).download(
        DownloadExecutionRequest {
            job_id: "1".to_owned(),
            playlist_item: None,
            instagram_selector: None,
            tiktok_source: None,
            x_source: None,
            bilibili_source: None,
            video_source: resolved
                .media
                .formats
                .iter()
                .find(|row| Some(&row.id) == resolved.plan.video_format_id.as_ref())
                .cloned(),
            audio_source: resolved
                .media
                .formats
                .iter()
                .find(|row| Some(&row.id) == resolved.plan.audio_format_id.as_ref())
                .cloned(),
            source: resolved.source,
            plan: resolved.plan,
            staging_dir,
        },
        cancellation,
        &cancel_progress,
    );
    assert_eq!(result.err(), Some(DownloadErrorCode::ProcessFailed));
    assert!(cancel_progress.observed.load(Ordering::Acquire));
    local_media_downloader_lib::application::download_job::JobStaging::cleanup(&staging, "1")
        .unwrap_or_else(|_| panic!("cancellation staging cleanup failed"));
    assert!(!runtime.path().join("download-jobs").join("1").exists());
    assert_eq!(
        fs::read_dir(&output)
            .unwrap_or_else(|_| panic!("real output directory could not be inspected"))
            .count(),
        published_before_cancel,
        "canceled smoke download must not publish a partial file"
    );
}
