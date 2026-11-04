use local_media_downloader_lib::application::process::CancellationToken;
use local_media_downloader_lib::{
    application::{
        output_verifier::VerifiedOutput,
        tiktok_download_job::{
            TikTokOutputPublisher, TikTokPublicationRequest, TikTokVerifiedAsset,
        },
    },
    domain::{download::DownloadErrorCode, tiktok::TikTokMediaItem},
    infrastructure::output::{
        tiktok::FilesystemTikTokOutputPublisher, PublicationOperations, StdPublicationOperations,
    },
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
fn item(ordinal: u16) -> TikTokMediaItem {
    serde_json::from_value(serde_json::json!({"itemId":ordinal.to_string(),"ordinal":ordinal,"kind":"video","container":"mp4","previewAvailable":false,"availability":"ready"})).unwrap()
}
fn request(items: Vec<TikTokMediaItem>) -> TikTokPublicationRequest {
    TikTokPublicationRequest::new(
        "71".into(),
        "12345678901234567890".into(),
        "A post".into(),
        Some("测试作者".into()),
        items,
    )
    .unwrap()
}

#[test]
fn tiktok_publication_request_rejects_invalid_identity_and_duplicate_or_reordered_items() {
    assert!(TikTokPublicationRequest::new(
        "0".into(),
        "1".into(),
        "title".into(),
        None,
        vec![item(1)]
    )
    .is_err());
    assert!(TikTokPublicationRequest::new(
        "71".into(),
        "../x".into(),
        "title".into(),
        None,
        vec![item(1)]
    )
    .is_err());
    assert!(TikTokPublicationRequest::new(
        "71".into(),
        "1".into(),
        "title".into(),
        None,
        vec![item(3), item(1)]
    )
    .is_err());
    assert!(TikTokPublicationRequest::new(
        "71".into(),
        "1".into(),
        "title".into(),
        None,
        vec![item(1), item(1)]
    )
    .is_err());
}

#[test]
fn tiktok_cancellation_before_commit_preserves_source_and_cleans_partial_copy_but_after_commit_keeps_success(
) {
    struct CancelOperations {
        token: CancellationToken,
        after_commit: bool,
    }
    impl PublicationOperations for CancelOperations {
        fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
            if !self.after_commit && source.file_stem().is_some_and(|s| s == "stage") {
                return Err(io::Error::from_raw_os_error(18));
            }
            StdPublicationOperations.move_no_replace(source, destination)?;
            self.token.cancel();
            Ok(())
        }
        fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
            StdPublicationOperations.copy_to_new_and_sync(source, destination)?;
            self.token.cancel();
            Ok(())
        }
        fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
            StdPublicationOperations.remove_file_if_exists(path)
        }
    }
    for phase in ["before", "copy", "committed"] {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("out");
        fs::create_dir(&out).unwrap();
        let token = CancellationToken::new();
        if phase == "before" {
            token.cancel();
        }
        let publisher = FilesystemTikTokOutputPublisher::with_operations(CancelOperations {
            token: token.clone(),
            after_commit: phase == "committed",
        });
        let media = item(3);
        let publication = publisher
            .prepare(&out, &request(vec![media.clone()]))
            .unwrap();
        let result = publisher.publish(asset(root.path(), 3), &publication, &media, &token);
        assert!(token.is_canceled());
        if phase == "committed" {
            assert_eq!(result.unwrap(), "03.mp4");
            assert!(publication.output_directory().join("03.mp4").is_file());
        } else {
            assert_eq!(
                result,
                Err(DownloadErrorCode::ProcessFailed),
                "cancel phase {phase}"
            );
            assert!(root.path().join("71/03/stage.mp4").is_file());
            assert_eq!(
                fs::read_dir(publication.output_directory())
                    .unwrap()
                    .count(),
                0,
                "no .part left at {phase}"
            );
            publisher.cleanup_empty(&publication).unwrap();
            assert!(!publication.output_directory().exists());
        }
    }
}
fn asset(root: &Path, ordinal: u16) -> TikTokVerifiedAsset {
    let path = root.join("71").join(format!("{ordinal:02}"));
    fs::create_dir_all(&path).unwrap();
    let path = path.join("stage.mp4");
    fs::write(&path, b"original media bytes").unwrap();
    TikTokVerifiedAsset::Video(VerifiedOutput {
        path: path.canonicalize().unwrap(),
        container: "mp4".into(),
        has_video: true,
        has_audio: true,
        duration_ms: 1000,
        byte_length: 20,
    })
}
#[test]
fn tiktok_single_and_sparse_selections_always_publish_original_ordinals_in_unique_folders() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let publisher = FilesystemTikTokOutputPublisher::new();
    let media = item(3);
    let req = request(vec![media.clone()]);
    let publication = publisher.prepare(&output, &req).unwrap();
    assert!(publication
        .folder_display_name()
        .contains("12345678901234567890"));
    let name = publisher
        .publish(
            asset(root.path(), 3),
            &publication,
            &media,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(name, "03.mp4");
    assert_eq!(
        fs::read(publication.output_directory().join("03.mp4")).unwrap(),
        b"original media bytes"
    );
    let second = publisher.prepare(&output, &req).unwrap();
    assert_ne!(second.output_directory(), publication.output_directory());
    publisher.cleanup_empty(&second).unwrap();
    assert!(!second.output_directory().exists());
    publisher.cleanup_empty(&publication).unwrap();
    assert!(publication.output_directory().exists());
}
struct CopyOperations {
    corrupt: bool,
}
impl PublicationOperations for CopyOperations {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if source.file_name().unwrap() == "stage.mp4" {
            Err(io::Error::from_raw_os_error(17))
        } else {
            StdPublicationOperations.move_no_replace(source, destination)
        }
    }
    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
        StdPublicationOperations.copy_to_new_and_sync(source, destination)?;
        if self.corrupt {
            fs::write(destination, b"corrupt copied bytes")?;
        }
        Ok(())
    }
    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        StdPublicationOperations.remove_file_if_exists(path)
    }
}
#[test]
fn tiktok_cross_volume_copy_checks_bytes_and_never_overwrites_existing_files() {
    for corrupt in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("out");
        fs::create_dir(&out).unwrap();
        let publisher =
            FilesystemTikTokOutputPublisher::with_operations(CopyOperations { corrupt });
        let media = item(3);
        let publication = publisher
            .prepare(&out, &request(vec![media.clone()]))
            .unwrap();
        fs::write(publication.output_directory().join("03.mp4"), b"keep me").unwrap();
        let result = publisher.publish(
            asset(root.path(), 3),
            &publication,
            &media,
            &CancellationToken::new(),
        );
        assert_eq!(
            fs::read(publication.output_directory().join("03.mp4")).unwrap(),
            b"keep me"
        );
        if corrupt {
            assert!(matches!(result, Err(DownloadErrorCode::OutputInvalid)));
            assert_eq!(
                fs::read_dir(publication.output_directory())
                    .unwrap()
                    .count(),
                1
            );
        } else {
            assert_eq!(result.unwrap(), "03 (1).mp4");
            assert!(!root.path().join("71/03/stage.mp4").exists());
        }
    }
}
#[test]
fn tiktok_publication_rejects_unselected_or_unverified_paths_and_invalid_requests() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("out");
    fs::create_dir(&out).unwrap();
    let publisher = FilesystemTikTokOutputPublisher::new();
    let publication = publisher.prepare(&out, &request(vec![item(3)])).unwrap();
    assert!(publisher
        .publish(
            asset(root.path(), 2),
            &publication,
            &item(2),
            &CancellationToken::new()
        )
        .is_err());
    let mut wrong = match asset(root.path(), 3) {
        TikTokVerifiedAsset::Video(v) => v,
        _ => unreachable!(),
    };
    wrong.path = PathBuf::from("C:/outside/stage.mp4");
    assert!(publisher
        .publish(
            TikTokVerifiedAsset::Video(wrong),
            &publication,
            &item(3),
            &CancellationToken::new()
        )
        .is_err());
    assert!(TikTokPublicationRequest::new(
        "0".into(),
        "1".into(),
        "title".into(),
        None,
        vec![item(1)]
    )
    .is_err());
    assert!(TikTokPublicationRequest::new(
        "71".into(),
        "../x".into(),
        "title".into(),
        None,
        vec![item(1)]
    )
    .is_err());
    assert!(TikTokPublicationRequest::new(
        "71".into(),
        "1".into(),
        "title".into(),
        None,
        vec![item(3), item(1)]
    )
    .is_err());
    publisher.cleanup_empty(&publication).unwrap();
    assert!(!publication.output_directory().exists());
}
