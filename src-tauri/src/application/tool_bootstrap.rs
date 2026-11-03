use std::path::{Path, PathBuf};

use crate::{
    application::tool_manifest::{validate_manifest, ValidatedManifest},
    domain::managed_tools::{
        ManagedToolError, ManagedToolErrorCode, ManagedToolManifest, ToolInventory, ToolName,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait ToolVersionProbe {
    fn probe(&self, tool: ToolName, candidate: &Path) -> Result<ProbeOutput, ManagedToolError>;
}

#[derive(Clone, Debug)]
pub struct StagedTool {
    pub name: ToolName,
    pub version: String,
    pub sha256: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StagedPackage {
    pub manifest: ValidatedManifest,
    pub transaction_dir: PathBuf,
    pub tools: Vec<StagedTool>,
}

pub trait ManagedToolStore {
    fn matching_inventory(&self, manifest: &ValidatedManifest) -> Option<ToolInventory>;
    fn stage(
        &self,
        package_root: &Path,
        manifest: &ValidatedManifest,
    ) -> Result<StagedPackage, ManagedToolError>;
    fn activate(&self, staged: &StagedPackage) -> Result<(), ManagedToolError>;
    fn discard(&self, staged: &StagedPackage);
    fn inventory(&self) -> ToolInventory;
    fn rollback(&self) -> Result<ToolInventory, ManagedToolError>;
}

pub fn parse_reported_version(
    tool: ToolName,
    output: &ProbeOutput,
) -> Result<String, ManagedToolError> {
    if output.exit_code != 0 {
        return Err(probe_failed(tool, "version probe exited unsuccessfully"));
    }
    if output.stdout.contains('\u{fffd}') || output.stderr.contains('\u{fffd}') {
        return Err(probe_failed(
            tool,
            "version probe output was not valid UTF-8",
        ));
    }

    if tool == ToolName::Deno {
        return parse_deno_version(
            tool,
            output.stdout.lines().next().unwrap_or_default().trim(),
        );
    }

    let line = output
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| probe_failed(tool, "version probe reported no version"))?;

    match tool {
        ToolName::YtDlp => Ok(line.to_owned()),
        ToolName::Deno => unreachable!("Deno is parsed from its first line"),
        ToolName::GalleryDl => parse_gallery_dl_version(line),
        ToolName::Ffmpeg => parse_prefixed_version(tool, line, "ffmpeg version "),
        ToolName::Ffprobe => parse_prefixed_version(tool, line, "ffprobe version "),
    }
}

pub fn parse_gallery_dl_version(line: &str) -> Result<String, ManagedToolError> {
    let mut parts = line.split('.');
    let valid_part = |part: &str| {
        !part.is_empty() && part.len() <= 5 && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(major), Some(minor), Some(patch), None)
            if valid_part(major) && valid_part(minor) && valid_part(patch) =>
        {
            Ok(line.to_owned())
        }
        _ => Err(probe_failed(
            ToolName::GalleryDl,
            "version probe reported an unexpected banner",
        )),
    }
}

fn parse_deno_version(tool: ToolName, line: &str) -> Result<String, ManagedToolError> {
    let mut tokens = line.split_whitespace();
    match (tokens.next(), tokens.next()) {
        (Some("deno"), Some(version)) if !version.is_empty() => Ok(version.to_owned()),
        _ => Err(probe_failed(
            tool,
            "version probe reported an unexpected banner",
        )),
    }
}

pub struct ToolBootstrapService<'a> {
    store: &'a dyn ManagedToolStore,
    probe: &'a dyn ToolVersionProbe,
    expected_target: &'a str,
}

impl<'a> ToolBootstrapService<'a> {
    pub fn new(
        store: &'a dyn ManagedToolStore,
        probe: &'a dyn ToolVersionProbe,
        expected_target: &'a str,
    ) -> Self {
        Self {
            store,
            probe,
            expected_target,
        }
    }

    pub fn bootstrap(
        &self,
        package_root: &Path,
        manifest: &ManagedToolManifest,
    ) -> Result<ToolInventory, ManagedToolError> {
        let manifest = validate_manifest(manifest, self.expected_target)?;
        self.bootstrap_validated(package_root, &manifest)
    }

    pub fn bootstrap_validated(
        &self,
        package_root: &Path,
        manifest: &ValidatedManifest,
    ) -> Result<ToolInventory, ManagedToolError> {
        if manifest.target() != self.expected_target {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::UnsupportedTarget,
                "manifest target does not match the bootstrap target",
            ));
        }
        let staged = self.store.stage(package_root, manifest)?;

        let result = self.probe_and_activate(manifest, &staged);
        self.store.discard(&staged);
        result
    }

    fn probe_and_activate(
        &self,
        manifest: &ValidatedManifest,
        staged: &StagedPackage,
    ) -> Result<ToolInventory, ManagedToolError> {
        if staged.manifest != *manifest {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::ToolIntegrityFailed,
                "staged package manifest does not match the requested manifest",
            ));
        }

        for name in ToolName::required() {
            let tool = staged
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .ok_or_else(|| {
                    ManagedToolError::new(
                        ManagedToolErrorCode::ToolMissing,
                        format!("staged {} executable is missing", name.as_str()),
                    )
                })?;
            let output = self.probe.probe(name, &tool.path)?;
            let reported_version = parse_reported_version(name, &output)?;
            let expected_version = manifest
                .tools()
                .iter()
                .find(|expected| expected.name == name)
                .expect("validated manifest contains every required tool")
                .version
                .as_str();

            if reported_version != expected_version {
                return Err(ManagedToolError::new(
                    ManagedToolErrorCode::ToolVersionMismatch,
                    format!(
                        "staged {} version does not match its manifest",
                        name.as_str()
                    ),
                ));
            }
        }

        self.store.activate(staged)?;
        Ok(self.store.inventory())
    }
}

pub struct ToolInventoryService<'a> {
    store: &'a dyn ManagedToolStore,
}

impl<'a> ToolInventoryService<'a> {
    pub fn new(store: &'a dyn ManagedToolStore) -> Self {
        Self { store }
    }

    pub fn inventory(&self) -> ToolInventory {
        self.store.inventory()
    }

    pub fn rollback(&self) -> Result<ToolInventory, ManagedToolError> {
        self.store.rollback()
    }
}

fn parse_prefixed_version(
    tool: ToolName,
    line: &str,
    prefix: &str,
) -> Result<String, ManagedToolError> {
    line.strip_prefix(prefix)
        .and_then(|remaining| remaining.split_whitespace().next())
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| probe_failed(tool, "version probe reported an unexpected banner"))
}

fn probe_failed(tool: ToolName, detail: &str) -> ManagedToolError {
    ManagedToolError::new(
        ManagedToolErrorCode::ToolProbeFailed,
        format!("{} {detail}", tool.as_str()),
    )
}
