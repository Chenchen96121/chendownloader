use crate::domain::download::DownloadErrorCode;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};
pub(super) fn verify_published(
    path: &Path,
    expected_length: u64,
    expected_hash: &[u8],
) -> Result<(), DownloadErrorCode> {
    verify_published_cancelable(
        path,
        expected_length,
        expected_hash,
        &crate::application::process::CancellationToken::new(),
    )
}
pub(super) fn verify_published_cancelable(
    path: &Path,
    expected_length: u64,
    expected_hash: &[u8],
    token: &crate::application::process::CancellationToken,
) -> Result<(), DownloadErrorCode> {
    let metadata = fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || is_reparse_point(&metadata)
        || metadata.len() != expected_length
        || hash_file_cancelable(path, token)? != expected_hash
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(())
}

pub(super) fn hash_file_cancelable(
    path: &Path,
    token: &crate::application::process::CancellationToken,
) -> Result<Vec<u8>, DownloadErrorCode> {
    let mut file = File::open(path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if token.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        let read = file
            .read(&mut buffer)
            .map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().to_vec())
}

#[cfg(windows)]
pub(super) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
pub(super) fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}
