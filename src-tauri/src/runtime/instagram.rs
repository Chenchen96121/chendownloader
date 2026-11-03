use std::{path::PathBuf, sync::Arc, time::Instant};

use tauri::Emitter;
use tauri_plugin_opener::OpenerExt;

use crate::{
    application::{
        download_activity::DownloadActivityGate,
        instagram_analysis::{InstagramAnalysisService, ResolvedInstagramDownload},
        instagram_download_job::{
            InstagramDownloadEventEmitter, InstagramDownloadJobManager,
            InstagramDownloadJobServices, InstagramVideoDownloadPipeline, StartedInstagramDownload,
        },
    },
    domain::{
        download::DownloadErrorCode,
        instagram::{
            InstagramAnalysisPreview, InstagramDownloadProgress, InstagramDownloadResult,
            InstagramPreviewPayload,
        },
        media::AnalysisErrorCode,
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        gallery_dl::{
            FallbackInstagramAssetAcquirer, FallbackInstagramInventoryExtractor,
            GalleryDlInstagramAdapter,
        },
        instagram_tool::{
            ToolFallbackInstagramInventoryExtractor, YtDlpInstagramInventoryExtractor,
        },
        instagram_web::{
            InstagramWebAssetAcquirer, InstagramWebInventoryExtractor, ReqwestInstagramWebTransport,
        },
        open_location::NativeCompletedLocationOpener,
        output::{instagram::FilesystemInstagramOutputPublisher, FilesystemJobStaging},
        process::WindowsProcessSupervisor,
        tools::FilesystemManagedToolStore,
        ytdlp::{download::YtDlpDownloadExecutor, YtDlpInstagramVideoAnalyzer},
    },
};

type Store = Arc<FilesystemManagedToolStore>;
type Process = Arc<WindowsProcessSupervisor>;
type GalleryAdapter = GalleryDlInstagramAdapter<Store, Process>;
type InventoryAdapter = FallbackInstagramInventoryExtractor<
    GalleryAdapter,
    ToolFallbackInstagramInventoryExtractor<
        InstagramWebInventoryExtractor<ReqwestInstagramWebTransport>,
        YtDlpInstagramInventoryExtractor<Store, Process>,
    >,
>;
type AssetAdapter = FallbackInstagramAssetAcquirer<
    GalleryAdapter,
    InstagramWebAssetAcquirer<ReqwestInstagramWebTransport, Store, Process>,
>;
type VideoAnalyzer = YtDlpInstagramVideoAnalyzer<Store, Process>;
type Service = InstagramAnalysisService<InventoryAdapter, VideoAnalyzer, AssetAdapter>;

pub const INSTAGRAM_DOWNLOAD_PROGRESS_EVENT: &str = "instagram-download-progress";

pub struct InstagramAnalysisRuntime {
    service: Service,
    app: Option<tauri::AppHandle>,
}

impl InstagramAnalysisRuntime {
    pub fn new(cache_root: PathBuf, store: Store, app: Option<tauri::AppHandle>) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        let inventory = FallbackInstagramInventoryExtractor::new(
            GalleryDlInstagramAdapter::new(Arc::clone(&store), Arc::clone(&process)),
            ToolFallbackInstagramInventoryExtractor::new(
                InstagramWebInventoryExtractor::new(ReqwestInstagramWebTransport::default()),
                YtDlpInstagramInventoryExtractor::new(Arc::clone(&store), Arc::clone(&process)),
            ),
        );
        let video = YtDlpInstagramVideoAnalyzer::new(Arc::clone(&store), Arc::clone(&process));
        let preview = FallbackInstagramAssetAcquirer::new(
            GalleryDlInstagramAdapter::new(Arc::clone(&store), Arc::clone(&process)),
            InstagramWebAssetAcquirer::new(ReqwestInstagramWebTransport::default(), store, process),
        );
        Self {
            service: InstagramAnalysisService::new_in(inventory, video, preview, cache_root),
            app,
        }
    }

    pub fn analyze(&self, url: &str) -> Result<InstagramAnalysisPreview, AnalysisErrorCode> {
        self.service.analyze(url, Instant::now())
    }

    pub fn load_preview(
        &self,
        analysis_id: &str,
        item_id: &str,
    ) -> Result<InstagramPreviewPayload, AnalysisErrorCode> {
        self.service
            .load_preview(analysis_id, item_id, Instant::now())
    }

    pub fn open_source(&self, analysis_id: &str) -> Result<(), AnalysisErrorCode> {
        let source = self.service.resolve_source(analysis_id, Instant::now())?;
        let app = self
            .app
            .as_ref()
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
        app.opener()
            .open_url(source.url(), None::<&str>)
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)
    }

    pub fn resolve_download(
        &self,
        analysis_id: &str,
        intent: crate::domain::instagram::InstagramDownloadIntent,
    ) -> Result<ResolvedInstagramDownload, DownloadErrorCode> {
        self.service
            .resolve_download(analysis_id, intent, Instant::now())
    }

    pub fn clear(&self) {
        self.service.clear();
    }
}

struct RuntimeInstagramEventEmitter {
    app: tauri::AppHandle,
}

impl InstagramDownloadEventEmitter for RuntimeInstagramEventEmitter {
    fn emit(&self, progress: InstagramDownloadProgress) -> Result<(), DownloadErrorCode> {
        self.app
            .emit(INSTAGRAM_DOWNLOAD_PROGRESS_EVENT, progress)
            .map_err(|_| DownloadErrorCode::Unknown)
    }
}

struct NoopInstagramEventEmitter;

impl InstagramDownloadEventEmitter for NoopInstagramEventEmitter {
    fn emit(&self, _progress: InstagramDownloadProgress) -> Result<(), DownloadErrorCode> {
        Ok(())
    }
}

pub struct InstagramDownloadRuntime {
    manager: Arc<InstagramDownloadJobManager>,
}

impl InstagramDownloadRuntime {
    pub fn new(
        app: tauri::AppHandle,
        app_local_data: PathBuf,
        tool_store: Store,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        Self::with_emitter(
            Arc::new(RuntimeInstagramEventEmitter { app }),
            app_local_data,
            tool_store,
            activity,
        )
    }

    pub fn without_app(
        app_local_data: PathBuf,
        tool_store: Store,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        Self::with_emitter(
            Arc::new(NoopInstagramEventEmitter),
            app_local_data,
            tool_store,
            activity,
        )
    }

    fn with_emitter(
        emitter: Arc<dyn InstagramDownloadEventEmitter>,
        app_local_data: PathBuf,
        tool_store: Store,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        let image = FallbackInstagramAssetAcquirer::new(
            GalleryDlInstagramAdapter::new(Arc::clone(&tool_store), Arc::clone(&process)),
            InstagramWebAssetAcquirer::new(
                ReqwestInstagramWebTransport::default(),
                Arc::clone(&tool_store),
                Arc::clone(&process),
            ),
        );
        let analyzer =
            YtDlpInstagramVideoAnalyzer::new(Arc::clone(&tool_store), Arc::clone(&process));
        let processor = FfmpegMediaProcessor::new(Arc::clone(&tool_store), Arc::clone(&process));
        let executor =
            YtDlpDownloadExecutor::new(Arc::clone(&tool_store), Arc::clone(&process), processor);
        let verifier = FfprobeOutputVerifier::new(tool_store, process);
        let video = InstagramVideoDownloadPipeline::new(analyzer, executor, verifier);
        let manager = InstagramDownloadJobManager::new(InstagramDownloadJobServices {
            activity,
            image: Arc::new(image),
            video: Arc::new(video),
            publisher: Arc::new(FilesystemInstagramOutputPublisher::new()),
            staging: Arc::new(FilesystemJobStaging::new(
                app_local_data.join("instagram-download-jobs"),
            )),
            emitter,
            opener: Arc::new(NativeCompletedLocationOpener),
        });
        Self { manager }
    }

    pub fn start(
        &self,
        download: ResolvedInstagramDownload,
        destination: PathBuf,
    ) -> Result<StartedInstagramDownload, DownloadErrorCode> {
        self.manager.start(download, destination)
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.manager.cancel(job_id)
    }

    pub fn result(&self, job_id: &str) -> Result<InstagramDownloadResult, DownloadErrorCode> {
        self.manager.result(job_id)
    }

    pub fn open_completed_location(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.manager.open_completed_location(job_id)
    }

    pub fn shutdown(&self) {
        self.manager.shutdown();
    }
}
