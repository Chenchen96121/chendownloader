use local_media_downloader_lib::{
    application::{
        destination::DestinationRegistry,
        destination_settings::{DefaultDestinationStatus, DestinationSettings},
    },
    infrastructure::destination::FilesystemDestinationValidator,
};
use std::{
    fs,
    time::{Duration, Instant},
};

#[test]
fn persists_across_restart_and_reapproves_after_expiry_without_exposing_paths() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("Private Downloads");
    fs::create_dir(&destination).unwrap();
    let config = root.path().join("settings/default-destination.json");
    let settings = DestinationSettings::new(config.clone(), FilesystemDestinationValidator);
    assert_eq!(settings.status(), DefaultDestinationStatus::Missing);
    settings.choose(Some(destination.clone())).unwrap().unwrap();
    let reopened = DestinationSettings::new(config, FilesystemDestinationValidator);
    let status = reopened.status();
    assert_eq!(
        status,
        DefaultDestinationStatus::Ready {
            display_name: "Private Downloads".into()
        }
    );
    assert!(!serde_json::to_string(&status)
        .unwrap()
        .contains(&root.path().to_string_lossy().to_string()));
    let registry = DestinationRegistry::new(FilesystemDestinationValidator);
    let now = Instant::now();
    let first = reopened.approve(&registry, now).unwrap();
    assert!(registry
        .resolve(&first.destination_id, now + Duration::from_secs(900))
        .is_err());
    let fresh = reopened
        .approve(&registry, now + Duration::from_secs(901))
        .unwrap();
    assert!(registry
        .resolve(&fresh.destination_id, now + Duration::from_secs(901))
        .is_ok());
}

#[test]
fn cancellation_invalid_destination_and_failed_persistence_preserve_previous_value() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("Downloads");
    fs::create_dir(&folder).unwrap();
    let config = root.path().join("default.json");
    let settings = DestinationSettings::new(config.clone(), FilesystemDestinationValidator);
    settings.choose(Some(folder.clone())).unwrap();
    let previous = fs::read(&config).unwrap();
    assert_eq!(settings.choose(None).unwrap(), None);
    assert!(settings.choose(Some(root.path().join("missing"))).is_err());
    assert_eq!(fs::read(&config).unwrap(), previous);
    let original = fs::metadata(&config).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&config, read_only).unwrap();
    assert!(settings.choose(Some(folder)).is_err());
    assert_eq!(fs::read(&config).unwrap(), previous);
    assert_eq!(
        settings.status(),
        DefaultDestinationStatus::Ready {
            display_name: "Downloads".into()
        }
    );
    fs::set_permissions(&config, original).unwrap();
}

#[test]
fn corrupt_unsupported_oversized_and_swapped_destinations_are_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("default.json");
    let settings = DestinationSettings::new(config.clone(), FilesystemDestinationValidator);
    for bytes in [
        b"not json".to_vec(),
        br#"{"version":2,"path":"C:\\Downloads"}"#.to_vec(),
        vec![b' '; 32769],
    ] {
        fs::write(&config, bytes).unwrap();
        assert_eq!(settings.status(), DefaultDestinationStatus::Unavailable);
    }
    let folder = root.path().join("Downloads");
    fs::create_dir(&folder).unwrap();
    settings.choose(Some(folder.clone())).unwrap();
    fs::remove_dir(&folder).unwrap();
    fs::write(&folder, b"swapped").unwrap();
    assert_eq!(settings.status(), DefaultDestinationStatus::Unavailable);
    assert!(settings
        .approve(
            &DestinationRegistry::new(FilesystemDestinationValidator),
            Instant::now()
        )
        .is_err());
}

#[test]
fn rejects_a_saved_folder_that_becomes_read_only() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("Downloads");
    fs::create_dir(&folder).unwrap();
    let settings = DestinationSettings::new(
        root.path().join("default.json"),
        FilesystemDestinationValidator,
    );
    settings.choose(Some(folder.clone())).unwrap();
    let original = fs::metadata(&folder).unwrap().permissions();
    let mut permissions = original.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&folder, permissions).unwrap();
    assert_eq!(settings.status(), DefaultDestinationStatus::Unavailable);
    assert!(settings
        .approve(
            &DestinationRegistry::new(FilesystemDestinationValidator),
            Instant::now()
        )
        .is_err());
    fs::set_permissions(&folder, original).unwrap();
}

#[cfg(windows)]
#[test]
fn rejects_reparse_config_and_saved_directory_when_symlinks_are_available() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::create_dir(&target).unwrap();
    let linked = root.path().join("linked");
    if std::os::windows::fs::symlink_dir(&target, &linked).is_err() {
        return;
    }
    let settings = DestinationSettings::new(
        root.path().join("settings.json"),
        FilesystemDestinationValidator,
    );
    assert!(settings.choose(Some(linked.clone())).is_err());
    let linked_settings =
        DestinationSettings::new(linked.join("settings.json"), FilesystemDestinationValidator);
    assert!(linked_settings.choose(Some(target)).is_err());
}
