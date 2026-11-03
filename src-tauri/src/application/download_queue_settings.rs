use crate::domain::download::DownloadErrorCode;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    limit: u8,
}

pub struct DownloadQueueSettings {
    path: PathBuf,
}
impl DownloadQueueSettings {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn load(&self) -> Result<u8, DownloadErrorCode> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(2),
            result => result.map_err(|_| DownloadErrorCode::InvalidRequest)?,
        };
        if metadata.len() > 4096 {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        ordinary(&self.path, false)?;
        ordinary(
            self.path
                .parent()
                .ok_or(DownloadErrorCode::InvalidRequest)?,
            true,
        )?;
        let mut bytes = vec![];
        fs::File::open(&self.path)
            .map_err(|_| DownloadErrorCode::InvalidRequest)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| DownloadErrorCode::InvalidRequest)?;
        if bytes.len() > 4096 {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let saved: Saved =
            serde_json::from_slice(&bytes).map_err(|_| DownloadErrorCode::InvalidRequest)?;
        if saved.version != 1 || !(1..=4).contains(&saved.limit) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(saved.limit)
    }
    pub fn save(&self, limit: u8) -> Result<(), DownloadErrorCode> {
        if !(1..=4).contains(&limit) || !self.path.is_absolute() {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let parent = self
            .path
            .parent()
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        fs::create_dir_all(parent).map_err(|_| DownloadErrorCode::PublicationFailed)?;
        ordinary(parent, true)?;
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => ordinary(&self.path, false)?,
            Err(_) => return Err(DownloadErrorCode::PublicationFailed),
        }
        let bytes = serde_json::to_vec(&Saved { version: 1, limit })
            .map_err(|_| DownloadErrorCode::Unknown)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| DownloadErrorCode::PublicationFailed)?;
        temp.write_all(&bytes)
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| DownloadErrorCode::PublicationFailed)?;
        temp.persist(&self.path)
            .map_err(|_| DownloadErrorCode::PublicationFailed)?;
        Ok(())
    }
}
fn ordinary(path: &Path, directory: bool) -> Result<(), DownloadErrorCode> {
    let metadata = fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::InvalidRequest)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(DownloadErrorCode::InvalidRequest);
        }
    }
    if metadata.file_type().is_symlink()
        || (if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        })
    {
        Err(DownloadErrorCode::InvalidRequest)
    } else {
        Ok(())
    }
}
