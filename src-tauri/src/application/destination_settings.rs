use super::destination::{DestinationRegistry, DestinationValidator};
use crate::domain::download::{DestinationApproval, DownloadErrorCode};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DefaultDestinationStatus {
    Missing,
    Ready {
        #[serde(rename = "displayName")]
        display_name: String,
    },
    Unavailable,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedDestination {
    version: u8,
    path: PathBuf,
}

pub struct DestinationSettings<V> {
    config: PathBuf,
    validator: V,
    gate: Mutex<()>,
}

impl<V: DestinationValidator> DestinationSettings<V> {
    pub fn new(config: PathBuf, validator: V) -> Self {
        Self {
            config,
            validator,
            gate: Mutex::new(()),
        }
    }

    pub fn status(&self) -> DefaultDestinationStatus {
        let Ok(_guard) = self.gate.lock() else {
            return DefaultDestinationStatus::Unavailable;
        };
        match self.load() {
            Ok(None) => DefaultDestinationStatus::Missing,
            Ok(Some(path)) => match self.validator.validate(&path) {
                Ok(path) => DefaultDestinationStatus::Ready {
                    display_name: self.validator.display_name(&path),
                },
                Err(_) => DefaultDestinationStatus::Unavailable,
            },
            Err(_) => DefaultDestinationStatus::Unavailable,
        }
    }

    pub fn choose(
        &self,
        selected: Option<PathBuf>,
    ) -> Result<Option<DefaultDestinationStatus>, DownloadErrorCode> {
        let Some(path) = selected else {
            return Ok(None);
        };
        let _guard = self.gate.lock().map_err(|_| unavailable())?;
        let path = self.validator.validate(&path)?;
        if !path.is_absolute() {
            return Err(unavailable());
        }
        let parent = self.config.parent().ok_or_else(unavailable)?;
        fs::create_dir_all(parent).map_err(|_| unavailable())?;
        ordinary(parent, true)?;
        if self.config.exists() {
            ordinary(&self.config, false)?;
            if fs::metadata(&self.config)
                .map_err(|_| unavailable())?
                .permissions()
                .readonly()
            {
                return Err(unavailable());
            }
        }
        let display_name = self.validator.display_name(&path);
        let bytes = serde_json::to_vec(&SavedDestination { version: 1, path })
            .map_err(|_| unavailable())?;
        if bytes.len() > 32768 {
            return Err(unavailable());
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| unavailable())?;
        temporary
            .write_all(&bytes)
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|_| unavailable())?;
        temporary.persist(&self.config).map_err(|_| unavailable())?;
        Ok(Some(DefaultDestinationStatus::Ready { display_name }))
    }

    pub fn approve<R: DestinationValidator>(
        &self,
        registry: &DestinationRegistry<R>,
        now: Instant,
    ) -> Result<DestinationApproval, DownloadErrorCode> {
        let _guard = self.gate.lock().map_err(|_| unavailable())?;
        let path = self.load()?.ok_or_else(unavailable)?;
        registry.approve(path, now)
    }

    fn load(&self) -> Result<Option<PathBuf>, DownloadErrorCode> {
        match fs::symlink_metadata(&self.config) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable()),
            Ok(metadata) if metadata.len() > 32768 => return Err(unavailable()),
            Ok(_) => {}
        }
        ordinary(self.config.parent().ok_or_else(unavailable)?, true)?;
        ordinary(&self.config, false)?;
        let mut bytes = Vec::new();
        fs::File::open(&self.config)
            .map_err(|_| unavailable())?
            .take(32769)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if bytes.len() > 32768 {
            return Err(unavailable());
        }
        let saved: SavedDestination = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if saved.version != 1 || !saved.path.is_absolute() {
            return Err(unavailable());
        }
        Ok(Some(saved.path))
    }
}

fn unavailable() -> DownloadErrorCode {
    DownloadErrorCode::DestinationUnavailable
}

fn ordinary(path: &Path, directory: bool) -> Result<(), DownloadErrorCode> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(unavailable());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(unavailable());
        }
    }
    Ok(())
}
