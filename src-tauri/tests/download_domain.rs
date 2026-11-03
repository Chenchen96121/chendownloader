use local_media_downloader_lib::domain::download::{
    DestinationApproval, DownloadErrorCode, DownloadMessageKey, DownloadProgress,
    DownloadProgressMetrics, DownloadState,
};
use serde_json::{json, Value};

#[test]
fn public_states_and_errors_are_closed_snake_case_values() {
    let states = [
        DownloadState::Preparing,
        DownloadState::Downloading,
        DownloadState::Processing,
        DownloadState::Verifying,
        DownloadState::Completed,
        DownloadState::Failed,
        DownloadState::Canceled,
    ];
    assert_eq!(
        serde_json::to_value(states).unwrap(),
        json!([
            "preparing",
            "downloading",
            "processing",
            "verifying",
            "completed",
            "failed",
            "canceled"
        ])
    );

    let errors = [
        DownloadErrorCode::InvalidRequest,
        DownloadErrorCode::AnalysisUnavailable,
        DownloadErrorCode::DestinationUnavailable,
        DownloadErrorCode::FormatUnavailable,
        DownloadErrorCode::DownloadInProgress,
        DownloadErrorCode::ToolMissing,
        DownloadErrorCode::ToolIntegrityFailed,
        DownloadErrorCode::AuthenticationRequired,
        DownloadErrorCode::AccessDenied,
        DownloadErrorCode::ContentUnavailable,
        DownloadErrorCode::ContentChanged,
        DownloadErrorCode::RateLimited,
        DownloadErrorCode::ExtractorOutdated,
        DownloadErrorCode::NetworkFailed,
        DownloadErrorCode::DownloadTimeout,
        DownloadErrorCode::ProcessFailed,
        DownloadErrorCode::OutputInvalid,
        DownloadErrorCode::ImageContentInvalid,
        DownloadErrorCode::ImageDimensionsMismatch,
        DownloadErrorCode::ImageSizeMismatch,
        DownloadErrorCode::ImageProbeFailed,
        DownloadErrorCode::DiskFull,
        DownloadErrorCode::PublicationFailed,
        DownloadErrorCode::Unknown,
    ];
    assert_eq!(
        serde_json::to_value(errors).unwrap(),
        json!([
            "invalid_request",
            "analysis_unavailable",
            "destination_unavailable",
            "format_unavailable",
            "download_in_progress",
            "tool_missing",
            "tool_integrity_failed",
            "authentication_required",
            "access_denied",
            "content_unavailable",
            "content_changed",
            "rate_limited",
            "extractor_outdated",
            "network_failed",
            "download_timeout",
            "process_failed",
            "output_invalid",
            "image_content_invalid",
            "image_dimensions_mismatch",
            "image_size_mismatch",
            "image_probe_failed",
            "disk_full",
            "publication_failed",
            "unknown"
        ])
    );
}

#[test]
fn state_transitions_are_monotonic_and_terminal_states_are_final() {
    assert!(DownloadState::Preparing.can_transition_to(DownloadState::Downloading));
    assert!(DownloadState::Downloading.can_transition_to(DownloadState::Processing));
    assert!(DownloadState::Processing.can_transition_to(DownloadState::Verifying));
    assert!(DownloadState::Verifying.can_transition_to(DownloadState::Completed));
    for active in [
        DownloadState::Preparing,
        DownloadState::Downloading,
        DownloadState::Processing,
        DownloadState::Verifying,
    ] {
        assert!(active.can_transition_to(DownloadState::Failed));
        assert!(active.can_transition_to(DownloadState::Canceled));
    }
    assert!(!DownloadState::Preparing.can_transition_to(DownloadState::Completed));
    assert!(!DownloadState::Downloading.can_transition_to(DownloadState::Verifying));
    assert!(!DownloadState::Completed.can_transition_to(DownloadState::Failed));
    assert!(!DownloadState::Failed.can_transition_to(DownloadState::Canceled));
    assert!(!DownloadState::Canceled.can_transition_to(DownloadState::Preparing));
}

#[test]
fn progress_accepts_only_safe_correlated_numbers_and_message_keys() {
    let progress = DownloadProgress::new(
        "42",
        DownloadState::Downloading,
        DownloadProgressMetrics {
            downloaded_bytes: Some(250),
            total_bytes: Some(1000),
            percent: Some(25.0),
            speed_bytes_per_second: Some(125.5),
            eta_seconds: Some(6),
        },
        DownloadMessageKey::Downloading,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(progress).unwrap(),
        json!({
            "jobId": "42",
            "state": "downloading",
            "downloadedBytes": 250,
            "totalBytes": 1000,
            "percent": 25.0,
            "speedBytesPerSecond": 125.5,
            "etaSeconds": 6,
            "messageKey": "download.downloading"
        })
    );

    for id in ["", "0", "01", "-1", "+1", "1.0", "18446744073709551616"] {
        assert!(DownloadProgress::new(
            id,
            DownloadState::Preparing,
            DownloadProgressMetrics::default(),
            DownloadMessageKey::Preparing,
        )
        .is_err());
    }
    for percent in [f64::NAN, f64::INFINITY, -0.1, 100.1] {
        assert!(DownloadProgress::new(
            "1",
            DownloadState::Downloading,
            DownloadProgressMetrics {
                percent: Some(percent),
                ..DownloadProgressMetrics::default()
            },
            DownloadMessageKey::Downloading,
        )
        .is_err());
    }
    assert!(DownloadProgress::new(
        "1",
        DownloadState::Downloading,
        DownloadProgressMetrics {
            downloaded_bytes: Some(9_007_199_254_740_992),
            ..DownloadProgressMetrics::default()
        },
        DownloadMessageKey::Downloading,
    )
    .is_err());
    assert!(DownloadProgress::new(
        "1",
        DownloadState::Downloading,
        DownloadProgressMetrics::default(),
        DownloadMessageKey::Verifying,
    )
    .is_err());
}

#[test]
fn destination_approval_is_opaque_bounded_and_path_free() {
    let approval = DestinationApproval::new("7".to_owned(), "Downloads".to_owned()).unwrap();
    assert_eq!(
        serde_json::to_value(approval).unwrap(),
        json!({ "destinationId": "7", "displayName": "Downloads" })
    );
    for id in ["", "0", "01", "-1", "18446744073709551616"] {
        assert!(DestinationApproval::new(id.to_owned(), "Downloads".to_owned()).is_err());
    }
    for name in ["", "   ", "bad\nname"] {
        assert!(DestinationApproval::new("1".to_owned(), name.to_owned()).is_err());
    }
    assert!(DestinationApproval::new("1".to_owned(), "x".repeat(81)).is_err());
}

#[test]
fn public_payloads_have_no_path_url_or_raw_error_fields() {
    let progress = DownloadProgress::new(
        "1",
        DownloadState::Preparing,
        DownloadProgressMetrics::default(),
        DownloadMessageKey::Preparing,
    )
    .unwrap();
    let payloads = [
        serde_json::to_value(progress).unwrap(),
        serde_json::to_value(DestinationApproval::new("1".to_owned(), "Safe".to_owned()).unwrap())
            .unwrap(),
    ];
    for payload in payloads {
        let object = payload.as_object().unwrap();
        for field in object.keys() {
            let lower = field.to_ascii_lowercase();
            assert!(!lower.contains("path"));
            assert!(!lower.contains("url"));
            assert!(!lower.contains("error"));
        }
        assert!(!matches!(payload, Value::String(_)));
    }
}
