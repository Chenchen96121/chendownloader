use crate::{
    application::{
        download_activity::DownloadActivityGate,
        x_analysis::{ResolvedXDownload, XAnalysisService},
        x_download_job::{
            StartedXDownload, XDownloadEventEmitter, XDownloadJobManager, XDownloadJobServices,
        },
    },
    domain::{
        download::DownloadErrorCode,
        media::AnalysisErrorCode,
        x::{
            XAnalysisPreview, XDownloadIntent, XDownloadProgress, XDownloadResult, XPreviewPayload,
        },
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        open_location::NativeCompletedLocationOpener,
        output::{x::FilesystemXOutputPublisher, FilesystemJobStaging},
        process::WindowsProcessSupervisor,
        tools::FilesystemManagedToolStore,
        x::{assets::XAssetAdapter, GalleryDlXAdapter},
        ytdlp::{download::YtDlpDownloadExecutor, x::YtDlpXVideoAnalyzer},
    },
};
use std::{path::PathBuf, sync::Arc, time::Instant};
use tauri::Emitter;
use tauri_plugin_opener::OpenerExt;

type Store = Arc<FilesystemManagedToolStore>;
type Process = Arc<WindowsProcessSupervisor>;
type Service = XAnalysisService<
    GalleryDlXAdapter<Store, Process>,
    YtDlpXVideoAnalyzer<Store, Process>,
    XAssetAdapter<Store, Process>,
>;
pub const X_DOWNLOAD_PROGRESS_EVENT: &str = "x-download-progress";

pub struct XAnalysisRuntime {
    service: Service,
    app: Option<tauri::AppHandle>,
}
impl XAnalysisRuntime {
    pub fn new(store: Store, app: Option<tauri::AppHandle>) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        Self {
            service: XAnalysisService::new(
                GalleryDlXAdapter::new(store.clone(), process.clone()),
                YtDlpXVideoAnalyzer::new(store.clone(), process.clone()),
                XAssetAdapter::new(store, process),
            ),
            app,
        }
    }
    pub fn analyze(&self, url: &str) -> Result<XAnalysisPreview, AnalysisErrorCode> {
        self.service.analyze(url, Instant::now())
    }
    pub fn load_preview(
        &self,
        analysis: &str,
        item: &str,
    ) -> Result<XPreviewPayload, AnalysisErrorCode> {
        self.service.load_preview(analysis, item, Instant::now())
    }
    pub fn open_source(&self, analysis: &str) -> Result<(), AnalysisErrorCode> {
        let source = self.service.resolve_source(analysis, Instant::now())?;
        self.app
            .as_ref()
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?
            .opener()
            .open_url(source.url(), None::<&str>)
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)
    }
    pub fn resolve_download(
        &self,
        analysis: &str,
        intent: XDownloadIntent,
    ) -> Result<ResolvedXDownload, DownloadErrorCode> {
        self.service
            .resolve_download(analysis, intent, Instant::now())
    }
    pub fn shutdown(&self) {
        self.service.shutdown();
    }
}

struct RuntimeXEventEmitter(Option<tauri::AppHandle>);
impl XDownloadEventEmitter for RuntimeXEventEmitter {
    fn emit(&self, progress: XDownloadProgress) -> Result<(), DownloadErrorCode> {
        if let Some(app) = &self.0 {
            app.emit(X_DOWNLOAD_PROGRESS_EVENT, progress)
                .map_err(|_| DownloadErrorCode::Unknown)?;
        }
        Ok(())
    }
}
pub struct XDownloadRuntime {
    manager: Arc<XDownloadJobManager>,
}
impl XDownloadRuntime {
    pub fn new(
        app: Option<tauri::AppHandle>,
        app_local_data: PathBuf,
        store: Store,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        let processor = FfmpegMediaProcessor::new(store.clone(), process.clone());
        Self {
            manager: XDownloadJobManager::new(XDownloadJobServices {
                activity,
                inventory: Arc::new(GalleryDlXAdapter::new(store.clone(), process.clone())),
                image: Arc::new(XAssetAdapter::new(store.clone(), process.clone())),
                video: Arc::new(YtDlpXVideoAnalyzer::new(store.clone(), process.clone())),
                executor: Arc::new(YtDlpDownloadExecutor::new(
                    store.clone(),
                    process.clone(),
                    processor,
                )),
                verifier: Arc::new(FfprobeOutputVerifier::new(store, process)),
                staging: Arc::new(FilesystemJobStaging::new(
                    app_local_data.join("x-download-jobs"),
                )),
                publisher: Arc::new(FilesystemXOutputPublisher::new()),
                opener: Arc::new(NativeCompletedLocationOpener),
                emitter: Arc::new(RuntimeXEventEmitter(app)),
            }),
        }
    }
    pub fn start(
        &self,
        download: ResolvedXDownload,
        destination: PathBuf,
    ) -> Result<StartedXDownload, DownloadErrorCode> {
        self.manager.start(download, destination)
    }
    pub fn cancel(&self, job: &str) -> Result<(), DownloadErrorCode> {
        self.manager.cancel(job)
    }
    pub fn result(&self, job: &str) -> Result<XDownloadResult, DownloadErrorCode> {
        self.manager.result(job)
    }
    pub fn open_completed_location(&self, job: &str) -> Result<(), DownloadErrorCode> {
        self.manager.open_completed_location(job)
    }
    pub fn shutdown(&self) {
        self.manager.shutdown();
    }
}
