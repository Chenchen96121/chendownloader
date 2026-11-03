#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProvisioningState {
    Preparing,
    Ready,
    Repairing,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProvisioningErrorCode {
    PackageMissing,
    PackageInvalid,
    ToolProbeFailed,
    ActivationFailed,
    Busy,
    DownloadInProgress,
}

impl ToolProvisioningErrorCode {
    pub fn is_fatal(self) -> bool {
        matches!(
            self,
            Self::PackageMissing
                | Self::PackageInvalid
                | Self::ToolProbeFailed
                | Self::ActivationFailed
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProvisioningStatus {
    state: ToolProvisioningState,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<ToolProvisioningErrorCode>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidToolProvisioningStatus;

impl ToolProvisioningStatus {
    pub fn new(
        state: ToolProvisioningState,
        error_code: Option<ToolProvisioningErrorCode>,
    ) -> Result<Self, InvalidToolProvisioningStatus> {
        let valid = match state {
            ToolProvisioningState::Failed => error_code.is_some_and(|code| code.is_fatal()),
            ToolProvisioningState::Preparing
            | ToolProvisioningState::Ready
            | ToolProvisioningState::Repairing => error_code.is_none(),
        };
        valid
            .then_some(Self { state, error_code })
            .ok_or(InvalidToolProvisioningStatus)
    }

    pub fn preparing() -> Self {
        Self {
            state: ToolProvisioningState::Preparing,
            error_code: None,
        }
    }

    pub fn ready() -> Self {
        Self {
            state: ToolProvisioningState::Ready,
            error_code: None,
        }
    }

    pub fn repairing() -> Self {
        Self {
            state: ToolProvisioningState::Repairing,
            error_code: None,
        }
    }

    pub fn failed(error_code: ToolProvisioningErrorCode) -> Self {
        debug_assert!(error_code.is_fatal());
        Self {
            state: ToolProvisioningState::Failed,
            error_code: Some(error_code),
        }
    }

    pub fn state(&self) -> ToolProvisioningState {
        self.state
    }

    pub fn error_code(&self) -> Option<ToolProvisioningErrorCode> {
        self.error_code
    }

    pub fn analysis_readiness(&self) -> Result<(), AnalysisErrorCode> {
        match (self.state, self.error_code) {
            (ToolProvisioningState::Ready, None) => Ok(()),
            (ToolProvisioningState::Preparing | ToolProvisioningState::Repairing, None)
            | (ToolProvisioningState::Failed, Some(ToolProvisioningErrorCode::PackageMissing)) => {
                Err(AnalysisErrorCode::ToolMissing)
            }
            (ToolProvisioningState::Failed, Some(_)) => Err(AnalysisErrorCode::ToolIntegrityFailed),
            _ => Err(AnalysisErrorCode::ToolIntegrityFailed),
        }
    }

    pub fn download_readiness(&self) -> Result<(), DownloadErrorCode> {
        self.analysis_readiness().map_err(|error| match error {
            AnalysisErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
            _ => DownloadErrorCode::ToolIntegrityFailed,
        })
    }
}
use super::{download::DownloadErrorCode, media::AnalysisErrorCode};
