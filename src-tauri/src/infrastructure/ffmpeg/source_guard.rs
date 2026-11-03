use crate::{
    application::local_processing::LocalProcessingRequest,
    domain::download::{valid_opaque_id, DownloadErrorCode},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
#[cfg(windows)]
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
const MAX_DIRECTORY_ENTRIES: usize = 3;
#[cfg(windows)]
pub(crate) struct SourceGuards {
    _staging: File,
    _source_dir: File,
    _slots: Vec<File>,
    _sources: Vec<File>,
}

#[cfg(windows)]
impl SourceGuards {
    pub(crate) fn acquire(
        request: &LocalProcessingRequest,
        staging: &Path,
        sources: &[PathBuf],
    ) -> Result<Self, DownloadErrorCode> {
        let staging_guard =
            open_guarded_component(staging, true, FILE_SHARE_READ | FILE_SHARE_WRITE)?;
        let source_dir_guard = open_guarded_component(&request.source_dir, true, FILE_SHARE_READ)?;
        let mut slot_guards = Vec::new();
        let mut source_guards = Vec::new();
        for source in sources {
            slot_guards.push(open_guarded_component(
                source.parent().ok_or(DownloadErrorCode::OutputInvalid)?,
                true,
                FILE_SHARE_READ,
            )?);
            source_guards.push(open_guarded_component(source, false, FILE_SHARE_READ)?);
        }

        if canonical_job_directory(request)? != staging || validate_sources(request)? != sources {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(Self {
            _staging: staging_guard,
            _source_dir: source_dir_guard,
            _slots: slot_guards,
            _sources: source_guards,
        })
    }
}

#[cfg(windows)]
fn open_guarded_component(
    path: &Path,
    directory: bool,
    share_mode: u32,
) -> Result<File, DownloadErrorCode> {
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let file = OpenOptions::new()
        .read(true)
        .share_mode(share_mode)
        .custom_flags(flags)
        .open(path)
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let metadata = file
        .metadata()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if metadata.is_dir() != directory
        || metadata.file_type().is_symlink()
        || is_reparse_point(&metadata)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(file)
}

#[cfg(not(windows))]
pub(crate) struct SourceGuards;

#[cfg(not(windows))]
impl SourceGuards {
    pub(crate) fn acquire(
        request: &LocalProcessingRequest,
        staging: &Path,
        sources: &[PathBuf],
    ) -> Result<Self, DownloadErrorCode> {
        if canonical_job_directory(request)? != staging || validate_sources(request)? != sources {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(Self)
    }
}

pub(crate) fn validate_sources(
    request: &LocalProcessingRequest,
) -> Result<Vec<PathBuf>, DownloadErrorCode> {
    let staging = canonical_job_directory(request)?;
    if request.source_dir != request.staging_dir.join("source")
        || path_too_long(&request.source_dir)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let source_metadata =
        fs::symlink_metadata(&request.source_dir).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !source_metadata.is_dir()
        || source_metadata.file_type().is_symlink()
        || is_reparse_point(&source_metadata)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let source_dir =
        fs::canonicalize(&request.source_dir).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if source_dir.parent() != Some(staging.as_path())
        || source_dir.file_name().and_then(|name| name.to_str()) != Some("source")
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }

    if request.sources.is_empty()
        || request.sources.len() > 2
        || request.sources.len() != request.plan.source_format_ids.len()
        || bounded_entries(&source_dir)?.len() != request.sources.len()
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let mut sources = Vec::new();
    for (index, source) in request.sources.iter().enumerate() {
        if source.slot != index
            || request.plan.source_format_ids.get(index) != Some(&source.format_id)
            || sources.iter().any(|p: &PathBuf| {
                p.parent() == Some(source_dir.join(format!("input-{index}")).as_path())
            })
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let slot = source_dir.join(format!("input-{index}"));
        let metadata = fs::symlink_metadata(&slot).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || is_reparse_point(&metadata)
            || fs::canonicalize(&slot).map_err(|_| DownloadErrorCode::OutputInvalid)? != slot
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        sources.push(validate_slot(&slot)?);
    }
    if request.plan.source_format_ids.len() == 2
        && request.plan.source_format_ids[0] == request.plan.source_format_ids[1]
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(sources)
}

fn validate_slot(source_dir: &Path) -> Result<PathBuf, DownloadErrorCode> {
    let entries = bounded_entries(source_dir)?;
    let [entry] = entries.as_slice() else {
        return Err(DownloadErrorCode::OutputInvalid);
    };
    let name = entry
        .file_name()
        .to_str()
        .map(str::to_owned)
        .ok_or(DownloadErrorCode::OutputInvalid)?;
    let extension = name
        .strip_prefix("source.")
        .filter(|extension| !extension.contains('.') && safe_video_extension(extension))
        .ok_or(DownloadErrorCode::OutputInvalid)?;
    if name != format!("source.{extension}") {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let metadata =
        fs::symlink_metadata(entry.path()).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let candidate = fs::canonicalize(entry.path()).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if candidate.parent() != Some(source_dir) || path_too_long(&candidate) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(candidate)
}

pub(crate) fn canonical_job_directory(
    request: &LocalProcessingRequest,
) -> Result<PathBuf, DownloadErrorCode> {
    if !valid_opaque_id(&request.job_id)
        || request
            .staging_dir
            .file_name()
            .and_then(|name| name.to_str())
            != Some(request.job_id.as_str())
        || path_too_long(&request.staging_dir)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let metadata =
        fs::symlink_metadata(&request.staging_dir).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    fs::canonicalize(&request.staging_dir).map_err(|_| DownloadErrorCode::OutputInvalid)
}

pub(crate) fn validate_output(
    request: &LocalProcessingRequest,
    sources: &[PathBuf],
    expected: &Path,
    container: &str,
) -> Result<(), DownloadErrorCode> {
    let current_sources = validate_sources(request)?;
    if current_sources != sources {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let staging = canonical_job_directory(request)?;
    let entries = bounded_entries(&staging)?;
    if entries.len() != 2 {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let output_metadata =
        fs::symlink_metadata(expected).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !output_metadata.is_file()
        || output_metadata.file_type().is_symlink()
        || is_reparse_point(&output_metadata)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical_output =
        fs::canonicalize(expected).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if canonical_output.as_path() != expected
        || canonical_output.parent() != Some(staging.as_path())
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let output_name = canonical_output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(DownloadErrorCode::OutputInvalid)?;
    if !matches!(container, "mp4" | "webm" | "mkv" | "m4a" | "mp3" | "flac")
        || output_name != format!("stage.{container}")
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(())
}

fn bounded_entries(directory: &Path) -> Result<Vec<fs::DirEntry>, DownloadErrorCode> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| DownloadErrorCode::OutputInvalid)? {
        if entries.len() >= MAX_DIRECTORY_ENTRIES {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        entries.push(entry.map_err(|_| DownloadErrorCode::OutputInvalid)?);
    }
    Ok(entries)
}

pub(crate) fn reject_existing_path(path: &Path) -> Result<(), DownloadErrorCode> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(DownloadErrorCode::OutputInvalid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(DownloadErrorCode::OutputInvalid),
    }
}

fn safe_video_extension(extension: &str) -> bool {
    matches!(
        extension,
        "mp4"
            | "m4v"
            | "mov"
            | "3gp"
            | "mkv"
            | "webm"
            | "m4a"
            | "aac"
            | "opus"
            | "ogg"
            | "mp3"
            | "flac"
            | "wav"
    )
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

#[cfg(windows)]
pub(crate) fn path_too_long(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().count() > 32_767
}

#[cfg(not(windows))]
pub(crate) fn path_too_long(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len() > 4_096
}
