//! Application state and composition responsibilities belong here as the runtime grows.

pub(crate) mod analysis;
pub(crate) mod bilibili;
pub(crate) mod destinations;
pub(crate) mod instagram;
pub(crate) mod jobs;
pub mod provisioning;
pub(crate) mod x;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use crate::{
    application::{
        destination::ApprovedDestination,
        download_activity::{DownloadActivityGate, DownloadQueueSnapshot},
        download_job::{DownloadResult, StartedDownload},
        download_queue_settings::DownloadQueueSettings,
        instagram_download_job::StartedInstagramDownload,
        live_analysis::LiveAnalysisPreview,
        preview_selection::{preview_selection, SelectionPreview},
        tool_bootstrap::ToolInventoryService,
        tool_provisioning::ToolProvisioner,
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::SelectionIntent,
        instagram::{
            InstagramAnalysisPreview, InstagramDownloadIntent, InstagramDownloadResult,
            InstagramPreviewPayload,
        },
        managed_tools::{ToolInventory, BUILD_TARGET},
        media::{AnalysisErrorCode, MediaInfo, Platform},
        tool_provisioning::{ToolProvisioningErrorCode, ToolProvisioningStatus},
    },
    infrastructure::{tools::FilesystemManagedToolStore, ytdlp::preview_fixture},
};

pub struct AppState {
    tool_store: Arc<FilesystemManagedToolStore>,
    live_analysis: Arc<analysis::LiveAnalysisRuntime>,
    instagram_analysis: Arc<instagram::InstagramAnalysisRuntime>,
    instagram_jobs: Arc<instagram::InstagramDownloadRuntime>,
    x_analysis: Arc<x::XAnalysisRuntime>,
    x_jobs: Arc<x::XDownloadRuntime>,
    bilibili_analysis: Arc<bilibili::BilibiliAnalysisRuntime>,
    bilibili_jobs: Arc<bilibili::BilibiliDownloadRuntime>,
    destinations: Arc<destinations::DestinationRuntime>,
    jobs: Arc<jobs::DownloadJobsRuntime>,
    provisioning: Arc<provisioning::ToolProvisioningRuntime>,
    media_gate: Arc<Mutex<()>>,
    download_activity: Arc<DownloadActivityGate>,
    queue_settings: Mutex<DownloadQueueSettings>,
    shutting_down: AtomicBool,
}

struct UnavailableProvisioner;

impl ToolProvisioner for UnavailableProvisioner {
    fn provision(
        &self,
        _mode: crate::application::tool_provisioning::ProvisioningMode,
    ) -> Result<ToolInventory, crate::domain::managed_tools::ManagedToolError> {
        Err(crate::domain::managed_tools::ManagedToolError::new(
            crate::domain::managed_tools::ManagedToolErrorCode::ToolMissing,
            "portable package is unavailable in this runtime",
        ))
    }
}

impl AppState {
    pub fn analyze_bilibili(
        &self,
        url: &str,
        intent: SelectionIntent,
    ) -> Result<crate::domain::bilibili::BilibiliAnalysisPreview, AnalysisErrorCode> {
        self.bilibili_analysis_readiness()?;
        self.bilibili_analysis.analyze(url, intent)
    }
    pub fn preview_bilibili(
        &self,
        id: &str,
        intent: SelectionIntent,
    ) -> Result<crate::domain::bilibili::BilibiliAnalysisPreview, AnalysisErrorCode> {
        self.bilibili_analysis_readiness()?;
        self.bilibili_analysis.preview(id, intent)
    }
    pub fn get_bilibili_resolution_options(
        &self,
        id: &str,
        mode: crate::domain::format_selection::SelectionMode,
        compatibility: bool,
    ) -> Result<crate::application::live_analysis::ResolutionOptions, AnalysisErrorCode> {
        self.bilibili_analysis_readiness()?;
        self.bilibili_analysis
            .resolution_options(id, mode, compatibility)
    }
    pub fn get_bilibili_cover(
        &self,
        id: &str,
    ) -> Result<crate::domain::bilibili::BilibiliCoverPayload, AnalysisErrorCode> {
        self.bilibili_analysis_readiness()?;
        self.bilibili_analysis.cover(id)
    }
    pub fn open_bilibili_source(&self, id: &str) -> Result<(), AnalysisErrorCode> {
        self.bilibili_analysis.open_source(id)
    }
    pub fn start_bilibili_download(
        &self,
        analysis: &str,
        destination: &str,
        intent: SelectionIntent,
    ) -> Result<StartedDownload, DownloadErrorCode> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        self.provisioning.status().download_readiness()?;
        let _gate = self
            .media_gate
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        self.provisioning.status().download_readiness()?;
        self.bilibili_jobs
            .start_download(analysis, destination, intent)
    }
    pub fn cancel_bilibili_download(&self, id: &str) -> Result<(), DownloadErrorCode> {
        self.bilibili_jobs.cancel(id)
    }
    pub fn bilibili_download_result(&self, id: &str) -> Result<DownloadResult, DownloadErrorCode> {
        self.bilibili_jobs.result(id)
    }
    pub fn open_bilibili_completed_location(&self, id: &str) -> Result<(), DownloadErrorCode> {
        self.bilibili_jobs.open_completed_location(id)
    }
    pub(crate) fn bilibili_analysis_handle(&self) -> Arc<bilibili::BilibiliAnalysisRuntime> {
        self.bilibili_analysis.clone()
    }
    pub(crate) fn bilibili_analysis_readiness(&self) -> Result<(), AnalysisErrorCode> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        self.provisioning.status().analysis_readiness()
    }
    pub fn analyze_x(
        &self,
        url: &str,
    ) -> Result<crate::domain::x::XAnalysisPreview, AnalysisErrorCode> {
        self.x_analysis_readiness()?;
        self.x_analysis.analyze(url)
    }
    pub fn get_x_preview(
        &self,
        analysis: &str,
        item: &str,
    ) -> Result<crate::domain::x::XPreviewPayload, AnalysisErrorCode> {
        self.x_analysis_readiness()?;
        self.x_analysis.load_preview(analysis, item)
    }
    pub fn open_x_source(&self, analysis: &str) -> Result<(), AnalysisErrorCode> {
        self.x_analysis.open_source(analysis)
    }
    pub fn start_x_download(
        &self,
        analysis: &str,
        destination: &str,
        intent: crate::domain::x::XDownloadIntent,
    ) -> Result<crate::application::x_download_job::StartedXDownload, DownloadErrorCode> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        self.provisioning.status().download_readiness()?;
        let _gate = self
            .media_gate
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        self.provisioning.status().download_readiness()?;
        let snapshot = self.x_analysis.resolve_download(analysis, intent)?;
        let destination = self.destinations.resolve(destination)?.into_path();
        self.x_jobs.start(snapshot, destination)
    }
    pub fn cancel_x_download(&self, job: &str) -> Result<(), DownloadErrorCode> {
        self.x_jobs.cancel(job)
    }
    pub fn x_download_result(
        &self,
        job: &str,
    ) -> Result<crate::domain::x::XDownloadResult, DownloadErrorCode> {
        self.x_jobs.result(job)
    }
    pub fn open_x_completed_location(&self, job: &str) -> Result<(), DownloadErrorCode> {
        self.x_jobs.open_completed_location(job)
    }
    pub(crate) fn x_analysis_handle(&self) -> Arc<x::XAnalysisRuntime> {
        self.x_analysis.clone()
    }
    pub(crate) fn x_analysis_readiness(&self) -> Result<(), AnalysisErrorCode> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        self.provisioning.status().analysis_readiness()
    }
    pub fn new(app_local_data: PathBuf) -> Self {
        let queue_settings =
            DownloadQueueSettings::new(app_local_data.join("settings/download-queue.json"));
        let tool_store = Arc::new(FilesystemManagedToolStore::new(
            app_local_data.join("managed-tools"),
            BUILD_TARGET,
        ));
        let live_analysis = Arc::new(analysis::LiveAnalysisRuntime::new(Arc::clone(&tool_store)));
        let instagram_analysis = Arc::new(instagram::InstagramAnalysisRuntime::new(
            app_local_data.join("instagram-analysis"),
            Arc::clone(&tool_store),
            None,
        ));
        let destinations = Arc::new(destinations::DestinationRuntime::new(
            app_local_data.clone(),
        ));
        let download_activity = Arc::new(DownloadActivityGate::new());
        let _ = download_activity.set_limit(queue_settings.load().unwrap_or(2));
        let jobs = Arc::new(jobs::DownloadJobsRuntime::without_app(
            app_local_data.clone(),
            Arc::clone(&tool_store),
            Arc::clone(&live_analysis),
            Arc::clone(&destinations),
            Arc::clone(&download_activity),
        ));
        let instagram_jobs = Arc::new(instagram::InstagramDownloadRuntime::without_app(
            app_local_data.clone(),
            Arc::clone(&tool_store),
            Arc::clone(&download_activity),
        ));
        let x_analysis = Arc::new(x::XAnalysisRuntime::new(tool_store.clone(), None));
        let x_jobs = Arc::new(x::XDownloadRuntime::new(
            None,
            app_local_data.clone(),
            tool_store.clone(),
            download_activity.clone(),
        ));
        let bilibili_analysis = Arc::new(bilibili::BilibiliAnalysisRuntime::new(
            tool_store.clone(),
            None,
        ));
        let bilibili_jobs = Arc::new(bilibili::BilibiliDownloadRuntime::new(
            None,
            app_local_data,
            tool_store.clone(),
            bilibili_analysis.clone(),
            destinations.clone(),
            download_activity.clone(),
        ));
        Self::compose(
            tool_store,
            live_analysis,
            (instagram_analysis, instagram_jobs),
            (x_analysis, x_jobs),
            (bilibili_analysis, bilibili_jobs),
            destinations,
            jobs,
            download_activity,
            Arc::new(UnavailableProvisioner),
            queue_settings,
        )
    }

    pub fn with_app_handle(
        app_local_data: PathBuf,
        app: tauri::AppHandle,
        tool_store: Arc<FilesystemManagedToolStore>,
        provisioner: Arc<dyn ToolProvisioner + Send + Sync>,
    ) -> Self {
        let queue_settings =
            DownloadQueueSettings::new(app_local_data.join("settings/download-queue.json"));
        let live_analysis = Arc::new(analysis::LiveAnalysisRuntime::new(Arc::clone(&tool_store)));
        let instagram_analysis = Arc::new(instagram::InstagramAnalysisRuntime::new(
            app_local_data.join("instagram-analysis"),
            Arc::clone(&tool_store),
            Some(app.clone()),
        ));
        let destinations = Arc::new(destinations::DestinationRuntime::new(
            app_local_data.clone(),
        ));
        let download_activity = Arc::new(DownloadActivityGate::new());
        let _ = download_activity.set_limit(queue_settings.load().unwrap_or(2));
        let jobs = Arc::new(jobs::DownloadJobsRuntime::new(
            app.clone(),
            app_local_data.clone(),
            Arc::clone(&tool_store),
            Arc::clone(&live_analysis),
            Arc::clone(&destinations),
            Arc::clone(&download_activity),
        ));
        let instagram_jobs = Arc::new(instagram::InstagramDownloadRuntime::new(
            app.clone(),
            app_local_data.clone(),
            Arc::clone(&tool_store),
            Arc::clone(&download_activity),
        ));
        let x_analysis = Arc::new(x::XAnalysisRuntime::new(
            tool_store.clone(),
            Some(app.clone()),
        ));
        let x_jobs = Arc::new(x::XDownloadRuntime::new(
            Some(app.clone()),
            app_local_data.clone(),
            tool_store.clone(),
            download_activity.clone(),
        ));
        let bilibili_analysis = Arc::new(bilibili::BilibiliAnalysisRuntime::new(
            tool_store.clone(),
            Some(app.clone()),
        ));
        let bilibili_jobs = Arc::new(bilibili::BilibiliDownloadRuntime::new(
            Some(app),
            app_local_data,
            tool_store.clone(),
            bilibili_analysis.clone(),
            destinations.clone(),
            download_activity.clone(),
        ));
        Self::compose(
            tool_store,
            live_analysis,
            (instagram_analysis, instagram_jobs),
            (x_analysis, x_jobs),
            (bilibili_analysis, bilibili_jobs),
            destinations,
            jobs,
            download_activity,
            provisioner,
            queue_settings,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn compose(
        tool_store: Arc<FilesystemManagedToolStore>,
        live_analysis: Arc<analysis::LiveAnalysisRuntime>,
        instagram: (
            Arc<instagram::InstagramAnalysisRuntime>,
            Arc<instagram::InstagramDownloadRuntime>,
        ),
        x: (Arc<x::XAnalysisRuntime>, Arc<x::XDownloadRuntime>),
        bilibili: (
            Arc<bilibili::BilibiliAnalysisRuntime>,
            Arc<bilibili::BilibiliDownloadRuntime>,
        ),
        destinations: Arc<destinations::DestinationRuntime>,
        jobs: Arc<jobs::DownloadJobsRuntime>,
        download_activity: Arc<DownloadActivityGate>,
        provisioner: Arc<dyn ToolProvisioner + Send + Sync>,
        queue_settings: DownloadQueueSettings,
    ) -> Self {
        let provisioning = Arc::new(provisioning::ToolProvisioningRuntime::new(
            provisioner,
            Arc::clone(&download_activity),
        ));
        Self {
            tool_store,
            live_analysis,
            instagram_analysis: instagram.0,
            destinations,
            jobs,
            instagram_jobs: instagram.1,
            x_analysis: x.0,
            x_jobs: x.1,
            bilibili_analysis: bilibili.0,
            bilibili_jobs: bilibili.1,
            provisioning,
            media_gate: Arc::new(Mutex::new(())),
            download_activity,
            queue_settings: Mutex::new(queue_settings),
            shutting_down: AtomicBool::new(false),
        }
    }

    pub fn tool_inventory(&self) -> ToolInventory {
        ToolInventoryService::new(self.tool_store.as_ref()).inventory()
    }

    pub fn preview_media_fixture(
        &self,
        platform: Platform,
    ) -> Result<MediaInfo, AnalysisErrorCode> {
        preview_fixture(platform)
    }

    pub fn preview_media_selection_fixture(
        &self,
        platform: Platform,
        intent: SelectionIntent,
    ) -> Result<SelectionPreview, AnalysisErrorCode> {
        preview_selection(preview_fixture(platform)?, intent)
    }

    pub fn analyze_media(
        &self,
        url: &str,
        intent: SelectionIntent,
    ) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
        self.provisioning.status().analysis_readiness()?;
        self.live_analysis.analyze(url, intent)
    }

    pub fn preview_media_selection(
        &self,
        analysis_id: &str,
        intent: SelectionIntent,
    ) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
        self.live_analysis.preview(analysis_id, intent)
    }

    pub(crate) fn live_analysis_handle(&self) -> Arc<analysis::LiveAnalysisRuntime> {
        Arc::clone(&self.live_analysis)
    }

    pub fn analyze_instagram(
        &self,
        url: &str,
    ) -> Result<InstagramAnalysisPreview, AnalysisErrorCode> {
        self.provisioning.status().analysis_readiness()?;
        self.instagram_analysis.analyze(url)
    }

    pub fn get_instagram_preview(
        &self,
        analysis_id: &str,
        item_id: &str,
    ) -> Result<InstagramPreviewPayload, AnalysisErrorCode> {
        self.provisioning.status().analysis_readiness()?;
        self.instagram_analysis.load_preview(analysis_id, item_id)
    }

    pub fn open_instagram_source(&self, analysis_id: &str) -> Result<(), AnalysisErrorCode> {
        self.instagram_analysis.open_source(analysis_id)
    }

    pub fn start_instagram_download(
        &self,
        analysis_id: &str,
        destination_id: &str,
        intent: InstagramDownloadIntent,
    ) -> Result<StartedInstagramDownload, DownloadErrorCode> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        self.provisioning.status().download_readiness()?;
        let _gate = self
            .media_gate
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        self.provisioning.status().download_readiness()?;
        let download = self
            .instagram_analysis
            .resolve_download(analysis_id, intent)?;
        let destination = self.destinations.resolve(destination_id)?.into_path();
        self.instagram_jobs.start(download, destination)
    }

    pub fn cancel_instagram_download(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.instagram_jobs.cancel(job_id)
    }

    pub fn instagram_download_result(
        &self,
        job_id: &str,
    ) -> Result<InstagramDownloadResult, DownloadErrorCode> {
        self.instagram_jobs.result(job_id)
    }

    pub fn open_instagram_completed_location(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.instagram_jobs.open_completed_location(job_id)
    }

    pub(crate) fn instagram_analysis_handle(&self) -> Arc<instagram::InstagramAnalysisRuntime> {
        Arc::clone(&self.instagram_analysis)
    }

    pub(crate) fn destination_handle(&self) -> Arc<destinations::DestinationRuntime> {
        Arc::clone(&self.destinations)
    }

    pub fn resolve_destination(
        &self,
        destination_id: &str,
    ) -> Result<ApprovedDestination, DownloadErrorCode> {
        self.destinations.resolve(destination_id)
    }

    pub fn start_download(
        &self,
        analysis_id: &str,
        destination_id: &str,
        intent: SelectionIntent,
    ) -> Result<StartedDownload, DownloadErrorCode> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        self.provisioning.status().download_readiness()?;
        let _gate = self
            .media_gate
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        self.provisioning.status().download_readiness()?;
        self.jobs
            .start_download(analysis_id, destination_id, intent)
    }

    pub fn start_tool_provisioning(
        &self,
    ) -> Result<ToolProvisioningStatus, ToolProvisioningErrorCode> {
        self.provisioning.start()
    }

    pub fn tool_provisioning_status(&self) -> ToolProvisioningStatus {
        self.provisioning.status()
    }

    pub(crate) fn repair_handles(
        &self,
    ) -> (Arc<provisioning::ToolProvisioningRuntime>, Arc<Mutex<()>>) {
        (Arc::clone(&self.provisioning), Arc::clone(&self.media_gate))
    }

    pub fn cancel_download(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.jobs.cancel_download(job_id)
    }

    pub fn download_queue(&self) -> Result<DownloadQueueSnapshot, DownloadErrorCode> {
        self.download_activity.snapshot()
    }
    pub fn set_download_concurrency(
        &self,
        limit: u8,
    ) -> Result<DownloadQueueSnapshot, DownloadErrorCode> {
        let settings = self
            .queue_settings
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        settings.save(limit)?;
        self.download_activity.set_limit(limit)?;
        self.download_activity.snapshot()
    }

    pub fn download_result(&self, job_id: &str) -> Result<DownloadResult, DownloadErrorCode> {
        self.jobs.download_result(job_id)
    }

    pub fn open_completed_location(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.jobs.open_completed_location(job_id)
    }

    /// Called explicitly before Tauri's process exit; managed state may not Drop.
    pub fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let _gate = self.media_gate.lock();
        self.download_activity.close();
        self.x_analysis.shutdown();
        self.bilibili_analysis.shutdown();
        self.bilibili_jobs.shutdown();
        self.x_jobs.shutdown();
        self.instagram_analysis.clear();
        self.instagram_jobs.shutdown();
        self.jobs.shutdown();
        self.provisioning.shutdown();
        self.destinations.clear();
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        managed_tools::{ToolInventory, ToolName, ToolState, ToolStatus, BUILD_TARGET},
        runtime_status::RuntimeStatus,
    };

    use super::AppState;

    #[test]
    fn runtime_status_derives_unconfigured_from_a_missing_inventory() {
        let status = RuntimeStatus::from_inventory("0.1.0", &ToolInventory::missing(BUILD_TARGET));

        assert!(!status.managed_tools_configured);
    }

    #[test]
    fn runtime_status_derives_configured_from_a_complete_ready_inventory() {
        let inventory = ToolInventory {
            target: BUILD_TARGET.to_owned(),
            tools: ToolName::required()
                .into_iter()
                .map(|name| ToolStatus {
                    name,
                    state: ToolState::Ready,
                    version: Some("8.0.1".to_owned()),
                })
                .collect(),
        };

        let status = RuntimeStatus::from_inventory("0.1.0", &inventory);

        assert!(status.managed_tools_configured);
    }

    #[test]
    fn clean_local_runtime_reports_missing_tools_without_a_real_package() {
        let tempdir = tempfile::tempdir().expect("temporary runtime directory");
        let app_state = AppState::new(tempdir.path().to_path_buf());

        let inventory = app_state.tool_inventory();
        assert_eq!(
            inventory
                .tools
                .iter()
                .map(|tool| tool.state)
                .collect::<Vec<_>>(),
            vec![
                ToolState::Missing,
                ToolState::Missing,
                ToolState::Missing,
                ToolState::Missing,
                ToolState::Missing,
            ]
        );
        assert!(!RuntimeStatus::from_inventory("0.1.0", &inventory).managed_tools_configured);
    }

    #[test]
    fn captured_bilibili_analysis_handle_cannot_reopen_after_shutdown() {
        use crate::domain::{
            format_selection::{SelectionIntent, SelectionMode},
            media::AnalysisErrorCode,
        };
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(root.path().to_owned());
        let captured = state.bilibili_analysis_handle();
        state.shutdown();
        let intent = SelectionIntent {
            mode: SelectionMode::BestAvailable,
            compatibility_mode: false,
            video_format_id: None,
            output_container: None,
            clip: None,
        };
        assert_eq!(
            captured
                .analyze(
                    "https://www.bilibili.com/video/BV1xx411c7mD",
                    intent.clone()
                )
                .err(),
            Some(AnalysisErrorCode::AnalysisUnavailable)
        );
        assert_eq!(
            captured.preview("1", intent).err(),
            Some(AnalysisErrorCode::AnalysisUnavailable)
        );
        assert_eq!(
            captured.cover("1").err(),
            Some(AnalysisErrorCode::AnalysisUnavailable)
        );
        assert!(captured.open_source("1").is_err());
    }
}
