use std::path::{Path, PathBuf};

use crate::domain::download::DownloadErrorCode;

use super::{output_verifier::VerifiedOutput, process::CancellationToken};

#[derive(Debug, Eq, PartialEq)]
pub struct PublishedOutput {
    pub final_path: PathBuf,
    pub display_name: String,
}

pub trait OutputPublisher: Send + Sync {
    fn publish(
        &self,
        verified: VerifiedOutput,
        destination: &Path,
        safe_title: &str,
        media_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<PublishedOutput, DownloadErrorCode>;
}
