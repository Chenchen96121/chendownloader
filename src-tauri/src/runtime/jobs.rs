use std::{path::PathBuf, sync::Arc};

use tauri::Emitter;

use crate::{
    application::download_activity::DownloadActivityGate,
    application::download_job::{
        DownloadEventEmitter, DownloadJobManager, DownloadJobServices, DownloadResult,
        StartedDownload,
    },
    domain::{
        download::{DownloadErrorCode, DownloadProgress},
        format_selection::SelectionIntent,
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        open_location::NativeCompletedLocationOpener,
        output::{FilesystemJobStaging, FilesystemOutputPublisher},
        process::WindowsProcessSupervisor,
        tools::FilesystemManagedToolStore,
        ytdlp::download::YtDlpDownloadExecutor,
    },
};

use super::{analysis::LiveAnalysisRuntime, destinations::DestinationRuntime};

pub const DOWNLOAD_PROGRESS_EVENT: &str = "download-progress";

struct RuntimeEventEmitter {
    app: tauri::AppHandle,
}

impl DownloadEventEmitter for RuntimeEventEmitter {
    fn emit(&self, progress: DownloadProgress) -> Result<(), DownloadErrorCode> {
        self.app
            .emit(DOWNLOAD_PROGRESS_EVENT, progress)
            .map_err(|_| DownloadErrorCode::Unknown)
    }
}

struct NoopEventEmitter;

impl DownloadEventEmitter for NoopEventEmitter {
    fn emit(&self, _progress: DownloadProgress) -> Result<(), DownloadErrorCode> {
        Ok(())
    }
}

pub struct DownloadJobsRuntime {
    manager: Arc<DownloadJobManager>,
}

impl DownloadJobsRuntime {
    pub fn new(
        app: tauri::AppHandle,
        app_local_data: PathBuf,
        tool_store: Arc<FilesystemManagedToolStore>,
        live_analysis: Arc<LiveAnalysisRuntime>,
        destinations: Arc<DestinationRuntime>,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        Self::with_emitter(
            Arc::new(RuntimeEventEmitter { app }),
            app_local_data,
            tool_store,
            live_analysis,
            destinations,
            activity,
        )
    }

    pub fn without_app(
        app_local_data: PathBuf,
        tool_store: Arc<FilesystemManagedToolStore>,
        live_analysis: Arc<LiveAnalysisRuntime>,
        destinations: Arc<DestinationRuntime>,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        Self::with_emitter(
            Arc::new(NoopEventEmitter),
            app_local_data,
            tool_store,
            live_analysis,
            destinations,
            activity,
        )
    }

    fn with_emitter(
        emitter: Arc<dyn DownloadEventEmitter>,
        app_local_data: PathBuf,
        tool_store: Arc<FilesystemManagedToolStore>,
        live_analysis: Arc<LiveAnalysisRuntime>,
        destinations: Arc<DestinationRuntime>,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        let clipper = FfmpegMediaProcessor::new(Arc::clone(&tool_store), Arc::clone(&process));
        let manager = DownloadJobManager::new(DownloadJobServices {
            activity,
            input: live_analysis,
            destination: destinations,
            executor: Arc::new(YtDlpDownloadExecutor::new(
                Arc::clone(&tool_store),
                Arc::clone(&process),
                clipper,
            )),
            verifier: Arc::new(FfprobeOutputVerifier::new(tool_store, process)),
            publisher: Arc::new(FilesystemOutputPublisher::new()),
            staging: Arc::new(FilesystemJobStaging::new(
                app_local_data.join("download-jobs"),
            )),
            emitter,
            opener: Arc::new(NativeCompletedLocationOpener),
        });
        Self { manager }
    }

    pub fn start_download(
        &self,
        analysis_id: &str,
        destination_id: &str,
        intent: SelectionIntent,
    ) -> Result<StartedDownload, DownloadErrorCode> {
        self.manager
            .start_download(analysis_id, destination_id, intent)
    }

    pub fn cancel_download(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.manager.cancel_download(job_id)
    }

    pub fn download_result(&self, job_id: &str) -> Result<DownloadResult, DownloadErrorCode> {
        self.manager.download_result(job_id)
    }

    pub fn open_completed_location(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.manager.open_completed_location(job_id)
    }

    pub fn shutdown(&self) {
        self.manager.shutdown();
    }
}
