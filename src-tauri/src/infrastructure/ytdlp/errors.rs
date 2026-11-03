use crate::{
    application::process::ProcessFailureKind,
    domain::{
        managed_tools::{ManagedToolError, ManagedToolErrorCode},
        media::AnalysisErrorCode,
    },
};

pub(super) fn map_tool_error(error: ManagedToolError) -> AnalysisErrorCode {
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

pub(super) fn map_process_error(error: ProcessFailureKind) -> AnalysisErrorCode {
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

pub(super) fn classify_nonzero_exit(stderr: &[u8]) -> AnalysisErrorCode {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if contains_any(
        &text,
        &[
            "sign in to confirm",
            "login required",
            "authentication required",
            "use --cookies",
            "instagram sent an empty media response",
            "only available for registered users",
            "redirected to the login page",
        ],
    ) {
        AnalysisErrorCode::AuthenticationRequired
    } else if contains_any(
        &text,
        &["http error 429", "too many requests", "rate limit"],
    ) {
        AnalysisErrorCode::RateLimited
    } else if contains_any(&text, &["http error 403", "forbidden", "access denied"]) {
        AnalysisErrorCode::AccessDenied
    } else if contains_any(
        &text,
        &[
            "video unavailable",
            "video is unavailable",
            "video is private",
            "private video",
            "has been removed",
            "content is unavailable",
            "deleted video",
        ],
    ) {
        AnalysisErrorCode::ContentUnavailable
    } else if contains_any(
        &text,
        &[
            "update yt-dlp",
            "newer version of yt-dlp",
            "signature solving failed",
            "nsig extraction failed",
        ],
    ) {
        AnalysisErrorCode::ExtractorOutdated
    } else if transient_connection_failure(&text)
        || contains_any(
            &text,
            &[
                "unable to download webpage",
                "certificate_verify_failed",
                "name or service not known",
                "temporary failure in name resolution",
                "connection refused",
                "network is unreachable",
                "timed out",
            ],
        )
    {
        AnalysisErrorCode::NetworkFailed
    } else {
        AnalysisErrorCode::Unknown
    }
}

pub(super) fn retryable_x_metadata_failure(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    classify_nonzero_exit(stderr) == AnalysisErrorCode::NetworkFailed
        && transient_connection_failure(&text)
        && !text.contains("certificate")
}

fn transient_connection_failure(text: &str) -> bool {
    contains_any(
        text,
        &[
            "eof occurred in violation of protocol",
            "unexpected_eof_while_reading",
            "connection reset by peer",
            "connectionreseterror",
            "forcibly closed by the remote host",
            "ssl_connect: connection closed abruptly",
        ],
    )
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abrupt_curl_tls_disconnect_is_network_failure_but_denials_keep_priority() {
        let abrupt = b"Unable to download JSON metadata: curl: (35) BoringSSL SSL_connect: Connection closed abruptly (SSL_ERROR_SYSCALL)";
        assert_eq!(
            classify_nonzero_exit(abrupt),
            AnalysisErrorCode::NetworkFailed
        );
        assert!(retryable_x_metadata_failure(abrupt));
        assert_eq!(
            classify_nonzero_exit(
                b"HTTP Error 403: forbidden; SSL_connect: Connection closed abruptly"
            ),
            AnalysisErrorCode::AccessDenied
        );
        assert!(!retryable_x_metadata_failure(
            b"certificate_verify_failed: SSL_connect: Connection closed abruptly"
        ));
    }
}
