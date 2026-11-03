use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::{
    check_publication_cancellation,
    verification::{
        hash_file_cancelable, is_reparse_point, verify_published, verify_published_cancelable,
    },
};

use crate::{
    application::instagram_download_job::{
        InstagramOutputPublisher, InstagramPublication, InstagramPublicationRequest,
        InstagramVerifiedAsset,
    },
    domain::{
        download::DownloadErrorCode,
        instagram::{
            InstagramItemAvailability, InstagramMediaItem, InstagramMediaKind, InstagramSourceKind,
        },
    },
};

use super::{sanitize_output_stem, PublicationOperations, StdPublicationOperations};

const MAX_COLLISIONS: u32 = 10_000;
const MAX_TEMP_ATTEMPTS: u32 = 32;

pub struct FilesystemInstagramOutputPublisher<O = StdPublicationOperations> {
    operations: O,
}

impl FilesystemInstagramOutputPublisher<StdPublicationOperations> {
    pub fn new() -> Self {
        Self {
            operations: StdPublicationOperations,
        }
    }
}

impl Default for FilesystemInstagramOutputPublisher<StdPublicationOperations> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O> FilesystemInstagramOutputPublisher<O> {
    pub fn with_operations(operations: O) -> Self {
        Self { operations }
    }
}

impl<O> InstagramOutputPublisher for FilesystemInstagramOutputPublisher<O>
where
    O: PublicationOperations,
{
    fn prepare(
        &self,
        destination: &Path,
        request: &InstagramPublicationRequest,
    ) -> Result<InstagramPublication, DownloadErrorCode> {
        let destination = validate_directory(destination)?;
        if request.source_kind() == InstagramSourceKind::Post && request.item_count() > 1 {
            let stem = sanitize_output_stem(
                request.creator().unwrap_or("Instagram"),
                request.shortcode(),
            );
            for collision in 0..MAX_COLLISIONS {
                let display_name = collision_name(&stem, collision);
                let candidate = destination.join(&display_name);
                match fs::create_dir(&candidate) {
                    Ok(()) => {
                        let canonical =
                            fs::canonicalize(&candidate).map_err(|error| map_io_error(&error))?;
                        if canonical.parent() != Some(destination.as_path()) {
                            let _ = fs::remove_dir(&candidate);
                            return Err(DownloadErrorCode::OutputInvalid);
                        }
                        return InstagramPublication::new(
                            canonical.clone(),
                            canonical,
                            Some(display_name),
                            request.clone(),
                        );
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(map_io_error(&error)),
                }
            }
            return Err(DownloadErrorCode::PublicationFailed);
        }
        InstagramPublication::new(destination.clone(), destination, None, request.clone())
    }

    fn publish(
        &self,
        asset: InstagramVerifiedAsset,
        publication: &InstagramPublication,
        item: &InstagramMediaItem,
        cancellation: &crate::application::process::CancellationToken,
    ) -> Result<String, DownloadErrorCode> {
        check_publication_cancellation(cancellation)?;
        if item.availability != InstagramItemAvailability::Ready
            || !publication.request().accepts_ordinal(item.ordinal)
            || match item.kind {
                // URL extensions are hints. Image bytes have already passed
                // signature, codec, size and dimension verification upstream.
                InstagramMediaKind::Image => {
                    !matches!(asset.container(), "jpg" | "png" | "webp" | "avif")
                }
                InstagramMediaKind::Video => item.container != asset.container(),
            }
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let source = validate_asset(&asset, publication.request(), item.ordinal)?;
        let output_directory = validate_directory(publication.output_directory())?;
        let source_hash = hash_file_cancelable(&source, cancellation)?;
        let stem = match publication.request().source_kind() {
            InstagramSourceKind::Post => format!("{:02}", item.ordinal),
            InstagramSourceKind::Reel => sanitize_output_stem(
                publication.request().title(),
                publication.request().shortcode(),
            ),
        };
        for collision in 0..MAX_COLLISIONS {
            check_publication_cancellation(cancellation)?;
            let display_name = output_name(&stem, asset.container(), collision);
            let final_path = output_directory.join(&display_name);
            if final_path.exists() {
                continue;
            }
            match self.operations.move_no_replace(&source, &final_path) {
                Ok(()) => {
                    if let Err(error) =
                        verify_published(&final_path, asset.byte_length(), &source_hash)
                    {
                        let _ = self.operations.remove_file_if_exists(&final_path);
                        return Err(error);
                    }
                    return Ok(display_name);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) if is_cross_volume(&error) => {
                    let published = self.publish_cross_volume(
                        &source,
                        &final_path,
                        &output_directory,
                        publication.request().job_id(),
                        item.ordinal,
                        collision,
                        asset.byte_length(),
                        &source_hash,
                        cancellation,
                    )?;
                    if !published {
                        continue;
                    }
                    return Ok(display_name);
                }
                Err(error) => return Err(map_io_error(&error)),
            }
        }
        Err(DownloadErrorCode::PublicationFailed)
    }
}

impl<O: PublicationOperations> FilesystemInstagramOutputPublisher<O> {
    #[allow(clippy::too_many_arguments)]
    fn publish_cross_volume(
        &self,
        source: &Path,
        final_path: &Path,
        destination: &Path,
        job_id: &str,
        ordinal: u16,
        collision: u32,
        byte_length: u64,
        source_hash: &[u8],
        cancellation: &crate::application::process::CancellationToken,
    ) -> Result<bool, DownloadErrorCode> {
        for attempt in 0..MAX_TEMP_ATTEMPTS {
            check_publication_cancellation(cancellation)?;
            let temporary = destination.join(format!(
                ".lmd-instagram-{job_id}-{ordinal}-{collision}-{attempt}.part"
            ));
            match self
                .operations
                .copy_to_new_and_sync_cancelable(source, &temporary, cancellation)
            {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    check_publication_cancellation(cancellation)?;
                    return Err(map_io_error(&error));
                }
            }
            if let Err(error) =
                verify_published_cancelable(&temporary, byte_length, source_hash, cancellation)
            {
                let _ = self.operations.remove_file_if_exists(&temporary);
                return Err(error);
            }
            if let Err(error) = check_publication_cancellation(cancellation) {
                let _ = self.operations.remove_file_if_exists(&temporary);
                return Err(error);
            }
            match self.operations.move_no_replace(&temporary, final_path) {
                Ok(()) => {
                    if let Err(error) = verify_published(final_path, byte_length, source_hash) {
                        let _ = self.operations.remove_file_if_exists(final_path);
                        return Err(error);
                    }
                    self.operations
                        .remove_file_if_exists(source)
                        .map_err(|error| map_io_error(&error))?;
                    return Ok(true);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    return Ok(false);
                }
                Err(error) => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    return Err(map_io_error(&error));
                }
            }
        }
        Err(DownloadErrorCode::PublicationFailed)
    }
}

fn validate_asset(
    asset: &InstagramVerifiedAsset,
    request: &InstagramPublicationRequest,
    ordinal: u16,
) -> Result<PathBuf, DownloadErrorCode> {
    let metadata =
        fs::symlink_metadata(asset.path()).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || is_reparse_point(&metadata)
        || metadata.len() != asset.byte_length()
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical = fs::canonicalize(asset.path()).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if canonical != asset.path()
        || canonical
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case(asset.container()))
        || canonical
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            != Some(format!("{ordinal:02}").as_str())
        || canonical
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            != Some(request.job_id())
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(canonical)
}

fn validate_directory(directory: &Path) -> Result<PathBuf, DownloadErrorCode> {
    let metadata =
        fs::symlink_metadata(directory).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::DestinationUnavailable);
    }
    fs::canonicalize(directory).map_err(|_| DownloadErrorCode::DestinationUnavailable)
}

fn output_name(stem: &str, extension: &str, collision: u32) -> String {
    if collision == 0 {
        format!("{stem}.{extension}")
    } else {
        format!("{stem} ({collision}).{extension}")
    }
}

fn collision_name(stem: &str, collision: u32) -> String {
    if collision == 0 {
        stem.to_owned()
    } else {
        format!("{stem} ({collision})")
    }
}

fn is_cross_volume(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(17 | 18))
}

fn map_io_error(error: &io::Error) -> DownloadErrorCode {
    if matches!(error.raw_os_error(), Some(28 | 112)) {
        DownloadErrorCode::DiskFull
    } else {
        DownloadErrorCode::PublicationFailed
    }
}
