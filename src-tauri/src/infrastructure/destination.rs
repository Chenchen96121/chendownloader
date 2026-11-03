use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub use crate::application::destination::sanitize_display_name;
use crate::{application::destination::DestinationValidator, domain::download::DownloadErrorCode};
use tauri_plugin_dialog::DialogExt;

pub struct FilesystemDestinationValidator;

pub fn pick_native_destination(
    app: &tauri::AppHandle,
) -> Result<Option<PathBuf>, DownloadErrorCode> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .map(|selected| {
            selected
                .into_path()
                .map_err(|_| DownloadErrorCode::DestinationUnavailable)
        })
        .transpose()
}

impl DestinationValidator for FilesystemDestinationValidator {
    fn validate(&self, path: &Path) -> Result<PathBuf, DownloadErrorCode> {
        let metadata =
            fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().readonly()
            || is_reparse_point(&metadata)
        {
            return Err(DownloadErrorCode::DestinationUnavailable);
        }

        let canonical =
            fs::canonicalize(path).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        let canonical_metadata = fs::symlink_metadata(&canonical)
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        if !canonical_metadata.is_dir()
            || canonical_metadata.file_type().is_symlink()
            || canonical_metadata.permissions().readonly()
            || is_reparse_point(&canonical_metadata)
        {
            return Err(DownloadErrorCode::DestinationUnavailable);
        }

        let mut probe = tempfile::Builder::new()
            .prefix(".local-media-downloader-write-probe-")
            .tempfile_in(&canonical)
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        probe
            .write_all(b"probe")
            .and_then(|_| probe.as_file().sync_all())
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        probe
            .close()
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        Ok(canonical)
    }
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
