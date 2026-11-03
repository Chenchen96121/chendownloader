pub const BUILD_TARGET: &str = env!("BUILD_TARGET");

#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ToolName {
    YtDlp,
    Deno,
    GalleryDl,
    Ffmpeg,
    Ffprobe,
}

impl ToolName {
    pub const fn required() -> [Self; 5] {
        [
            Self::YtDlp,
            Self::Deno,
            Self::GalleryDl,
            Self::Ffmpeg,
            Self::Ffprobe,
        ]
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Deno => "deno",
            Self::GalleryDl => "gallery-dl",
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    pub fn artifact_name(self, target: &str) -> String {
        format!("{}-{target}.exe", self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestTool {
    pub name: ToolName,
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedToolManifest {
    pub schema_version: u8,
    pub target: String,
    pub tools: Vec<ManifestTool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Missing,
    Ready,
    IntegrityFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub name: ToolName,
    pub state: ToolState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInventory {
    pub target: String,
    pub tools: Vec<ToolStatus>,
}

impl ToolInventory {
    pub fn missing(target: &str) -> Self {
        Self::with_state(target, ToolState::Missing)
    }

    pub fn integrity_failed(target: &str) -> Self {
        Self::with_state(target, ToolState::IntegrityFailed)
    }

    pub fn managed_tools_configured(&self) -> bool {
        self.tools.len() == ToolName::required().len()
            && self
                .tools
                .iter()
                .zip(ToolName::required())
                .all(|(tool, expected_name)| {
                    tool.name == expected_name
                        && tool.state == ToolState::Ready
                        && tool
                            .version
                            .as_deref()
                            .is_some_and(|version| !version.is_empty())
                })
    }

    fn with_state(target: &str, state: ToolState) -> Self {
        Self {
            target: target.to_owned(),
            tools: ToolName::required()
                .into_iter()
                .map(|name| ToolStatus {
                    name,
                    state,
                    version: None,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedToolErrorCode {
    UnsupportedTarget,
    InvalidToolManifest,
    ToolMissing,
    ToolIntegrityFailed,
    ToolProbeFailed,
    ToolVersionMismatch,
    ToolActivationFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedToolError {
    pub code: ManagedToolErrorCode,
    pub detail: String,
}

impl ManagedToolError {
    pub fn new(code: ManagedToolErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}
