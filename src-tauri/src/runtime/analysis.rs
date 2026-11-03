use std::{sync::Arc, time::Instant};

use crate::{
    application::{
        download_job::{DownloadInputResolver, ResolvedDownloadInput},
        live_analysis::{LiveAnalysisPreview, LiveAnalysisService},
    },
    domain::{
        download::DownloadErrorCode, format_selection::SelectionIntent, media::AnalysisErrorCode,
    },
    infrastructure::{
        process::WindowsProcessSupervisor, tools::FilesystemManagedToolStore,
        ytdlp::YtDlpMetadataAnalyzer,
    },
};

type ProductionAnalyzer =
    YtDlpMetadataAnalyzer<Arc<FilesystemManagedToolStore>, Arc<WindowsProcessSupervisor>>;

pub struct LiveAnalysisRuntime {
    service: LiveAnalysisService<ProductionAnalyzer>,
}

impl DownloadInputResolver for LiveAnalysisRuntime {
    fn resolve_download(
        &self,
        analysis_id: &str,
        intent: SelectionIntent,
    ) -> Result<ResolvedDownloadInput, DownloadErrorCode> {
        self.service
            .resolve_download(analysis_id, intent, Instant::now())
    }
}

impl LiveAnalysisRuntime {
    pub fn new(store: Arc<FilesystemManagedToolStore>) -> Self {
        let process = Arc::new(WindowsProcessSupervisor);
        Self {
            service: LiveAnalysisService::new(YtDlpMetadataAnalyzer::new(store, process)),
        }
    }

    pub fn analyze(
        &self,
        url: &str,
        intent: SelectionIntent,
    ) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
        self.service.analyze(url, intent, Instant::now())
    }

    pub fn preview(
        &self,
        analysis_id: &str,
        intent: SelectionIntent,
    ) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
        self.service.preview(analysis_id, intent, Instant::now())
    }

    pub fn open_media_page(&self, analysis_id: &str) -> Result<(), AnalysisErrorCode> {
        let source = self
            .service
            .resolve_media_page(analysis_id, Instant::now())?;
        crate::infrastructure::open_location::open_media_page(&source)
    }

    pub fn resolution_options(
        &self,
        analysis_id: &str,
        mode: crate::domain::format_selection::SelectionMode,
        compatibility_mode: bool,
    ) -> Result<crate::application::live_analysis::ResolutionOptions, AnalysisErrorCode> {
        self.service
            .resolution_options(analysis_id, mode, compatibility_mode, Instant::now())
    }
}
