use local_media_downloader_lib::{
    application::download_queue_settings::DownloadQueueSettings,
    domain::download::DownloadErrorCode,
};
use std::fs;

#[test]
fn concurrency_setting_defaults_to_two_and_survives_restart_without_changing_other_settings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("queue.json");
    let other = root.path().join("default-destination.json");
    fs::write(&other, b"existing destination").unwrap();
    let settings = DownloadQueueSettings::new(path.clone());
    assert_eq!(settings.load(), Ok(2));
    for value in [1, 4, 2, 3] {
        settings.save(value).unwrap();
        assert_eq!(DownloadQueueSettings::new(path.clone()).load(), Ok(value));
    }
    assert_eq!(fs::read(&other).unwrap(), b"existing destination");
    for invalid in [0, 5, 255] {
        assert_eq!(
            settings.save(invalid),
            Err(DownloadErrorCode::InvalidRequest)
        );
    }
    assert_eq!(settings.load(), Ok(3));
}

#[test]
fn malformed_oversize_and_unversioned_configuration_is_not_trusted() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("queue.json");
    for bytes in [
        br#"{"version":1,"limit":0}"#.to_vec(),
        br#"{"version":2,"limit":2}"#.to_vec(),
        br#"{"version":1,"limit":2,"url":"private"}"#.to_vec(),
        vec![b' '; 4097],
        b"null".to_vec(),
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(DownloadQueueSettings::new(path.clone()).load().is_err());
    }
}

#[test]
fn native_runtime_persists_the_queue_limit_and_rejects_open_command_fields() {
    let root = tempfile::tempdir().unwrap();
    let state = local_media_downloader_lib::runtime::AppState::new(root.path().to_owned());
    assert_eq!(state.download_queue().unwrap().limit, 2);
    assert_eq!(state.set_download_concurrency(4).unwrap().limit, 4);
    assert!(state.set_download_concurrency(5).is_err());
    drop(state);
    let next = local_media_downloader_lib::runtime::AppState::new(root.path().to_owned());
    assert_eq!(next.download_queue().unwrap().limit, 4);
    use local_media_downloader_lib::ipc::commands::DownloadConcurrencyRequest;
    assert!(
        serde_json::from_str::<DownloadConcurrencyRequest>(r#"{"limit":2,"url":"private"}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<DownloadConcurrencyRequest>(r#"{"limit":256}"#).is_err());
}

#[test]
fn explicit_app_shutdown_is_idempotent_and_rejects_new_native_downloads() {
    let root = tempfile::tempdir().unwrap();
    let state = local_media_downloader_lib::runtime::AppState::new(root.path().to_owned());
    state.shutdown();
    state.shutdown();
    use local_media_downloader_lib::domain::format_selection::{SelectionIntent, SelectionMode};
    let intent = SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::BestAvailable,
        video_format_id: None,
        output_container: None,
        clip: None,
    };
    assert_eq!(
        state.start_download("1", "2", intent).err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
}

#[cfg(windows)]
#[test]
fn reparse_configuration_cannot_read_or_overwrite_its_target_when_symlinks_are_available() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target.json");
    fs::write(&target, br#"{"version":1,"limit":4}"#).unwrap();
    let path = root.path().join("queue.json");
    if std::os::windows::fs::symlink_file(&target, &path).is_err() {
        return;
    }
    let settings = DownloadQueueSettings::new(path);
    assert!(settings.load().is_err());
    assert!(settings.save(1).is_err());
    assert_eq!(fs::read(target).unwrap(), br#"{"version":1,"limit":4}"#);
}
