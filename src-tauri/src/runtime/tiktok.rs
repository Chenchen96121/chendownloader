use crate::{
    application::{
        download_activity::DownloadActivityGate,
        tiktok_analysis::{ResolvedTikTokDownload, TikTokAnalysisService},
        tiktok_download_job::{
            StartedTikTokDownload, TikTokDownloadEventEmitter, TikTokDownloadJobManager,
            TikTokDownloadJobServices,
        },
    },
    domain::{
        download::DownloadErrorCode,
        media::AnalysisErrorCode,
        tiktok::{
            TikTokAnalysisPreview, TikTokDownloadIntent, TikTokDownloadProgress,
            TikTokDownloadResult, TikTokPreviewPayload,
        },
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        open_location::NativeCompletedLocationOpener,
        output::{tiktok::FilesystemTikTokOutputPublisher, FilesystemJobStaging},
        process::WindowsProcessSupervisor,
        tiktok::{
            assets::TikTokAssetAdapter, download::TikTokDownloadExecutor,
            inventory::TikTokInventoryAdapter, tool::TikTokToolResolver,
        },
        tools::FilesystemManagedToolStore,
        ytdlp::tiktok::{YtDlpTikTokMetadata, YtDlpTikTokVideoAnalyzer},
    },
};
use std::{path::PathBuf, sync::Arc, time::Instant};
use tauri::Emitter;
use tauri_plugin_opener::OpenerExt;

type Store = Arc<FilesystemManagedToolStore>;
type Process = Arc<WindowsProcessSupervisor>;
type Service = TikTokAnalysisService<
    TikTokInventoryAdapter,
    YtDlpTikTokVideoAnalyzer,
    TikTokAssetAdapter<Store, Process>,
>;
pub const TIKTOK_DOWNLOAD_PROGRESS_EVENT: &str = "tiktok-download-progress";

pub struct TikTokAnalysisRuntime {
    service: Service,
    app: Option<tauri::AppHandle>,
}
impl TikTokAnalysisRuntime {
    pub fn new(store: Store, app: Option<tauri::AppHandle>) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        let tool =
            Arc::new(TikTokToolResolver::from_executable().expect("embedded TikTok manifest"));
        let metadata: Arc<dyn crate::application::tiktok_source::TikTokMetadataExtractor> =
            Arc::new(YtDlpTikTokMetadata::new(store.clone(), process.clone()));
        let probe = Arc::new(
            crate::infrastructure::tiktok::probe::NativeTikTokCandidateProbe {
                resolver: store.clone(),
                process: process.clone(),
                tool: tool.clone(),
            },
        );
        Self {
            service: TikTokAnalysisService::new(
                TikTokInventoryAdapter {
                    tool: tool.clone(),
                    metadata: Some(metadata.clone()),
                },
                YtDlpTikTokVideoAnalyzer {
                    metadata: metadata.clone(),
                    probe: probe.clone(),
                },
                TikTokAssetAdapter::new(store, process, tool),
            ),
            app,
        }
    }
    pub fn analyze(&self, url: &str) -> Result<TikTokAnalysisPreview, AnalysisErrorCode> {
        self.service.analyze(url, Instant::now())
    }
    pub fn load_preview(
        &self,
        analysis: &str,
        item: &str,
    ) -> Result<TikTokPreviewPayload, AnalysisErrorCode> {
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
        intent: TikTokDownloadIntent,
    ) -> Result<ResolvedTikTokDownload, DownloadErrorCode> {
        self.service
            .resolve_download(analysis, intent, Instant::now())
    }
    pub fn shutdown(&self) {
        self.service.shutdown();
    }
}

struct RuntimeTikTokEventEmitter(Option<tauri::AppHandle>);
impl TikTokDownloadEventEmitter for RuntimeTikTokEventEmitter {
    fn emit(&self, progress: TikTokDownloadProgress) -> Result<(), DownloadErrorCode> {
        if let Some(app) = &self.0 {
            app.emit(TIKTOK_DOWNLOAD_PROGRESS_EVENT, progress)
                .map_err(|_| DownloadErrorCode::Unknown)?;
        }
        Ok(())
    }
}
pub struct TikTokDownloadRuntime {
    manager: Arc<TikTokDownloadJobManager>,
}
impl TikTokDownloadRuntime {
    pub fn new(
        app: Option<tauri::AppHandle>,
        app_local_data: PathBuf,
        store: Store,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        let tool =
            Arc::new(TikTokToolResolver::from_executable().expect("embedded TikTok manifest"));
        let metadata: Arc<dyn crate::application::tiktok_source::TikTokMetadataExtractor> =
            Arc::new(YtDlpTikTokMetadata::new(store.clone(), process.clone()));
        let processor = FfmpegMediaProcessor::new(store.clone(), process.clone());
        let probe = Arc::new(
            crate::infrastructure::tiktok::probe::NativeTikTokCandidateProbe {
                resolver: store.clone(),
                process: process.clone(),
                tool: tool.clone(),
            },
        );
        Self {
            manager: TikTokDownloadJobManager::new(TikTokDownloadJobServices {
                activity,
                inventory: Arc::new(TikTokInventoryAdapter {
                    tool: tool.clone(),
                    metadata: Some(metadata.clone()),
                }),
                image: Arc::new(TikTokAssetAdapter::new(
                    store.clone(),
                    process.clone(),
                    tool.clone(),
                )),
                video: Arc::new(YtDlpTikTokVideoAnalyzer { metadata, probe }),
                executor: Arc::new(TikTokDownloadExecutor { tool, processor }),
                verifier: Arc::new(FfprobeOutputVerifier::new(store, process)),
                staging: Arc::new(FilesystemJobStaging::new(
                    app_local_data.join("tiktok-download-jobs"),
                )),
                publisher: Arc::new(FilesystemTikTokOutputPublisher::new()),
                opener: Arc::new(NativeCompletedLocationOpener),
                emitter: Arc::new(RuntimeTikTokEventEmitter(app)),
            }),
        }
    }
    pub fn start(
        &self,
        download: ResolvedTikTokDownload,
        destination: PathBuf,
    ) -> Result<StartedTikTokDownload, DownloadErrorCode> {
        self.manager.start(download, destination)
    }
    pub fn cancel(&self, job: &str) -> Result<(), DownloadErrorCode> {
        self.manager.cancel(job)
    }
    pub fn result(&self, job: &str) -> Result<TikTokDownloadResult, DownloadErrorCode> {
        self.manager.result(job)
    }
    pub fn open_completed_location(&self, job: &str) -> Result<(), DownloadErrorCode> {
        self.manager.open_completed_location(job)
    }
    pub fn shutdown(&self) {
        self.manager.shutdown();
    }
}
