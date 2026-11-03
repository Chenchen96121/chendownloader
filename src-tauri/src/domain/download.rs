const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Preparing,
    Downloading,
    Processing,
    Verifying,
    Completed,
    Failed,
    Canceled,
}

impl DownloadState {
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Preparing, Self::Downloading)
                | (Self::Downloading, Self::Processing)
                | (Self::Processing, Self::Verifying)
                | (Self::Verifying, Self::Completed)
                | (
                    Self::Preparing | Self::Downloading | Self::Processing | Self::Verifying,
                    Self::Failed | Self::Canceled
                )
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadErrorCode {
    InvalidRequest,
    AnalysisUnavailable,
    DestinationUnavailable,
    FormatUnavailable,
    DownloadInProgress,
    ToolMissing,
    ToolIntegrityFailed,
    AuthenticationRequired,
    AccessDenied,
    ContentUnavailable,
    ContentChanged,
    RateLimited,
    ExtractorOutdated,
    NetworkFailed,
    DownloadTimeout,
    ProcessFailed,
    OutputInvalid,
    ImageContentInvalid,
    ImageDimensionsMismatch,
    ImageSizeMismatch,
    ImageProbeFailed,
    DiskFull,
    PublicationFailed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum DownloadMessageKey {
    #[serde(rename = "download.preparing")]
    Preparing,
    #[serde(rename = "download.downloading")]
    Downloading,
    #[serde(rename = "download.processing")]
    Processing,
    #[serde(rename = "download.verifying")]
    Verifying,
    #[serde(rename = "download.completed")]
    Completed,
    #[serde(rename = "download.failed")]
    Failed,
    #[serde(rename = "download.canceled")]
    Canceled,
}

impl DownloadMessageKey {
    fn state(self) -> DownloadState {
        match self {
            Self::Preparing => DownloadState::Preparing,
            Self::Downloading => DownloadState::Downloading,
            Self::Processing => DownloadState::Processing,
            Self::Verifying => DownloadState::Verifying,
            Self::Completed => DownloadState::Completed,
            Self::Failed => DownloadState::Failed,
            Self::Canceled => DownloadState::Canceled,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub job_id: String,
    pub state: DownloadState,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
    pub speed_bytes_per_second: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub message_key: DownloadMessageKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DownloadProgressMetrics {
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
    pub speed_bytes_per_second: Option<f64>,
    pub eta_seconds: Option<u64>,
}

impl DownloadProgress {
    pub fn new(
        job_id: &str,
        state: DownloadState,
        metrics: DownloadProgressMetrics,
        message_key: DownloadMessageKey,
    ) -> Result<Self, DownloadErrorCode> {
        let DownloadProgressMetrics {
            downloaded_bytes,
            total_bytes,
            percent,
            speed_bytes_per_second,
            eta_seconds,
        } = metrics;
        if !valid_opaque_id(job_id)
            || message_key.state() != state
            || downloaded_bytes.is_some_and(|value| value > MAX_SAFE_INTEGER)
            || total_bytes.is_some_and(|value| value == 0 || value > MAX_SAFE_INTEGER)
            || percent.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
            || speed_bytes_per_second.is_some_and(|value| {
                !value.is_finite() || value < 0.0 || value > MAX_SAFE_INTEGER as f64
            })
            || eta_seconds.is_some_and(|value| value > 31_536_000)
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(Self {
            job_id: job_id.to_owned(),
            state,
            downloaded_bytes,
            total_bytes,
            percent,
            speed_bytes_per_second,
            eta_seconds,
            message_key,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationApproval {
    pub destination_id: String,
    pub display_name: String,
}

impl DestinationApproval {
    pub fn new(destination_id: String, display_name: String) -> Result<Self, DownloadErrorCode> {
        if !valid_opaque_id(&destination_id)
            || display_name.trim().is_empty()
            || display_name.chars().count() > 80
            || display_name.chars().any(char::is_control)
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(Self {
            destination_id,
            display_name,
        })
    }
}

pub fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|parsed| parsed > 0)
}
