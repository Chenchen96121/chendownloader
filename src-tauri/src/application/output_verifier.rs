use std::path::PathBuf;

use crate::domain::{download::DownloadErrorCode, format_selection::SelectionPlan};

use super::{download_executor::StagedDownload, process::CancellationToken};

#[derive(Debug, Eq, PartialEq)]
pub struct VerifiedOutput {
    pub path: PathBuf,
    pub container: String,
    pub has_video: bool,
    pub has_audio: bool,
    pub duration_ms: u64,
    pub byte_length: u64,
}

pub trait OutputVerifier: Send + Sync {
    fn verify(
        &self,
        staged: StagedDownload,
        plan: &SelectionPlan,
        cancellation: CancellationToken,
    ) -> Result<VerifiedOutput, DownloadErrorCode>;
}
