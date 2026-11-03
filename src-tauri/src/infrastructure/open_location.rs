use std::{ffi::OsStr, fs, path::Path};

use crate::{
    application::download_job::CompletedLocationOpener, domain::download::DownloadErrorCode,
};

pub struct NativeCompletedLocationOpener;

impl CompletedLocationOpener for NativeCompletedLocationOpener {
    fn open(&self, path: &Path) -> Result<(), DownloadErrorCode> {
        let metadata =
            fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(DownloadErrorCode::DestinationUnavailable);
        }
        let canonical =
            fs::canonicalize(path).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        open_native(canonical.as_os_str())
    }
}

pub fn open_media_page(
    source: &crate::application::media_url::ValidatedMediaUrl,
) -> Result<(), crate::domain::media::AnalysisErrorCode> {
    if source.platform() != crate::domain::media::Platform::Youtube {
        return Err(crate::domain::media::AnalysisErrorCode::UnsupportedPlatform);
    }
    open_native(OsStr::new(source.url()))
        .map_err(|_| crate::domain::media::AnalysisErrorCode::AnalysisUnavailable)
}

#[cfg(windows)]
fn open_native(target: &OsStr) -> Result<(), DownloadErrorCode> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::FreeLibrary,
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
    };

    type ShellExecuteWFn = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *const u16,
        *const u16,
        *const u16,
        *const u16,
        i32,
    ) -> *mut core::ffi::c_void;

    let library_name: Vec<u16> = "shell32.dll".encode_utf16().chain([0]).collect();
    let operation: Vec<u16> = "open".encode_utf16().chain([0]).collect();
    let target: Vec<u16> = target.encode_wide().chain([0]).collect();
    // Loading on demand avoids initializing desktop UI when test executables start.
    // SAFETY: names are NUL terminated and the resolved API is called while loaded.
    let result = unsafe {
        let module = LoadLibraryW(library_name.as_ptr());
        if module.is_null() {
            return Err(DownloadErrorCode::DestinationUnavailable);
        }
        let procedure = GetProcAddress(module, c"ShellExecuteW".as_ptr().cast());
        let Some(procedure) = procedure else {
            FreeLibrary(module);
            return Err(DownloadErrorCode::DestinationUnavailable);
        };
        let shell_execute: ShellExecuteWFn = std::mem::transmute(procedure);
        let result = shell_execute(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        );
        FreeLibrary(module);
        result
    };
    if result as isize > 32 {
        Ok(())
    } else {
        Err(DownloadErrorCode::DestinationUnavailable)
    }
}

#[cfg(not(windows))]
fn open_native(_target: &OsStr) -> Result<(), DownloadErrorCode> {
    Err(DownloadErrorCode::DestinationUnavailable)
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
