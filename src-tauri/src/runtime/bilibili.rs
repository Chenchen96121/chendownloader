use super::destinations::DestinationRuntime;
use crate::{
    application::{
        bilibili_analysis::BilibiliAnalysisService,
        bilibili_download::{
            BilibiliDownloadRefresher, BilibiliDownloadSnapshot, VerifiedBilibiliDownloadSource,
        },
        download_activity::DownloadActivityGate,
        download_job::{
            DownloadEventEmitter, DownloadInputResolver, DownloadJobManager, DownloadJobServices,
            DownloadResult, ResolvedDownloadInput, StartedDownload,
        },
        live_analysis::ResolutionOptions,
        process::CancellationToken,
    },
    domain::{
        bilibili::{BilibiliAnalysisPreview, BilibiliCoverPayload},
        download::{DownloadErrorCode, DownloadProgress},
        format_selection::{SelectionIntent, SelectionMode},
        media::AnalysisErrorCode,
    },
    infrastructure::{
        bilibili::{cover::HttpBilibiliCoverLoader, page::HttpBilibiliPageLoader},
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        open_location::NativeCompletedLocationOpener,
        output::{FilesystemJobStaging, FilesystemOutputPublisher},
        process::WindowsProcessSupervisor,
        tools::FilesystemManagedToolStore,
        ytdlp::{bilibili::YtDlpBilibiliAnalyzer, download::YtDlpDownloadExecutor},
    },
};
use std::{path::PathBuf, sync::Arc, time::Instant};
use tauri::Emitter;
use tauri_plugin_opener::OpenerExt;
type Store = Arc<FilesystemManagedToolStore>;
type Process = Arc<WindowsProcessSupervisor>;
type Service = BilibiliAnalysisService<
    YtDlpBilibiliAnalyzer<Store, Process, HttpBilibiliPageLoader>,
    HttpBilibiliCoverLoader<Store, Process>,
>;
pub const BILIBILI_DOWNLOAD_PROGRESS_EVENT: &str = "bilibili-download-progress";

pub struct BilibiliAnalysisRuntime {
    service: Service,
    app: Option<tauri::AppHandle>,
}
impl BilibiliAnalysisRuntime {
    pub fn new(store: Store, app: Option<tauri::AppHandle>) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        Self {
            service: BilibiliAnalysisService::new(
                YtDlpBilibiliAnalyzer::new(
                    store.clone(),
                    process.clone(),
                    HttpBilibiliPageLoader::default(),
                ),
                HttpBilibiliCoverLoader::new(store, process),
            ),
            app,
        }
    }
    pub fn analyze(
        &self,
        url: &str,
        intent: SelectionIntent,
    ) -> Result<BilibiliAnalysisPreview, AnalysisErrorCode> {
        self.service.analyze(url, intent, Instant::now())
    }
    pub fn preview(
        &self,
        id: &str,
        intent: SelectionIntent,
    ) -> Result<BilibiliAnalysisPreview, AnalysisErrorCode> {
        self.service.preview(id, intent, Instant::now())
    }
    pub fn resolution_options(
        &self,
        id: &str,
        mode: SelectionMode,
        compatibility: bool,
    ) -> Result<ResolutionOptions, AnalysisErrorCode> {
        self.service
            .resolution_options(id, mode, compatibility, Instant::now())
    }
    pub fn cover(&self, id: &str) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        self.service.cover(id, Instant::now())
    }
    pub fn open_source(&self, id: &str) -> Result<(), AnalysisErrorCode> {
        let source = self.service.resolve_source(id, Instant::now())?;
        self.app
            .as_ref()
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?
            .opener()
            .open_url(source.url(), None::<&str>)
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)
    }
    pub fn shutdown(&self) {
        self.service.shutdown();
    }
}
impl DownloadInputResolver for BilibiliAnalysisRuntime {
    fn resolve_download(
        &self,
        analysis: &str,
        intent: SelectionIntent,
    ) -> Result<ResolvedDownloadInput, DownloadErrorCode> {
        let snapshot = self
            .service
            .prepare_download(analysis, intent, Instant::now())?;
        Ok(ResolvedDownloadInput {
            source: snapshot.source().clone(),
            media: snapshot.media().clone(),
            plan: snapshot.plan().clone(),
            bilibili: Some(snapshot),
        })
    }
}
impl BilibiliDownloadRefresher for BilibiliAnalysisRuntime {
    fn refresh(
        &self,
        snapshot: &BilibiliDownloadSnapshot,
        cancellation: CancellationToken,
    ) -> Result<VerifiedBilibiliDownloadSource, DownloadErrorCode> {
        self.service.refresh(snapshot, cancellation)
    }
}
struct RuntimeEventEmitter(Option<tauri::AppHandle>);
impl DownloadEventEmitter for RuntimeEventEmitter {
    fn emit(&self, progress: DownloadProgress) -> Result<(), DownloadErrorCode> {
        if let Some(app) = &self.0 {
            app.emit(BILIBILI_DOWNLOAD_PROGRESS_EVENT, progress)
                .map_err(|_| DownloadErrorCode::Unknown)?;
        }
        Ok(())
    }
}
pub struct BilibiliDownloadRuntime {
    manager: Arc<DownloadJobManager>,
}
impl BilibiliDownloadRuntime {
    pub fn new(
        app: Option<tauri::AppHandle>,
        app_local_data: PathBuf,
        store: Store,
        analysis: Arc<BilibiliAnalysisRuntime>,
        destinations: Arc<DestinationRuntime>,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        let processor = FfmpegMediaProcessor::new(store.clone(), process.clone());
        let manager = DownloadJobManager::for_bilibili(
            DownloadJobServices {
                activity,
                input: analysis.clone(),
                destination: destinations,
                executor: Arc::new(YtDlpDownloadExecutor::new(
                    store.clone(),
                    process.clone(),
                    processor,
                )),
                verifier: Arc::new(FfprobeOutputVerifier::new(store, process)),
                publisher: Arc::new(FilesystemOutputPublisher::new()),
                staging: Arc::new(FilesystemJobStaging::new(
                    app_local_data.join("bilibili-download-jobs"),
                )),
                emitter: Arc::new(RuntimeEventEmitter(app)),
                opener: Arc::new(NativeCompletedLocationOpener),
            },
            analysis,
        );
        Self { manager }
    }
    pub fn start_download(
        &self,
        analysis: &str,
        destination: &str,
        intent: SelectionIntent,
    ) -> Result<StartedDownload, DownloadErrorCode> {
        self.manager.start_download(analysis, destination, intent)
    }
    pub fn cancel(&self, id: &str) -> Result<(), DownloadErrorCode> {
        self.manager.cancel_download(id)
    }
    pub fn result(&self, id: &str) -> Result<DownloadResult, DownloadErrorCode> {
        self.manager.download_result(id)
    }
    pub fn open_completed_location(&self, id: &str) -> Result<(), DownloadErrorCode> {
        self.manager.open_completed_location(id)
    }
    pub fn shutdown(&self) {
        self.manager.shutdown();
    }
}
