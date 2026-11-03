use std::{fs::OpenOptions, io::Write, path::Path};

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

pub(super) fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.parent() != destination.parent() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic replacement requires same-directory paths",
        ));
    }
    replace_platform(source, destination)
}

#[cfg(windows)]
fn replace_platform(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source_wide: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both buffers are NUL-terminated and live throughout the synchronous call.
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_platform(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}
