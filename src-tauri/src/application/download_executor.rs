use std::{path::PathBuf, sync::Arc};

use crate::domain::{
    download::{DownloadErrorCode, DownloadProgress},
    format_selection::{ClipRange, SelectionOperation, SelectionPlan},
};

use super::{
    instagram_video::InstagramVideoSelector, media_url::ValidatedMediaUrl,
    process::CancellationToken,
};

pub struct DownloadExecutionRequest {
    pub job_id: String,
    pub source: ValidatedMediaUrl,
    pub playlist_item: Option<u16>,
    pub instagram_selector: Option<InstagramVideoSelector>,
    pub x_source: Option<super::x_video::VerifiedXDownloadSource>,
    pub bilibili_source: Option<super::bilibili_download::VerifiedBilibiliDownloadSource>,
    pub plan: SelectionPlan,
    pub video_source: Option<crate::domain::media::MediaFormat>,
    pub audio_source: Option<crate::domain::media::MediaFormat>,
    pub staging_dir: PathBuf,
}

pub struct StagedDownload {
    pub job_id: String,
    pub staging_dir: PathBuf,
    pub expected_operation: SelectionOperation,
    pub expected_container: Option<String>,
    pub requested_clip: Option<ClipRange>,
    pub evidence: super::local_processing::ProcessingEvidence,
}

pub trait DownloadProgressSink: Send + Sync {
    fn on_progress(&self, progress: DownloadProgress);
}

impl<T: DownloadProgressSink + ?Sized> DownloadProgressSink for &T {
    fn on_progress(&self, progress: DownloadProgress) {
        (**self).on_progress(progress);
    }
}

impl<T: DownloadProgressSink + ?Sized> DownloadProgressSink for Arc<T> {
    fn on_progress(&self, progress: DownloadProgress) {
        (**self).on_progress(progress);
    }
}

pub trait DownloadExecutor: Send + Sync {
    fn download(
        &self,
        request: DownloadExecutionRequest,
        cancellation: CancellationToken,
        progress: &dyn DownloadProgressSink,
    ) -> Result<StagedDownload, DownloadErrorCode>;
}
