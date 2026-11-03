use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use local_media_downloader_lib::{
    application::{
        download_job::JobStaging,
        instagram_download_job::{
            InstagramOutputPublisher, InstagramPublicationRequest, InstagramVerifiedAsset,
        },
        output_publisher::OutputPublisher,
        output_verifier::VerifiedOutput,
        process::CancellationToken,
    },
    domain::{
        download::DownloadErrorCode,
        instagram::{
            InstagramItemAvailability, InstagramMediaItem, InstagramMediaKind, InstagramSourceKind,
        },
    },
    infrastructure::output::{
        cleanup_job_staging, instagram::FilesystemInstagramOutputPublisher, sanitize_output_stem,
        FilesystemJobStaging, FilesystemOutputPublisher, PublicationOperations,
        StdPublicationOperations,
    },
};

fn verified(source: PathBuf, length: u64) -> VerifiedOutput {
    VerifiedOutput {
        path: source,
        container: "mp4".to_owned(),
        has_video: true,
        has_audio: true,
        duration_ms: 1_000,
        byte_length: length,
    }
}

fn staged_file(root: &Path, job_id: &str, content: &[u8]) -> PathBuf {
    let staging = root.join(job_id);
    fs::create_dir(&staging).unwrap();
    let source = staging.join("stage.mp4");
    fs::write(&source, content).unwrap();
    fs::canonicalize(source).unwrap()
}

struct CancelAfterCommit {
    token: CancellationToken,
    cross_volume: bool,
}

impl PublicationOperations for CancelAfterCommit {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if self.cross_volume && source.file_name().is_some_and(|name| name == "stage.mp4") {
            return Err(io::Error::from_raw_os_error(17));
        }
        StdPublicationOperations.move_no_replace(source, destination)?;
        self.token.cancel();
        Ok(())
    }

    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
        StdPublicationOperations.copy_to_new_and_sync(source, destination)
    }

    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        StdPublicationOperations.remove_file_if_exists(path)
    }
}

#[test]
fn single_video_cancellation_stops_before_publication_but_preserves_committed_success() {
    for (pre_canceled, cross_volume) in [(true, false), (false, false), (false, true)] {
        let root = tempfile::tempdir().unwrap();
        let source = staged_file(root.path(), "42", b"verified");
        let destination = root.path().join("out");
        fs::create_dir(&destination).unwrap();
        let token = CancellationToken::new();
        if pre_canceled {
            token.cancel();
        }
        let publisher = FilesystemOutputPublisher::with_operations(CancelAfterCommit {
            token: token.clone(),
            cross_volume,
        });
        let result = publisher.publish(
            verified(source.clone(), 8),
            &destination,
            "Title",
            "media",
            &token,
        );
        if pre_canceled {
            assert_eq!(result, Err(DownloadErrorCode::ProcessFailed));
            assert_eq!(fs::read(&source).unwrap(), b"verified");
            assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
        } else {
            assert!(token.is_canceled());
            let published = result.unwrap();
            assert_eq!(fs::read(published.final_path).unwrap(), b"verified");
            assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
            assert!(!source.exists());
        }
    }
}

#[test]
fn instagram_cancellation_stops_before_publication_but_preserves_committed_success() {
    for (pre_canceled, cross_volume) in [(true, false), (false, false), (false, true)] {
        let root = tempfile::tempdir().unwrap();
        let source = instagram_staged_file(root.path(), "42", 1, "mp4", b"verified");
        let destination = root.path().join("out");
        fs::create_dir(&destination).unwrap();
        let token = CancellationToken::new();
        let publisher = FilesystemInstagramOutputPublisher::with_operations(CancelAfterCommit {
            token: token.clone(),
            cross_volume,
        });
        let publication = publisher
            .prepare(
                &destination,
                &instagram_request("42", InstagramSourceKind::Reel, 1),
            )
            .unwrap();
        if pre_canceled {
            token.cancel();
        }
        let result = publisher.publish(
            InstagramVerifiedAsset::new(source.clone(), "mp4", 8).unwrap(),
            &publication,
            &instagram_item(1, InstagramMediaKind::Video, "mp4"),
            &token,
        );
        if pre_canceled {
            assert_eq!(result, Err(DownloadErrorCode::ProcessFailed));
            assert_eq!(fs::read(&source).unwrap(), b"verified");
            assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
        } else {
            assert!(token.is_canceled());
            assert_eq!(
                fs::read(destination.join(result.unwrap())).unwrap(),
                b"verified"
            );
            assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
            assert!(!source.exists());
        }
    }
}

#[test]
fn concurrent_publication_move_has_exactly_one_winner_and_never_replaces_the_winner() {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };
    for round in 0..32 {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("same-name.mp4");
        let barrier = Arc::new(Barrier::new(8));
        let mut workers = vec![];
        for index in 0..8 {
            let source = root.path().join(format!("source-{index}.mp4"));
            let bytes = vec![index as u8; 32768];
            fs::write(&source, &bytes).unwrap();
            let target = destination.clone();
            let ready = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                ready.wait();
                (
                    index,
                    bytes,
                    StdPublicationOperations.move_no_replace(&source, &target),
                    source,
                )
            }));
        }
        let outcomes = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        let winners = outcomes
            .iter()
            .filter(|(_, _, result, _)| result.is_ok())
            .collect::<Vec<_>>();
        assert_eq!(
            winners.len(),
            1,
            "atomic no-replace failed in round {round}"
        );
        assert_eq!(fs::read(&destination).unwrap(), winners[0].1);
        for (_, bytes, outcome, source) in &outcomes {
            if let Err(error) = outcome {
                assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
                assert_eq!(fs::read(source).unwrap(), *bytes);
            }
        }
    }
}

fn instagram_staged_file(
    root: &Path,
    job_id: &str,
    ordinal: u16,
    extension: &str,
    content: &[u8],
) -> PathBuf {
    let staging = root.join(job_id).join(format!("{ordinal:02}"));
    fs::create_dir_all(&staging).unwrap();
    let source = staging.join(format!("stage.{extension}"));
    fs::write(&source, content).unwrap();
    fs::canonicalize(source).unwrap()
}

fn instagram_request(
    job_id: &str,
    kind: InstagramSourceKind,
    item_count: u16,
) -> InstagramPublicationRequest {
    InstagramPublicationRequest::new(
        job_id,
        kind,
        "Abc_123-",
        "A / demo: post",
        Some("creator".to_owned()),
        item_count,
    )
    .unwrap()
}

fn instagram_item(ordinal: u16, kind: InstagramMediaKind, container: &str) -> InstagramMediaItem {
    InstagramMediaItem {
        item_id: ordinal.to_string(),
        ordinal,
        kind,
        width: None,
        height: None,
        container: container.to_owned(),
        file_size: None,
        fps: None,
        video_bitrate_kbps: None,
        audio_bitrate_kbps: None,
        video_codec: None,
        audio_codec: None,
        dynamic_range: None,
        preview_available: false,
        availability: InstagramItemAvailability::Ready,
        unavailable_reason: None,
    }
}

#[test]
fn publishes_with_a_sanitized_chinese_ascii_filename_and_preserves_content() {
    let root = tempfile::tempdir().unwrap();
    let source = staged_file(root.path(), "42", b"verified bytes");
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();

    let published = FilesystemOutputPublisher::new()
        .publish(
            verified(source.clone(), 14),
            &destination,
            "  你好 / demo:<video>?...  ",
            "abc_123",
            &CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(published.display_name, "你好 demo video [abc_123].mp4");
    assert_eq!(
        published.final_path,
        fs::canonicalize(&destination)
            .unwrap()
            .join("你好 demo video [abc_123].mp4")
    );
    assert_eq!(fs::read(&published.final_path).unwrap(), b"verified bytes");
    assert!(!source.exists());
}

#[test]
fn sanitization_handles_reserved_names_trailing_dots_controls_and_empty_titles() {
    assert_eq!(sanitize_output_stem("CON. ", ""), "media");
    assert_eq!(sanitize_output_stem("NUL", "safe"), "NUL_ [safe]");
    assert_eq!(
        sanitize_output_stem("  my\u{0000}: file...  ", "id"),
        "my file [id]"
    );
    assert_eq!(sanitize_output_stem("", "id"), "media [id]");
    assert!(
        sanitize_output_stem(&"界".repeat(400), "id")
            .encode_utf16()
            .count()
            <= 180
    );
}

#[test]
fn existing_files_are_never_overwritten_and_collision_suffixes_are_deterministic() {
    let root = tempfile::tempdir().unwrap();
    let source = staged_file(root.path(), "42", b"new");
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("Title [media].mp4"), b"old zero").unwrap();
    fs::write(destination.join("Title [media] (1).mp4"), b"old one").unwrap();

    let published = FilesystemOutputPublisher::new()
        .publish(
            verified(source, 3),
            &destination,
            "Title",
            "media",
            &CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(published.display_name, "Title [media] (2).mp4");
    assert_eq!(
        fs::read(destination.join("Title [media].mp4")).unwrap(),
        b"old zero"
    );
    assert_eq!(
        fs::read(destination.join("Title [media] (1).mp4")).unwrap(),
        b"old one"
    );
    assert_eq!(fs::read(published.final_path).unwrap(), b"new");
}

struct CrossVolumeOnce {
    moves: AtomicUsize,
    standard: StdPublicationOperations,
}

impl CrossVolumeOnce {
    fn new() -> Self {
        Self {
            moves: AtomicUsize::new(0),
            standard: StdPublicationOperations,
        }
    }
}

impl PublicationOperations for CrossVolumeOnce {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if self.moves.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(io::Error::from_raw_os_error(17))
        } else {
            self.standard.move_no_replace(source, destination)
        }
    }

    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
        self.standard.copy_to_new_and_sync(source, destination)
    }

    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        self.standard.remove_file_if_exists(path)
    }
}

struct CrossVolumeCollision {
    moves: AtomicUsize,
}
impl PublicationOperations for CrossVolumeCollision {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        match self.moves.fetch_add(1, Ordering::SeqCst) {
            0 => Err(io::Error::from_raw_os_error(17)),
            1 => {
                fs::write(destination, b"other task")?;
                Err(io::Error::from(io::ErrorKind::AlreadyExists))
            }
            _ => StdPublicationOperations.move_no_replace(source, destination),
        }
    }
    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
        StdPublicationOperations.copy_to_new_and_sync(source, destination)
    }
    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        StdPublicationOperations.remove_file_if_exists(path)
    }
}

#[test]
fn instagram_cross_volume_final_collision_retries_without_touching_another_task() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let source = instagram_staged_file(root.path(), "42", 1, "jpg", b"my asset");
    let publisher = FilesystemInstagramOutputPublisher::with_operations(CrossVolumeCollision {
        moves: AtomicUsize::new(0),
    });
    let publication = publisher
        .prepare(
            &destination,
            &instagram_request("42", InstagramSourceKind::Post, 1),
        )
        .unwrap();
    let name = publisher
        .publish(
            InstagramVerifiedAsset::new(source, "jpg", 8).unwrap(),
            &publication,
            &instagram_item(1, InstagramMediaKind::Image, "jpg"),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(name, "01 (1).jpg");
    assert_eq!(fs::read(destination.join("01.jpg")).unwrap(), b"other task");
    assert_eq!(fs::read(destination.join(name)).unwrap(), b"my asset");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 2);
}

#[test]
fn cross_volume_fallback_copies_flushes_and_atomically_publishes() {
    let root = tempfile::tempdir().unwrap();
    let source = staged_file(root.path(), "42", b"cross-volume");
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();

    let publisher = FilesystemOutputPublisher::with_operations(CrossVolumeOnce::new());
    let published = publisher
        .publish(
            verified(source.clone(), 12),
            &destination,
            "Title",
            "media",
            &CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(fs::read(published.final_path).unwrap(), b"cross-volume");
    assert!(!source.exists());
    assert_eq!(
        fs::read_dir(&destination).unwrap().count(),
        1,
        "destination-side temporary file must be removed"
    );
}

enum FailurePoint {
    InitialMove,
    Copy,
    FinalMove,
}

struct FailingOperations {
    point: FailurePoint,
    moves: AtomicUsize,
    standard: StdPublicationOperations,
}

impl FailingOperations {
    fn new(point: FailurePoint) -> Self {
        Self {
            point,
            moves: AtomicUsize::new(0),
            standard: StdPublicationOperations,
        }
    }
}

impl PublicationOperations for FailingOperations {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        let call = self.moves.fetch_add(1, Ordering::SeqCst);
        match (&self.point, call) {
            (FailurePoint::InitialMove, 0) => {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "private"))
            }
            (FailurePoint::Copy | FailurePoint::FinalMove, 0) => {
                Err(io::Error::from_raw_os_error(17))
            }
            (FailurePoint::FinalMove, 1) => {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "private"))
            }
            _ => self.standard.move_no_replace(source, destination),
        }
    }

    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if matches!(self.point, FailurePoint::Copy) {
            fs::write(destination, b"partial")?;
            Err(io::Error::new(io::ErrorKind::WriteZero, "private"))
        } else {
            self.standard.copy_to_new_and_sync(source, destination)
        }
    }

    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        self.standard.remove_file_if_exists(path)
    }
}

#[test]
fn move_copy_and_final_rename_failures_publish_nothing_and_remove_temporary_files() {
    for point in [
        FailurePoint::InitialMove,
        FailurePoint::Copy,
        FailurePoint::FinalMove,
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = staged_file(root.path(), "42", b"verified");
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let result = FilesystemOutputPublisher::with_operations(FailingOperations::new(point))
            .publish(
                verified(source.clone(), 8),
                &destination,
                "Title",
                "media",
                &CancellationToken::new(),
            );
        assert_eq!(result.err(), Some(DownloadErrorCode::PublicationFailed));
        assert!(
            source.exists(),
            "verified staging source must remain recoverable"
        );
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    }
}

#[test]
fn rejects_invalid_verified_metadata_destination_and_media_identifiers() {
    let root = tempfile::tempdir().unwrap();
    let source = staged_file(root.path(), "42", b"verified");
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();

    let invalid = [
        verified(source.clone(), 0),
        VerifiedOutput {
            container: "../mp4".to_owned(),
            ..verified(source.clone(), 8)
        },
        VerifiedOutput {
            path: root.path().join("missing.mp4"),
            ..verified(source.clone(), 8)
        },
    ];
    for value in invalid {
        assert_eq!(
            FilesystemOutputPublisher::new()
                .publish(
                    value,
                    &destination,
                    "Title",
                    "media",
                    &CancellationToken::new()
                )
                .err(),
            Some(DownloadErrorCode::OutputInvalid)
        );
    }
    assert_eq!(
        FilesystemOutputPublisher::new()
            .publish(
                verified(source, 8),
                &destination,
                "Title",
                "../escape",
                &CancellationToken::new()
            )
            .err(),
        Some(DownloadErrorCode::OutputInvalid)
    );
}

#[test]
fn cleanup_is_confined_to_one_valid_direct_child_job_directory() {
    let root = tempfile::tempdir().unwrap();
    let staging_root = root.path().join("jobs");
    let outside = root.path().join("outside");
    fs::create_dir(&staging_root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), b"keep").unwrap();
    fs::create_dir(staging_root.join("42")).unwrap();
    fs::write(staging_root.join("42").join("partial"), b"partial").unwrap();

    cleanup_job_staging(&staging_root, "42").unwrap();
    assert!(!staging_root.join("42").exists());
    assert!(outside.join("keep.txt").exists());

    assert_eq!(
        cleanup_job_staging(&staging_root, "../outside"),
        Err(DownloadErrorCode::OutputInvalid)
    );
    assert!(outside.join("keep.txt").exists());
}

#[test]
fn filesystem_staging_creates_one_private_job_child_and_refuses_reuse_or_invalid_ids() {
    let root = tempfile::tempdir().unwrap();
    let jobs = FilesystemJobStaging::new(root.path().join("jobs"));
    let created = jobs.create("42").unwrap();
    let canonical_root = fs::canonicalize(root.path().join("jobs")).unwrap();
    assert_eq!(created.parent(), Some(canonical_root.as_path()));
    assert_eq!(
        created.file_name().and_then(|value| value.to_str()),
        Some("42")
    );
    assert_eq!(jobs.create("42"), Err(DownloadErrorCode::PublicationFailed));
    assert_eq!(
        jobs.create("../outside"),
        Err(DownloadErrorCode::InvalidRequest)
    );
    jobs.cleanup("42").unwrap();
    assert!(!created.exists());
}

#[cfg(windows)]
#[test]
fn cleanup_rejects_a_reparse_point_job_directory_when_creation_is_permitted() {
    use std::os::windows::fs::symlink_dir;

    let root = tempfile::tempdir().unwrap();
    let staging_root = root.path().join("jobs");
    let outside = root.path().join("outside");
    fs::create_dir(&staging_root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), b"keep").unwrap();
    if symlink_dir(&outside, staging_root.join("42")).is_err() {
        return;
    }
    assert_eq!(
        cleanup_job_staging(&staging_root, "42"),
        Err(DownloadErrorCode::OutputInvalid)
    );
    assert!(outside.join("keep.txt").exists());
}

#[test]
fn instagram_single_post_item_publishes_as_ordinal_without_a_folder() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let source = instagram_staged_file(root.path(), "42", 1, "jpg", b"image-bytes");
    let publisher = FilesystemInstagramOutputPublisher::new();
    let publication = publisher
        .prepare(
            &destination,
            &instagram_request("42", InstagramSourceKind::Post, 1),
        )
        .unwrap();

    let display = publisher
        .publish(
            InstagramVerifiedAsset::new(source.clone(), "jpg", 11).unwrap(),
            &publication,
            &instagram_item(1, InstagramMediaKind::Image, "jpg"),
            &CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(display, "01.jpg");
    assert_eq!(
        publication.open_path(),
        fs::canonicalize(&destination).unwrap()
    );
    assert_eq!(
        fs::read(destination.join("01.jpg")).unwrap(),
        b"image-bytes"
    );
    assert!(!source.exists());
}

#[test]
fn instagram_image_publication_uses_the_verified_format_not_the_url_extension_hint() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let publisher = FilesystemInstagramOutputPublisher::new();
    let publication = publisher
        .prepare(
            &destination,
            &instagram_request("42", InstagramSourceKind::Post, 1),
        )
        .unwrap();
    for extension in ["png", "webp", "avif"] {
        let source = instagram_staged_file(root.path(), "42", 1, extension, b"verified-image");
        let display = publisher
            .publish(
                InstagramVerifiedAsset::new(source, extension, 14).unwrap(),
                &publication,
                &instagram_item(1, InstagramMediaKind::Image, "jpg"),
                &CancellationToken::new(),
            )
            .unwrap();
        assert!(display.ends_with(&format!(".{extension}")));
        assert_eq!(
            fs::read(destination.join(display)).unwrap(),
            b"verified-image"
        );
    }
}

#[test]
fn instagram_publication_still_rejects_cross_kind_and_unplanned_video_containers() {
    for (kind, hinted, actual) in [
        (InstagramMediaKind::Image, "jpg", "mp4"),
        (InstagramMediaKind::Video, "mp4", "jpg"),
        (InstagramMediaKind::Video, "mp4", "webm"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let source = instagram_staged_file(root.path(), "42", 1, actual, b"verified-image");
        let publisher = FilesystemInstagramOutputPublisher::new();
        let publication = publisher
            .prepare(
                &destination,
                &instagram_request("42", InstagramSourceKind::Post, 1),
            )
            .unwrap();
        assert_eq!(
            publisher.publish(
                InstagramVerifiedAsset::new(source.clone(), actual, 14).unwrap(),
                &publication,
                &instagram_item(1, kind, hinted),
                &CancellationToken::new(),
            ),
            Err(DownloadErrorCode::OutputInvalid)
        );
        assert!(source.exists());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }
}

#[test]
fn instagram_publication_accepts_the_full_domain_title_limit() {
    assert!(InstagramPublicationRequest::new(
        "42",
        InstagramSourceKind::Reel,
        "Abc_123-",
        "x".repeat(512),
        Some("creator".to_owned()),
        1,
    )
    .is_ok());
    assert_eq!(
        InstagramPublicationRequest::new(
            "42",
            InstagramSourceKind::Reel,
            "Abc_123-",
            "x".repeat(513),
            Some("creator".to_owned()),
            1,
        )
        .err(),
        Some(DownloadErrorCode::InvalidRequest)
    );
}

#[test]
fn instagram_multi_post_uses_a_unique_safe_folder_and_stable_ordinals() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::create_dir(destination.join("creator [Abc_123-]")).unwrap();
    let publisher = FilesystemInstagramOutputPublisher::new();
    let request = instagram_request("42", InstagramSourceKind::Post, 2);
    let publication = publisher.prepare(&destination, &request).unwrap();
    assert_eq!(
        publication.folder_display_name(),
        Some("creator [Abc_123-] (1)")
    );

    for (ordinal, extension, bytes, kind) in [
        (1, "jpg", b"image".as_slice(), InstagramMediaKind::Image),
        (2, "mp4", b"video".as_slice(), InstagramMediaKind::Video),
    ] {
        let source = instagram_staged_file(root.path(), "42", ordinal, extension, bytes);
        let display = publisher
            .publish(
                InstagramVerifiedAsset::new(source, extension, bytes.len() as u64).unwrap(),
                &publication,
                &instagram_item(ordinal, kind, extension),
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(display, format!("{ordinal:02}.{extension}"));
    }
    assert_eq!(
        fs::read(publication.open_path().join("01.jpg")).unwrap(),
        b"image"
    );
    assert_eq!(
        fs::read(publication.open_path().join("02.mp4")).unwrap(),
        b"video"
    );
}

#[test]
fn instagram_reel_uses_a_safe_title_and_never_overwrites() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("A demo post [Abc_123-].mp4"), b"old").unwrap();
    let source = instagram_staged_file(root.path(), "42", 1, "mp4", b"new");
    let publisher = FilesystemInstagramOutputPublisher::new();
    let publication = publisher
        .prepare(
            &destination,
            &instagram_request("42", InstagramSourceKind::Reel, 1),
        )
        .unwrap();
    let display = publisher
        .publish(
            InstagramVerifiedAsset::new(source, "mp4", 3).unwrap(),
            &publication,
            &instagram_item(1, InstagramMediaKind::Video, "mp4"),
            &CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(display, "A demo post [Abc_123-] (1).mp4");
    assert_eq!(fs::read(destination.join(&display)).unwrap(), b"new");
    assert_eq!(
        fs::read(destination.join("A demo post [Abc_123-].mp4")).unwrap(),
        b"old"
    );
}

struct CorruptCrossVolumeCopy {
    moves: AtomicUsize,
    standard: StdPublicationOperations,
}

impl PublicationOperations for CorruptCrossVolumeCopy {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if self.moves.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(io::Error::from_raw_os_error(17))
        } else {
            self.standard.move_no_replace(source, destination)
        }
    }

    fn copy_to_new_and_sync(&self, _source: &Path, destination: &Path) -> io::Result<()> {
        fs::write(destination, b"bad!!")
    }

    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        self.standard.remove_file_if_exists(path)
    }
}

#[test]
fn instagram_cross_volume_copy_must_match_the_verified_source_hash_before_publish() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let source = instagram_staged_file(root.path(), "42", 1, "jpg", b"good!");
    let publisher = FilesystemInstagramOutputPublisher::with_operations(CorruptCrossVolumeCopy {
        moves: AtomicUsize::new(0),
        standard: StdPublicationOperations,
    });
    let publication = publisher
        .prepare(
            &destination,
            &instagram_request("42", InstagramSourceKind::Post, 1),
        )
        .unwrap();
    let result = publisher.publish(
        InstagramVerifiedAsset::new(source.clone(), "jpg", 5).unwrap(),
        &publication,
        &instagram_item(1, InstagramMediaKind::Image, "jpg"),
        &CancellationToken::new(),
    );

    assert_eq!(result, Err(DownloadErrorCode::OutputInvalid));
    assert!(source.exists());
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
}
