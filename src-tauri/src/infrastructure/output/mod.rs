use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    application::{
        download_job::JobStaging,
        output_publisher::{OutputPublisher, PublishedOutput},
        output_verifier::VerifiedOutput,
        process::CancellationToken,
    },
    domain::download::{valid_opaque_id, DownloadErrorCode},
};

pub mod instagram;
mod verification;
pub mod x;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_STEM_UTF16_UNITS: usize = 180;
const MAX_COLLISIONS: u32 = 10_000;
const MAX_TEMP_ATTEMPTS: u32 = 32;

pub struct FilesystemJobStaging {
    root: PathBuf,
}

impl FilesystemJobStaging {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl JobStaging for FilesystemJobStaging {
    fn create(&self, job_id: &str) -> Result<PathBuf, DownloadErrorCode> {
        if !valid_opaque_id(job_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        fs::create_dir_all(&self.root).map_err(map_staging_io_error)?;
        let root_metadata = fs::symlink_metadata(&self.root).map_err(map_staging_io_error)?;
        if !root_metadata.is_dir()
            || root_metadata.file_type().is_symlink()
            || is_reparse_point(&root_metadata)
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let canonical_root = fs::canonicalize(&self.root).map_err(map_staging_io_error)?;
        let staging = canonical_root.join(job_id);
        fs::create_dir(&staging).map_err(map_staging_io_error)?;
        let metadata = fs::symlink_metadata(&staging).map_err(map_staging_io_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            let _ = fs::remove_dir(&staging);
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(staging)
    }

    fn cleanup(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        cleanup_job_staging(&self.root, job_id)
    }
}

pub trait PublicationOperations: Send + Sync {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()>;
    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()>;
    fn copy_to_new_and_sync_cancelable(
        &self,
        source: &Path,
        destination: &Path,
        token: &crate::application::process::CancellationToken,
    ) -> io::Result<()> {
        self.copy_to_new_and_sync(source, destination)?;
        if token.is_canceled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        Ok(())
    }
    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StdPublicationOperations;

impl PublicationOperations for StdPublicationOperations {
    fn copy_to_new_and_sync_cancelable(
        &self,
        source: &Path,
        destination: &Path,
        token: &crate::application::process::CancellationToken,
    ) -> io::Result<()> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let mut buffer = [0u8; 64 * 1024];
        let mut copied = 0u64;
        loop {
            if token.is_canceled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            copied += count as u64;
        }
        if token.is_canceled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        output.flush()?;
        output.sync_all()?;
        if copied != input.metadata()?.len() || copied != output.metadata()?.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(())
    }
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let wide = |path: &Path| -> io::Result<Vec<u16>> {
                let mut units: Vec<_> = path.as_os_str().encode_wide().collect();
                if units.contains(&0) || units.len() >= 32767 {
                    return Err(io::Error::from(io::ErrorKind::InvalidInput));
                }
                units.push(0);
                Ok(units)
            };
            let source = wide(source)?;
            let destination = wide(destination)?;
            // Both buffers are live, NUL-terminated UTF-16. MoveFileW atomically
            // fails if the destination exists; fs::rename permits replacement.
            let moved = unsafe {
                windows_sys::Win32::Storage::FileSystem::MoveFileW(
                    source.as_ptr(),
                    destination.as_ptr(),
                )
            };
            if moved == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        #[cfg(not(windows))]
        {
            // File publication only. Link creation is atomic no-replace; EXDEV
            // is left to the destination-side copy fallback in the publisher.
            fs::hard_link(source, destination)?;
            if let Err(error) = fs::remove_file(source) {
                let _ = fs::remove_file(destination);
                return Err(error);
            }
            Ok(())
        }
    }

    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let copied = io::copy(&mut input, &mut output)?;
        output.flush()?;
        output.sync_all()?;
        let source_length = input.metadata()?.len();
        if copied != source_length || output.metadata()?.len() != source_length {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "incomplete destination copy",
            ));
        }
        Ok(())
    }

    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

pub struct FilesystemOutputPublisher<O = StdPublicationOperations> {
    operations: O,
}

impl FilesystemOutputPublisher<StdPublicationOperations> {
    pub fn new() -> Self {
        Self {
            operations: StdPublicationOperations,
        }
    }
}

impl Default for FilesystemOutputPublisher<StdPublicationOperations> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O> FilesystemOutputPublisher<O> {
    pub fn with_operations(operations: O) -> Self {
        Self { operations }
    }
}

impl<O> OutputPublisher for FilesystemOutputPublisher<O>
where
    O: PublicationOperations,
{
    fn publish(
        &self,
        verified: VerifiedOutput,
        destination: &Path,
        safe_title: &str,
        media_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<PublishedOutput, DownloadErrorCode> {
        check_publication_cancellation(cancellation)?;
        let source = validate_verified_output(&verified, media_id)?;
        let destination = validate_destination(destination)?;
        let job_id = source
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .filter(|value| valid_opaque_id(value))
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        let stem = sanitize_output_stem(safe_title, media_id);

        for collision in 0..MAX_COLLISIONS {
            check_publication_cancellation(cancellation)?;
            let display_name = output_name(&stem, &verified.container, collision);
            let final_path = destination.join(&display_name);
            if final_path.exists() {
                continue;
            }
            match self.operations.move_no_replace(&source, &final_path) {
                Ok(()) => {
                    return Ok(PublishedOutput {
                        final_path,
                        display_name,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) if is_cross_volume(&error) => {
                    match self.publish_cross_volume(
                        &source,
                        &final_path,
                        &destination,
                        job_id,
                        collision,
                        cancellation,
                    ) {
                        Ok(()) => {
                            return Ok(PublishedOutput {
                                final_path,
                                display_name,
                            });
                        }
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(error) => {
                            check_publication_cancellation(cancellation)?;
                            return Err(map_publication_error(&error));
                        }
                    }
                }
                Err(error) => return Err(map_publication_error(&error)),
            }
        }
        Err(DownloadErrorCode::PublicationFailed)
    }
}

impl<O: PublicationOperations> FilesystemOutputPublisher<O> {
    fn publish_cross_volume(
        &self,
        source: &Path,
        final_path: &Path,
        destination: &Path,
        job_id: &str,
        collision: u32,
        cancellation: &CancellationToken,
    ) -> io::Result<()> {
        for attempt in 0..MAX_TEMP_ATTEMPTS {
            if cancellation.is_canceled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let temporary = destination.join(format!(".lmd-{job_id}-{collision}-{attempt}.part"));
            match self
                .operations
                .copy_to_new_and_sync_cancelable(source, &temporary, cancellation)
            {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    return Err(error);
                }
            }
            if cancellation.is_canceled() {
                let _ = self.operations.remove_file_if_exists(&temporary);
                return Err(io::ErrorKind::Interrupted.into());
            }
            match self.operations.move_no_replace(&temporary, final_path) {
                Ok(()) => {
                    let _ = self.operations.remove_file_if_exists(source);
                    return Ok(());
                }
                Err(error) => {
                    let _ = self.operations.remove_file_if_exists(&temporary);
                    return Err(error);
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary destination names exhausted",
        ))
    }
}

fn check_publication_cancellation(
    cancellation: &CancellationToken,
) -> Result<(), DownloadErrorCode> {
    if cancellation.is_canceled() {
        Err(DownloadErrorCode::ProcessFailed)
    } else {
        Ok(())
    }
}

fn validate_verified_output(
    verified: &VerifiedOutput,
    media_id: &str,
) -> Result<PathBuf, DownloadErrorCode> {
    if verified.byte_length == 0
        || verified.byte_length > MAX_SAFE_INTEGER
        || !safe_token(media_id, 64)
        || !safe_token(&verified.container, 16)
        || verified
            .container
            .bytes()
            .any(|byte| byte.is_ascii_uppercase())
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let metadata =
        fs::symlink_metadata(&verified.path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file()
        || metadata.len() != verified.byte_length
        || metadata.file_type().is_symlink()
        || is_reparse_point(&metadata)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical =
        fs::canonicalize(&verified.path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if canonical != verified.path
        || canonical
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case(&verified.container))
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(canonical)
}

fn validate_destination(destination: &Path) -> Result<PathBuf, DownloadErrorCode> {
    let metadata =
        fs::symlink_metadata(destination).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::DestinationUnavailable);
    }
    fs::canonicalize(destination).map_err(|_| DownloadErrorCode::DestinationUnavailable)
}

pub fn sanitize_output_stem(title: &str, media_id: &str) -> String {
    let mut title = sanitize_component(title);
    let media_id = sanitize_component(media_id);
    if title.is_empty() {
        title = "media".to_owned();
    }
    if windows_reserved_name(&title) {
        if media_id.is_empty() {
            title = "media".to_owned();
        } else {
            title.push('_');
        }
    }
    let suffix = if media_id.is_empty() {
        String::new()
    } else {
        format!(" [{media_id}]")
    };
    let maximum_title_units = MAX_STEM_UTF16_UNITS.saturating_sub(suffix.encode_utf16().count());
    title = truncate_utf16(&title, maximum_title_units);
    title = title.trim_end_matches([' ', '.']).to_owned();
    if title.is_empty() {
        title = "media".to_owned();
    }
    format!("{title}{suffix}")
}

fn sanitize_component(value: &str) -> String {
    let replaced: String = value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
                || matches!(
                    character,
                    '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect();
    replaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches([' ', '.'])
        .to_owned()
}

fn truncate_utf16(value: &str, maximum_units: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next <= maximum_units {
                units = next;
                true
            } else {
                false
            }
        })
        .collect()
}

fn windows_reserved_name(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn output_name(stem: &str, extension: &str, collision: u32) -> String {
    if collision == 0 {
        format!("{stem}.{extension}")
    } else {
        format!("{stem} ({collision}).{extension}")
    }
}

fn safe_token(value: &str, maximum_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn is_cross_volume(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(17 | 18))
}

fn map_publication_error(error: &io::Error) -> DownloadErrorCode {
    if matches!(error.raw_os_error(), Some(28 | 112)) {
        DownloadErrorCode::DiskFull
    } else {
        DownloadErrorCode::PublicationFailed
    }
}

fn map_staging_io_error(error: io::Error) -> DownloadErrorCode {
    if matches!(error.raw_os_error(), Some(28 | 112)) {
        DownloadErrorCode::DiskFull
    } else {
        DownloadErrorCode::PublicationFailed
    }
}

pub fn cleanup_job_staging(staging_root: &Path, job_id: &str) -> Result<(), DownloadErrorCode> {
    if !valid_opaque_id(job_id) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let root_metadata =
        fs::symlink_metadata(staging_root).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !root_metadata.is_dir()
        || root_metadata.file_type().is_symlink()
        || is_reparse_point(&root_metadata)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical_root =
        fs::canonicalize(staging_root).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let target = canonical_root.join(job_id);
    let metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(DownloadErrorCode::OutputInvalid),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical_target =
        fs::canonicalize(&target).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if canonical_target.parent() != Some(canonical_root.as_path())
        || canonical_target
            .file_name()
            .and_then(|value| value.to_str())
            != Some(job_id)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    fs::remove_dir_all(canonical_target).map_err(|_| DownloadErrorCode::PublicationFailed)
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

pub mod tiktok;
