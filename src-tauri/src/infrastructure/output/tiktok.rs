use super::{
    sanitize_output_stem,
    verification::{
        hash_file_cancelable, is_reparse_point, verify_published, verify_published_cancelable,
    },
    PublicationOperations, StdPublicationOperations,
};
use crate::{
    application::tiktok_download_job::{
        TikTokOutputPublisher, TikTokPublication, TikTokPublicationRequest, TikTokVerifiedAsset,
    },
    domain::{
        download::DownloadErrorCode,
        tiktok::{TikTokItemAvailability, TikTokMediaItem, TikTokMediaKind},
    },
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
pub struct FilesystemTikTokOutputPublisher<O = StdPublicationOperations> {
    operations: O,
}
impl FilesystemTikTokOutputPublisher<StdPublicationOperations> {
    pub fn new() -> Self {
        Self {
            operations: StdPublicationOperations,
        }
    }
}
impl Default for FilesystemTikTokOutputPublisher<StdPublicationOperations> {
    fn default() -> Self {
        Self::new()
    }
}
impl<O> FilesystemTikTokOutputPublisher<O> {
    pub fn with_operations(operations: O) -> Self {
        Self { operations }
    }
}
impl<O: PublicationOperations> TikTokOutputPublisher for FilesystemTikTokOutputPublisher<O> {
    fn prepare(
        &self,
        destination: &Path,
        request: &TikTokPublicationRequest,
    ) -> Result<TikTokPublication, DownloadErrorCode> {
        let destination = directory(destination)?;
        let author = request
            .creator()
            .unwrap_or("X")
            .chars()
            .take(80)
            .collect::<String>();
        let stem = sanitize_output_stem(&author, request.post_id());
        for collision in 0..10_000 {
            let display_name = if collision == 0 {
                stem.clone()
            } else {
                format!("{stem} ({collision})")
            };
            if display_name.chars().count() > 160 {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            let candidate = destination.join(&display_name);
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    let canonical = match directory(&candidate) {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = fs::remove_dir(&candidate);
                            return Err(e);
                        }
                    };
                    if canonical.parent() != Some(destination.as_path()) {
                        let _ = fs::remove_dir(&candidate);
                        return Err(DownloadErrorCode::OutputInvalid);
                    }
                    return Ok(TikTokPublication {
                        destination,
                        directory: canonical,
                        display_name,
                        request: request.clone(),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_error(e)),
            }
        }
        Err(DownloadErrorCode::PublicationFailed)
    }
    fn publish(
        &self,
        asset: TikTokVerifiedAsset,
        publication: &TikTokPublication,
        item: &TikTokMediaItem,
        cancellation: &crate::application::process::CancellationToken,
    ) -> Result<String, DownloadErrorCode> {
        check_cancel(cancellation)?;
        validate_publication(publication)?;
        if item.availability != TikTokItemAvailability::Ready
            || !publication.request.items().iter().any(|i| {
                i.item_id == item.item_id && i.ordinal == item.ordinal && i.kind == item.kind
            })
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let kind_valid = match (&asset, item.kind) {
            (TikTokVerifiedAsset::Image(image), TikTokMediaKind::Image) => {
                matches!(image.container(), "jpg" | "png" | "webp" | "avif")
            }
            (TikTokVerifiedAsset::Video(video), TikTokMediaKind::Video) => {
                video.has_video && matches!(video.container.as_str(), "mp4" | "webm" | "mkv")
            }
            _ => false,
        };
        if !kind_valid || asset.byte_length() == 0 || asset.byte_length() > 9_007_199_254_740_991 {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let source = validate_asset(&asset, publication.request.job_id(), item.ordinal)?;
        let digest = hash_file_cancelable(&source, cancellation)?;
        if let TikTokVerifiedAsset::Image(image) = &asset {
            let actual = digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if actual != image.sha256() {
                return Err(DownloadErrorCode::OutputInvalid);
            }
        }
        for collision in 0..10_000 {
            let name = if collision == 0 {
                format!("{:02}.{}", item.ordinal, asset.container())
            } else {
                format!("{:02} ({collision}).{}", item.ordinal, asset.container())
            };
            let destination = publication.directory.join(&name);
            check_cancel(cancellation)?;
            match self.operations.move_no_replace(&source, &destination) {
                Ok(()) => {
                    if let Err(error) = verify_published(&destination, asset.byte_length(), &digest)
                    {
                        let _ = self.operations.remove_file_if_exists(&destination);
                        return Err(error);
                    }
                    return Ok(name);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) if matches!(e.raw_os_error(), Some(17 | 18)) => {
                    if self.copy_publish(
                        &source,
                        &destination,
                        publication,
                        item.ordinal,
                        collision,
                        asset.byte_length(),
                        &digest,
                        cancellation,
                    )? {
                        return Ok(name);
                    }
                }
                Err(e) => return Err(io_error(e)),
            }
        }
        Err(DownloadErrorCode::PublicationFailed)
    }
    fn cleanup_empty(&self, publication: &TikTokPublication) -> Result<(), DownloadErrorCode> {
        if !publication.directory.exists() {
            return Ok(());
        }
        validate_publication(publication)?;
        if fs::read_dir(&publication.directory)
            .map_err(io_error)?
            .next()
            .is_some()
        {
            return Ok(());
        }
        match fs::remove_dir(&publication.directory) {
            Ok(()) => Ok(()),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                Ok(())
            }
            Err(e) => Err(io_error(e)),
        }
    }
}
impl<O: PublicationOperations> FilesystemTikTokOutputPublisher<O> {
    #[allow(clippy::too_many_arguments)]
    fn copy_publish(
        &self,
        source: &Path,
        destination: &Path,
        publication: &TikTokPublication,
        ordinal: u16,
        collision: u32,
        length: u64,
        digest: &[u8],
        cancellation: &crate::application::process::CancellationToken,
    ) -> Result<bool, DownloadErrorCode> {
        for attempt in 0..32 {
            check_cancel(cancellation)?;
            let temporary = publication.directory.join(format!(
                ".lmd-x-{}-{ordinal}-{collision}-{attempt}.part",
                publication.request.job_id()
            ));
            match self
                .operations
                .copy_to_new_and_sync_cancelable(source, &temporary, cancellation)
            {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    check_cancel(cancellation)?;
                    return Err(io_error(e));
                }
            }
            if let Err(error) =
                verify_published_cancelable(&temporary, length, digest, cancellation)
            {
                let _ = self.operations.remove_file_if_exists(&temporary);
                return Err(error);
            }
            if let Err(error) = check_cancel(cancellation) {
                let _ = self.operations.remove_file_if_exists(&temporary);
                return Err(error);
            }
            match self.operations.move_no_replace(&temporary, destination) {
                Ok(()) => {
                    if let Err(error) = verify_published(destination, length, digest) {
                        let _ = self.operations.remove_file_if_exists(destination);
                        return Err(error);
                    }
                    // A source cleanup failure cannot undo an already verified publication.
                    let _ = self.operations.remove_file_if_exists(source);
                    return Ok(true);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    return Ok(false);
                }
                Err(e) => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    return Err(io_error(e));
                }
            }
        }
        Err(DownloadErrorCode::PublicationFailed)
    }
}
fn validate_publication(publication: &TikTokPublication) -> Result<(), DownloadErrorCode> {
    let output = directory(&publication.directory)?;
    if output != publication.directory
        || output.parent() != Some(publication.destination.as_path())
        || directory(&publication.destination)? != publication.destination
        || output.file_name().and_then(|s| s.to_str()) != Some(publication.display_name.as_str())
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(())
}
fn check_cancel(
    token: &crate::application::process::CancellationToken,
) -> Result<(), DownloadErrorCode> {
    if token.is_canceled() {
        Err(DownloadErrorCode::ProcessFailed)
    } else {
        Ok(())
    }
}
fn validate_asset(
    asset: &TikTokVerifiedAsset,
    job_id: &str,
    ordinal: u16,
) -> Result<PathBuf, DownloadErrorCode> {
    let path = asset.path();
    let meta = fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || is_reparse_point(&meta)
        || meta.len() != asset.byte_length()
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let parent = canonical.parent().ok_or(DownloadErrorCode::OutputInvalid)?;
    if canonical != path
        || canonical.file_stem().and_then(|s| s.to_str()) != Some("stage")
        || canonical.extension().and_then(|s| s.to_str()) != Some(asset.container())
        || parent.file_name().and_then(|s| s.to_str()) != Some(format!("{ordinal:02}").as_str())
        || parent
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            != Some(job_id)
        || directory(parent)? != parent
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(canonical)
}
fn directory(path: &Path) -> Result<PathBuf, DownloadErrorCode> {
    let meta = fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
    if !meta.is_dir() || meta.file_type().is_symlink() || is_reparse_point(&meta) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    path.canonicalize()
        .map_err(|_| DownloadErrorCode::DestinationUnavailable)
}
fn io_error(error: io::Error) -> DownloadErrorCode {
    if matches!(error.raw_os_error(), Some(28 | 112)) {
        DownloadErrorCode::DiskFull
    } else {
        DownloadErrorCode::PublicationFailed
    }
}
