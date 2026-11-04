use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use local_media_downloader_lib::{
    application::destination::{
        approve_selected_destination, DestinationRegistry, DestinationValidator,
        DESTINATION_LIFETIME,
    },
    domain::download::DownloadErrorCode,
    infrastructure::destination::{sanitize_display_name, FilesystemDestinationValidator},
};

#[derive(Default)]
struct RecordingValidator {
    paths: Mutex<Vec<PathBuf>>,
}

impl DestinationValidator for RecordingValidator {
    fn validate(&self, path: &Path) -> Result<PathBuf, DownloadErrorCode> {
        self.paths.lock().unwrap().push(path.to_path_buf());
        Ok(path.to_path_buf())
    }
}

#[test]
fn canceled_picker_returns_none_without_replacing_current_approval() {
    let now = Instant::now();
    let registry = DestinationRegistry::new(RecordingValidator::default());
    let first = approve_selected_destination(&registry, Some(PathBuf::from("first")), now)
        .unwrap()
        .unwrap();
    assert_eq!(approve_selected_destination(&registry, None, now), Ok(None));
    assert_eq!(
        registry.resolve(&first.destination_id, now).unwrap().path(),
        Path::new("first")
    );
}

#[test]
fn validates_an_ordinary_directory_and_removes_its_write_probe() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("Downloads");
    fs::create_dir(&destination).unwrap();
    let validator = FilesystemDestinationValidator;
    let canonical = DestinationValidator::validate(&validator, &destination).unwrap();
    assert_eq!(canonical, fs::canonicalize(&destination).unwrap());
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
}

#[test]
fn rejects_missing_file_and_read_only_destinations() {
    let root = tempfile::tempdir().unwrap();
    let validator = FilesystemDestinationValidator;
    assert_eq!(
        DestinationValidator::validate(&validator, &root.path().join("missing")),
        Err(DownloadErrorCode::DestinationUnavailable)
    );

    let file = root.path().join("file.txt");
    fs::write(&file, b"x").unwrap();
    assert_eq!(
        DestinationValidator::validate(&validator, &file),
        Err(DownloadErrorCode::DestinationUnavailable)
    );

    let read_only = root.path().join("read-only");
    fs::create_dir(&read_only).unwrap();
    let original_permissions = fs::metadata(&read_only).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&read_only, permissions).unwrap();
    assert_eq!(
        DestinationValidator::validate(&validator, &read_only),
        Err(DownloadErrorCode::DestinationUnavailable)
    );
    fs::set_permissions(&read_only, original_permissions).unwrap();
}

#[cfg(windows)]
#[test]
fn rejects_symlink_or_reparse_point_directories() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let link = root.path().join("link");
    fs::create_dir(&target).unwrap();
    if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
        return;
    }
    assert_eq!(
        DestinationValidator::validate(&FilesystemDestinationValidator, &link),
        Err(DownloadErrorCode::DestinationUnavailable)
    );
}

#[test]
fn approval_ids_replace_expire_and_clear_without_exposing_paths() {
    let now = Instant::now();
    let registry = DestinationRegistry::new(RecordingValidator::default());
    let first = registry.approve(PathBuf::from("Secret One"), now).unwrap();
    let second = registry.approve(PathBuf::from("Secret Two"), now).unwrap();
    assert_eq!(first.destination_id, "1");
    assert_eq!(second.destination_id, "2");
    assert_eq!(first.display_name, "Secret One");
    assert_eq!(
        registry.resolve(&first.destination_id, now).err(),
        Some(DownloadErrorCode::DestinationUnavailable)
    );
    assert_eq!(
        registry
            .resolve(&second.destination_id, now)
            .unwrap()
            .path(),
        Path::new("Secret Two")
    );
    assert!(registry
        .resolve(
            &second.destination_id,
            now + DESTINATION_LIFETIME - Duration::from_nanos(1)
        )
        .is_ok());
    assert_eq!(
        registry
            .resolve(&second.destination_id, now + DESTINATION_LIFETIME)
            .err(),
        Some(DownloadErrorCode::DestinationUnavailable)
    );

    let third = registry
        .approve(PathBuf::from("Secret Three"), now)
        .unwrap();
    registry.clear();
    assert_eq!(
        registry.resolve(&third.destination_id, now).err(),
        Some(DownloadErrorCode::DestinationUnavailable)
    );
}

#[test]
fn platforms_approving_the_same_folder_do_not_invalidate_each_other() {
    let registry = DestinationRegistry::new(RecordingValidator::default());
    let now = Instant::now();
    let first = registry
        .approve(PathBuf::from("Shared folder"), now)
        .unwrap();
    let second = registry
        .approve(PathBuf::from("Shared folder"), now + Duration::from_secs(1))
        .unwrap();
    assert_eq!(first.destination_id, second.destination_id);
    assert!(registry
        .resolve(&first.destination_id, now + Duration::from_secs(2))
        .is_ok());
    assert!(registry
        .resolve(&second.destination_id, now + Duration::from_secs(2))
        .is_ok());
    // Another platform checking the folder must not extend an existing token's lifetime.
    assert!(registry
        .resolve(&first.destination_id, now + DESTINATION_LIFETIME)
        .is_err());
    let renewed = registry
        .approve(PathBuf::from("Shared folder"), now + DESTINATION_LIFETIME)
        .unwrap();
    assert_ne!(renewed.destination_id, first.destination_id);
    assert!(registry
        .resolve(&first.destination_id, now + DESTINATION_LIFETIME)
        .is_err());
}

#[test]
fn display_labels_remove_controls_bidi_marks_and_bound_length() {
    let label = sanitize_display_name(OsStr::new("  Down\nloads\u{202e} safe  "));
    assert_eq!(label, "Downloads safe");
    assert_eq!(
        sanitize_display_name(OsStr::new("\n\u{202e}")),
        "Selected folder"
    );
    assert_eq!(
        sanitize_display_name(OsStr::new(&"文".repeat(100)))
            .chars()
            .count(),
        80
    );
}

#[test]
fn resolve_rechecks_a_destination_swapped_after_approval() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let registry = DestinationRegistry::new(FilesystemDestinationValidator);
    let approval = registry
        .approve(destination.clone(), Instant::now())
        .unwrap();
    fs::remove_dir(&destination).unwrap();
    fs::write(&destination, b"swapped").unwrap();
    assert_eq!(
        registry
            .resolve(&approval.destination_id, Instant::now())
            .err(),
        Some(DownloadErrorCode::DestinationUnavailable)
    );
}
