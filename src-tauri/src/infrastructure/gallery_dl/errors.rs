use crate::{
    application::process::ProcessFailureKind,
    domain::{
        download::DownloadErrorCode,
        managed_tools::{ManagedToolError, ManagedToolErrorCode},
        media::AnalysisErrorCode,
    },
};

pub(super) fn analysis_tool(error: ManagedToolError) -> AnalysisErrorCode {
    match error.code {
        ManagedToolErrorCode::ToolMissing => AnalysisErrorCode::ToolMissing,
        ManagedToolErrorCode::ToolIntegrityFailed
        | ManagedToolErrorCode::InvalidToolManifest
        | ManagedToolErrorCode::ToolVersionMismatch => AnalysisErrorCode::ToolIntegrityFailed,
        ManagedToolErrorCode::UnsupportedTarget
        | ManagedToolErrorCode::ToolProbeFailed
        | ManagedToolErrorCode::ToolActivationFailed => AnalysisErrorCode::ProcessFailed,
    }
}

pub(super) fn download_tool(error: ManagedToolError) -> DownloadErrorCode {
    match error.code {
        ManagedToolErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        ManagedToolErrorCode::ToolIntegrityFailed
        | ManagedToolErrorCode::InvalidToolManifest
        | ManagedToolErrorCode::ToolVersionMismatch => DownloadErrorCode::ToolIntegrityFailed,
        ManagedToolErrorCode::UnsupportedTarget
        | ManagedToolErrorCode::ToolProbeFailed
        | ManagedToolErrorCode::ToolActivationFailed => DownloadErrorCode::ProcessFailed,
    }
}

pub(super) fn analysis_process(error: ProcessFailureKind) -> AnalysisErrorCode {
    match error {
        ProcessFailureKind::TimedOut | ProcessFailureKind::InactivityTimedOut => {
            AnalysisErrorCode::AnalysisTimeout
        }
        ProcessFailureKind::StdoutLimitExceeded
        | ProcessFailureKind::StderrLimitExceeded
        | ProcessFailureKind::LineLimitExceeded => AnalysisErrorCode::InvalidMetadata,
        ProcessFailureKind::LaunchFailed
        | ProcessFailureKind::JobControlFailed
        | ProcessFailureKind::IoFailed
        | ProcessFailureKind::Canceled => AnalysisErrorCode::ProcessFailed,
    }
}

pub(super) fn download_process(error: ProcessFailureKind) -> DownloadErrorCode {
    match error {
        ProcessFailureKind::TimedOut | ProcessFailureKind::InactivityTimedOut => {
            DownloadErrorCode::DownloadTimeout
        }
        ProcessFailureKind::StdoutLimitExceeded
        | ProcessFailureKind::StderrLimitExceeded
        | ProcessFailureKind::LineLimitExceeded => DownloadErrorCode::OutputInvalid,
        ProcessFailureKind::LaunchFailed
        | ProcessFailureKind::JobControlFailed
        | ProcessFailureKind::IoFailed
        | ProcessFailureKind::Canceled => DownloadErrorCode::ProcessFailed,
    }
}

pub(super) fn classify_analysis(stderr: &[u8]) -> AnalysisErrorCode {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if contains(
        &text,
        &[
            "username:",
            "password:",
            "login required",
            "authentication required",
            "use cookies",
            "use --cookies",
            "http redirect to login page",
        ],
    ) {
        AnalysisErrorCode::AuthenticationRequired
    } else if contains(
        &text,
        &["http error 429", "too many requests", "rate limit"],
    ) {
        AnalysisErrorCode::RateLimited
    } else if contains(&text, &["http error 403", "forbidden", "access denied"]) {
        AnalysisErrorCode::AccessDenied
    } else if contains(
        &text,
        &[
            "not found",
            "private",
            "content unavailable",
            "has been removed",
        ],
    ) {
        AnalysisErrorCode::ContentUnavailable
    } else if contains(
        &text,
        &[
            "keyerror",
            "extractor changed",
            "unsupported data",
            "please update gallery-dl",
        ],
    ) {
        AnalysisErrorCode::ExtractorOutdated
    } else if contains(
        &text,
        &[
            "name resolution",
            "connection refused",
            "network is unreachable",
            "certificate_verify_failed",
            "timed out",
        ],
    ) {
        AnalysisErrorCode::NetworkFailed
    } else {
        AnalysisErrorCode::Unknown
    }
}

pub(super) fn classify_download(stderr: &[u8]) -> DownloadErrorCode {
    match classify_analysis(stderr) {
        AnalysisErrorCode::AuthenticationRequired => DownloadErrorCode::AuthenticationRequired,
        AnalysisErrorCode::RateLimited => DownloadErrorCode::RateLimited,
        AnalysisErrorCode::AccessDenied => DownloadErrorCode::AccessDenied,
        AnalysisErrorCode::ContentUnavailable => DownloadErrorCode::ContentUnavailable,
        AnalysisErrorCode::ExtractorOutdated => DownloadErrorCode::ExtractorOutdated,
        AnalysisErrorCode::NetworkFailed => DownloadErrorCode::NetworkFailed,
        _ => DownloadErrorCode::Unknown,
    }
}

fn contains(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}
