use crate::{
    application::{
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram,
            ProcessRequest, ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        download::DownloadErrorCode,
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_STDOUT_LIMIT: usize = 64 * 1024;
const STDERR_LIMIT: usize = 256 * 1024;
/// A byte-preserving, verified local image. Absolute paths stay on the Rust side.
pub struct VerifiedImage {
    path: PathBuf,
    container: String,
    mime_type: Option<&'static str>,
    width: u32,
    height: u32,
    byte_length: u64,
    sha256: String,
}

impl VerifiedImage {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn container(&self) -> &str {
        &self.container
    }
    pub fn mime_type(&self) -> Option<&'static str> {
        self.mime_type
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Re-hash a fallback copy before it can be published outside staging.
    pub fn verify_byte_identical_copy(&self, candidate: &Path) -> Result<(), DownloadErrorCode> {
        let metadata =
            fs::symlink_metadata(candidate).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || is_reparse_point(&metadata)
            || metadata.len() != self.byte_length
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let digest = hash_file(candidate).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if digest != self.sha256 {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(())
    }
}

pub(crate) struct ImageStaging {
    directory: PathBuf,
    preserve: bool,
}
impl ImageStaging {
    pub(crate) fn new(directory: &Path) -> Result<Self, DownloadErrorCode> {
        Ok(Self {
            directory: validate_empty_directory(directory)?,
            preserve: false,
        })
    }
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn verify<R: ManagedToolResolver, P: ProcessSupervisor>(
        mut self,
        resolver: &R,
        process: &P,
        expected_width: Option<u32>,
        expected_height: Option<u32>,
        max_bytes: u64,
        cancellation: &CancellationToken,
    ) -> Result<VerifiedImage, DownloadErrorCode> {
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        let mut candidate = find_single_file(&self.directory)?;
        let size = fs::metadata(&candidate)
            .map_err(|_| DownloadErrorCode::OutputInvalid)?
            .len();
        if size == 0 || size > max_bytes {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let identity = identify_image(&candidate)?;
        let desired = self.directory.join(format!("stage.{}", identity.container));
        if candidate != desired {
            if desired.exists() {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            fs::rename(&candidate, &desired).map_err(|_| DownloadErrorCode::OutputInvalid)?;
            candidate = desired;
        }
        let result = verify_local_image(
            resolver,
            process,
            &candidate,
            expected_width,
            expected_height,
            max_bytes,
            cancellation,
        )?;
        self.preserve = true;
        Ok(result)
    }
}
impl Drop for ImageStaging {
    fn drop(&mut self) {
        if self.preserve {
            return;
        }
        if let Ok(entries) = fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                if let Ok(metadata) = fs::symlink_metadata(entry.path()) {
                    if metadata.is_dir() && !metadata.file_type().is_symlink() {
                        let _ = fs::remove_dir(entry.path());
                    } else {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}
pub fn verify_image_file<R: ManagedToolResolver, P: ProcessSupervisor>(
    resolver: &R,
    process: &P,
    path: &Path,
    expected_dimensions: Option<(u32, u32)>,
    max_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<VerifiedImage, DownloadErrorCode> {
    verify_local_image(
        resolver,
        process,
        path,
        expected_dimensions.map(|v| v.0),
        expected_dimensions.map(|v| v.1),
        max_bytes,
        cancellation,
    )
}
#[allow(clippy::too_many_arguments)]
fn verify_local_image<R: ManagedToolResolver, P: ProcessSupervisor>(
    resolver: &R,
    process: &P,
    path: &Path,
    expected_width: Option<u32>,
    expected_height: Option<u32>,
    max_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<VerifiedImage, DownloadErrorCode> {
    if cancellation.is_canceled() {
        return Err(DownloadErrorCode::ProcessFailed);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || is_reparse_point(&metadata)
        || metadata.len() == 0
        || metadata.len() > max_bytes
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let path = path
        .canonicalize()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let identity = identify_image(&path)?;
    let byte_length = metadata.len();
    let (width, height) = probe_image(
        resolver,
        process,
        &path,
        path.parent().ok_or(DownloadErrorCode::OutputInvalid)?,
        byte_length,
        ImageProbeExpectation {
            identity: &identity,
            expected_width,
            expected_height,
        },
        cancellation,
    )?;
    let sha256 = hash_file(&path)?;
    if cancellation.is_canceled() {
        return Err(DownloadErrorCode::ProcessFailed);
    }
    Ok(VerifiedImage {
        path,
        container: identity.container.into(),
        mime_type: identity.mime_type,
        width,
        height,
        byte_length,
        sha256,
    })
}
struct ImageIdentity {
    container: &'static str,
    mime_type: Option<&'static str>,
    codec: &'static str,
}
struct ImageProbeExpectation<'a> {
    identity: &'a ImageIdentity,
    expected_width: Option<u32>,
    expected_height: Option<u32>,
}
fn validate_empty_directory(directory: &Path) -> Result<PathBuf, DownloadErrorCode> {
    let metadata = fs::symlink_metadata(directory).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical = directory
        .canonicalize()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if fs::read_dir(&canonical)
        .map_err(|_| DownloadErrorCode::OutputInvalid)?
        .next()
        .is_some()
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(canonical)
}

fn find_single_file(directory: &Path) -> Result<PathBuf, DownloadErrorCode> {
    let entries = fs::read_dir(directory)
        .map_err(|_| DownloadErrorCode::OutputInvalid)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let [entry] = entries.as_slice() else {
        return Err(DownloadErrorCode::OutputInvalid);
    };
    let name = entry.file_name();
    let name = name.to_str().ok_or(DownloadErrorCode::OutputInvalid)?;
    if !name.starts_with("stage.") || name.matches('.').count() != 1 {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let metadata =
        fs::symlink_metadata(entry.path()).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let candidate = entry
        .path()
        .canonicalize()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if candidate.parent() != Some(directory) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(candidate)
}

fn identify_image(path: &Path) -> Result<ImageIdentity, DownloadErrorCode> {
    let mut header = [0_u8; 32];
    let read = File::open(path)
        .and_then(|mut file| file.read(&mut header))
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let header = &header[..read];
    if header.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok(ImageIdentity {
            container: "jpg",
            mime_type: Some("image/jpeg"),
            codec: "mjpeg",
        })
    } else if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(ImageIdentity {
            container: "png",
            mime_type: Some("image/png"),
            codec: "png",
        })
    } else if header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        Ok(ImageIdentity {
            container: "webp",
            mime_type: Some("image/webp"),
            codec: "webp",
        })
    } else if header.len() >= 16
        && &header[4..8] == b"ftyp"
        && (&header[8..12] == b"avif" || &header[8..12] == b"avis")
    {
        Ok(ImageIdentity {
            container: "avif",
            mime_type: Some("image/avif"),
            codec: "av1",
        })
    } else {
        Err(DownloadErrorCode::ImageContentInvalid)
    }
}

fn probe_image<R, P>(
    resolver: &R,
    process: &P,
    path: &Path,
    working_dir: &Path,
    byte_length: u64,
    expectation: ImageProbeExpectation<'_>,
    cancellation: &CancellationToken,
) -> Result<(u32, u32), DownloadErrorCode>
where
    R: ManagedToolResolver,
    P: ProcessSupervisor,
{
    let ffprobe = resolver
        .resolve_verified(ToolName::Ffprobe)
        .map_err(map_tool_error)?;
    let args = [
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-protocol_whitelist"),
        OsString::from("file"),
        OsString::from("-print_format"),
        OsString::from("json"),
        OsString::from("-show_entries"),
        OsString::from("stream=codec_name,codec_type,width,height:format=format_name,size"),
        OsString::from("--"),
        path.as_os_str().to_owned(),
    ]
    .into();
    let exit = process
        .run(
            ProcessRequest {
                program: ProcessProgram::from_resolved_tool(ffprobe),
                args,
                working_dir: working_dir.to_owned(),
                timeout: Some(PROBE_TIMEOUT),
                inactivity_timeout: None,
                output_mode: OutputMode::Capture {
                    stdout_limit: PROBE_STDOUT_LIMIT,
                    stderr_limit: STDERR_LIMIT,
                },
            },
            cancellation.clone(),
            &NoopSink,
        )
        .map_err(map_process_error)?;
    if cancellation.is_canceled() {
        return Err(DownloadErrorCode::ProcessFailed);
    }
    if exit.code != 0 {
        return Err(DownloadErrorCode::ImageProbeFailed);
    }
    let probe: ProbeDocument =
        serde_json::from_slice(&exit.stdout).map_err(|_| DownloadErrorCode::ImageProbeFailed)?;
    let [stream] = probe.streams.as_slice() else {
        return Err(DownloadErrorCode::ImageProbeFailed);
    };
    if stream.codec_type != "video"
        || stream.codec_name != expectation.identity.codec
        || stream.width == 0
        || stream.height == 0
    {
        return Err(DownloadErrorCode::ImageProbeFailed);
    }
    // image2 can omit format.size for Windows canonical paths. The bounded
    // ordinary file's metadata is authoritative; compare the optional probe
    // measurement only when present, and never accept a malformed measurement.
    if probe
        .format
        .size
        .as_ref()
        .is_some_and(|size| size.parse::<u64>().ok() != Some(byte_length))
    {
        return Err(DownloadErrorCode::ImageSizeMismatch);
    }
    if expectation
        .expected_width
        .is_some_and(|v| v != stream.width)
        || expectation
            .expected_height
            .is_some_and(|v| v != stream.height)
    {
        return Err(DownloadErrorCode::ImageDimensionsMismatch);
    }
    Ok((stream.width, stream.height))
}

fn hash_file(path: &Path) -> Result<String, DownloadErrorCode> {
    let mut file = File::open(path).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Deserialize)]
struct ProbeDocument {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}
#[derive(Deserialize)]
struct ProbeStream {
    codec_name: String,
    codec_type: String,
    width: u32,
    height: u32,
}
#[derive(Deserialize)]
struct ProbeFormat {
    #[allow(dead_code)]
    format_name: String,
    #[serde(default, deserialize_with = "deserialize_present_probe_size")]
    size: Option<String>,
}

fn deserialize_present_probe_size<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // default handles absence; an explicitly present null/non-string is invalid.
    String::deserialize(deserializer).map(Some)
}

fn map_tool_error(error: ManagedToolError) -> DownloadErrorCode {
    match error.code {
        ManagedToolErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        ManagedToolErrorCode::ToolIntegrityFailed
        | ManagedToolErrorCode::InvalidToolManifest
        | ManagedToolErrorCode::ToolVersionMismatch => DownloadErrorCode::ToolIntegrityFailed,
        ManagedToolErrorCode::UnsupportedTarget
        | ManagedToolErrorCode::ToolProbeFailed
        | ManagedToolErrorCode::ToolActivationFailed => DownloadErrorCode::ProcessFailed,
    }
}

fn map_process_error(error: ProcessFailureKind) -> DownloadErrorCode {
    match error {
        ProcessFailureKind::TimedOut | ProcessFailureKind::InactivityTimedOut => {
            DownloadErrorCode::DownloadTimeout
        }
        ProcessFailureKind::StdoutLimitExceeded
        | ProcessFailureKind::StderrLimitExceeded
        | ProcessFailureKind::LineLimitExceeded => DownloadErrorCode::OutputInvalid,
        ProcessFailureKind::LaunchFailed
        | ProcessFailureKind::JobControlFailed
        | ProcessFailureKind::IoFailed
        | ProcessFailureKind::Canceled => DownloadErrorCode::ProcessFailed,
    }
}

struct NoopSink;
impl ProcessEventSink for NoopSink {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
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
