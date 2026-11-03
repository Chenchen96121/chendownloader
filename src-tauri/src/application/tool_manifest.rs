use crate::domain::managed_tools::{
    ManagedToolError, ManagedToolErrorCode, ManagedToolManifest, ManifestTool, ToolName,
};

const MINIMUM_YT_DLP_VERSION: (u16, u16, u16) = (2026, 6, 9);
const REQUIRED_GALLERY_DL_VERSION: &str = "1.32.15";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedManifest {
    target: String,
    tools: [ManifestTool; 5],
}

impl ValidatedManifest {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn tools(&self) -> &[ManifestTool; 5] {
        &self.tools
    }

    pub fn into_manifest(self) -> ManagedToolManifest {
        ManagedToolManifest {
            schema_version: 1,
            target: self.target,
            tools: self.tools.into(),
        }
    }
}

pub fn validate_manifest(
    manifest: &ManagedToolManifest,
    expected_target: &str,
) -> Result<ValidatedManifest, ManagedToolError> {
    if manifest.schema_version != 1 {
        return Err(invalid("schema version must be 1"));
    }

    if manifest.target != expected_target {
        return Err(ManagedToolError::new(
            ManagedToolErrorCode::UnsupportedTarget,
            "manifest target does not match the build target",
        ));
    }

    if manifest.tools.len() != ToolName::required().len()
        || ToolName::required().into_iter().any(|required| {
            manifest
                .tools
                .iter()
                .filter(|tool| tool.name == required)
                .count()
                != 1
        })
    {
        return Err(invalid(
            "manifest tools must be the unique required tool set",
        ));
    }

    if manifest
        .tools
        .iter()
        .any(|tool| !valid_version(&tool.version))
    {
        return Err(invalid("tool version has invalid characters or length"));
    }

    if manifest
        .tools
        .iter()
        .any(|tool| !valid_digest(&tool.sha256))
    {
        return Err(invalid(
            "tool digest must be 64 lowercase hexadecimal characters",
        ));
    }

    let yt_dlp = manifest
        .tools
        .iter()
        .find(|tool| tool.name == ToolName::YtDlp)
        .expect("required tool membership was already validated");
    if !valid_yt_dlp_version(&yt_dlp.version) {
        return Err(invalid("yt-dlp version is invalid or below the minimum"));
    }

    let deno = manifest
        .tools
        .iter()
        .find(|tool| tool.name == ToolName::Deno)
        .expect("required tool membership was already validated");
    if !valid_deno_version(&deno.version) {
        return Err(invalid("deno version must be bounded MAJOR.MINOR.PATCH"));
    }

    let gallery_dl = manifest
        .tools
        .iter()
        .find(|tool| tool.name == ToolName::GalleryDl)
        .expect("required tool membership was already validated");
    if gallery_dl.version != REQUIRED_GALLERY_DL_VERSION {
        return Err(invalid("gallery-dl version must match the audited release"));
    }

    let ffmpeg = manifest
        .tools
        .iter()
        .find(|tool| tool.name == ToolName::Ffmpeg)
        .expect("required tool membership was already validated");
    let ffprobe = manifest
        .tools
        .iter()
        .find(|tool| tool.name == ToolName::Ffprobe)
        .expect("required tool membership was already validated");
    if ffmpeg.version != ffprobe.version {
        return Err(invalid("ffmpeg and ffprobe versions must match"));
    }

    let mut tools = manifest.tools.clone();
    tools.sort_by_key(|tool| tool.name);
    let tools = tools
        .try_into()
        .expect("required tool membership guarantees exactly five tools");

    Ok(ValidatedManifest {
        target: manifest.target.clone(),
        tools,
    })
}

fn invalid(detail: impl Into<String>) -> ManagedToolError {
    ManagedToolError::new(ManagedToolErrorCode::InvalidToolManifest, detail)
}

fn valid_version(version: &str) -> bool {
    (1..=64).contains(&version.len())
        && version.is_ascii()
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_yt_dlp_version(version: &str) -> bool {
    let bytes = version.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'.'
        || bytes[7] != b'.'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return false;
    }

    let year = parse_decimal(&bytes[0..4]);
    let month = parse_decimal(&bytes[5..7]);
    let day = parse_decimal(&bytes[8..10]);
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (year, month, day) >= MINIMUM_YT_DLP_VERSION
}

fn valid_deno_version(version: &str) -> bool {
    let mut parts = version.split('.');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part.len() <= 5
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && part.parse::<u32>().is_ok_and(|value| value <= 99_999)
    };

    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if valid_part(major) && valid_part(minor) && valid_part(patch)
    )
}

fn parse_decimal(bytes: &[u8]) -> u16 {
    bytes
        .iter()
        .fold(0, |value, byte| value * 10 + u16::from(byte - b'0'))
}
