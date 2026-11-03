use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use super::atomic_replace;
use sha2::{Digest, Sha256};

use crate::{
    application::{
        tool_bootstrap::{ManagedToolStore, StagedPackage, StagedTool},
        tool_manifest::{validate_manifest, ValidatedManifest},
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::managed_tools::{
        ManagedToolError, ManagedToolErrorCode, ManagedToolManifest, ManifestTool, ToolInventory,
        ToolState, ToolStatus,
    },
};

pub struct FilesystemManagedToolStore {
    root: PathBuf,
    target: String,
}

impl FilesystemManagedToolStore {
    pub fn new(root: PathBuf, target: &str) -> Self {
        Self {
            root,
            target: target.to_owned(),
        }
    }

    fn target_root(&self) -> PathBuf {
        self.root.join(&self.target)
    }

    fn version_path(&self, tool: &ManifestTool) -> PathBuf {
        self.target_root()
            .join(tool.name.as_str())
            .join("versions")
            .join(format!("{}-{}", tool.version, &tool.sha256[..12]))
            .join(tool.name.artifact_name(&self.target))
    }

    fn catalog_path(&self) -> PathBuf {
        self.target_root().join("catalog.json")
    }

    fn previous_path(&self) -> PathBuf {
        self.target_root().join("catalog.previous.json")
    }

    fn catalog_lock(&self) -> Result<fs::File, ManagedToolError> {
        let failed = |_| activation_failed("managed-tool catalog lock could not be acquired");
        fs::create_dir_all(self.target_root()).map_err(failed)?;
        // Keep the same inode across instances/processes; removing/recreating it would split locks.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.target_root().join("catalog.lock"))
            .map_err(failed)?;
        file.lock().map_err(failed)?;
        Ok(file)
    }

    fn read_catalog(&self, path: &Path) -> Result<(Vec<u8>, ValidatedManifest), ManagedToolError> {
        let metadata = fs::symlink_metadata(path).map_err(activation_failed)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(integrity_failed("catalog is not a regular file"));
        }
        let bytes = fs::read(path).map_err(activation_failed)?;
        let manifest: ManagedToolManifest =
            serde_json::from_slice(&bytes).map_err(activation_failed)?;
        let manifest = validate_manifest(&manifest, &self.target)?;
        for tool in manifest.tools() {
            verify_hash(&self.version_path(tool), &tool.sha256)?;
        }
        Ok((bytes, manifest))
    }

    fn replace_catalog(&self, bytes: &[u8], rotate_active: bool) -> Result<(), ManagedToolError> {
        let pending = self.target_root().join("catalog.pending.json");
        remove_stale_pending(&pending)?;
        atomic_replace::write_new(&pending, bytes).map_err(activation_failed)?;
        let result = (|| {
            if rotate_active {
                // Read and verify before changing previous; do not retain invalid active evidence.
                if let Ok((active_bytes, _)) = self.read_catalog(&self.catalog_path()) {
                    let previous_pending = self.target_root().join("catalog.previous.pending.json");
                    remove_stale_pending(&previous_pending)?;
                    atomic_replace::write_new(&previous_pending, &active_bytes)
                        .map_err(activation_failed)?;
                    let rotation =
                        atomic_replace::replace(&previous_pending, &self.previous_path())
                            .map_err(activation_failed);
                    if rotation.is_err() {
                        let _ = fs::remove_file(&previous_pending);
                    }
                    rotation?;
                }
            }
            atomic_replace::replace(&pending, &self.catalog_path()).map_err(activation_failed)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&pending);
        }
        result
    }

    fn ready_inventory(&self, manifest: &ValidatedManifest) -> ToolInventory {
        ToolInventory {
            target: self.target.clone(),
            tools: manifest
                .tools()
                .iter()
                .map(|tool| ToolStatus {
                    name: tool.name,
                    state: ToolState::Ready,
                    version: Some(tool.version.clone()),
                })
                .collect(),
        }
    }

    fn active_manifest_for_resolution(&self) -> Result<ValidatedManifest, ManagedToolError> {
        let path = self.catalog_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ManagedToolError::new(
                    ManagedToolErrorCode::ToolMissing,
                    "active managed-tool catalog is missing",
                ));
            }
            Err(_) => return Err(integrity_failed("active catalog could not be verified")),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(integrity_failed("active catalog is not a regular file"));
        }
        let bytes =
            fs::read(path).map_err(|_| integrity_failed("active catalog could not be verified"))?;
        let manifest: ManagedToolManifest = serde_json::from_slice(&bytes)
            .map_err(|_| integrity_failed("active catalog is invalid"))?;
        validate_manifest(&manifest, &self.target)
            .map_err(|_| integrity_failed("active catalog is invalid"))
    }
}

impl ManagedToolResolver for FilesystemManagedToolStore {
    fn resolve_verified(
        &self,
        name: crate::domain::managed_tools::ToolName,
    ) -> Result<ResolvedTool, ManagedToolError> {
        let _catalog_lock = self.catalog_lock()?;
        let manifest = self.active_manifest_for_resolution()?;
        let tool = manifest
            .tools()
            .iter()
            .find(|tool| tool.name == name)
            .expect("validated manifest contains every required tool");
        let path = self.version_path(tool);
        verify_hash(&path, &tool.sha256)
            .map_err(|_| integrity_failed("active artifact failed verification"))?;
        let canonical_root = fs::canonicalize(self.target_root())
            .map_err(|_| integrity_failed("managed-tool root could not be verified"))?;
        let canonical_path = fs::canonicalize(&path)
            .map_err(|_| integrity_failed("active artifact could not be verified"))?;
        if !canonical_path.starts_with(canonical_root) {
            return Err(integrity_failed(
                "active artifact escaped the managed-tool root",
            ));
        }

        Ok(ResolvedTool {
            name,
            version: tool.version.clone(),
            path,
        })
    }
}

impl ManagedToolStore for FilesystemManagedToolStore {
    fn matching_inventory(&self, manifest: &ValidatedManifest) -> Option<ToolInventory> {
        let _catalog_lock = self.catalog_lock().ok()?;
        let (_, active) = self.read_catalog(&self.catalog_path()).ok()?;
        if active != *manifest {
            return None;
        }
        Some(self.ready_inventory(&active))
    }

    fn stage(
        &self,
        package_root: &Path,
        manifest: &ValidatedManifest,
    ) -> Result<StagedPackage, ManagedToolError> {
        if manifest.target() != self.target {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::UnsupportedTarget,
                "store target does not match manifest",
            ));
        }
        let package_root = fs::canonicalize(package_root).map_err(activation_failed)?;
        let transactions = self.target_root().join("transactions");
        fs::create_dir_all(&transactions).map_err(activation_failed)?;
        let transaction = tempfile::Builder::new()
            .prefix("stage-")
            .tempdir_in(transactions)
            .map_err(activation_failed)?;
        let mut tools = Vec::new();
        for tool in manifest.tools() {
            let source = package_root.join(tool.name.artifact_name(&self.target));
            let metadata = fs::symlink_metadata(&source).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ManagedToolError::new(
                        ManagedToolErrorCode::ToolMissing,
                        "package artifact is missing",
                    )
                } else {
                    activation_failed(error)
                }
            })?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(integrity_failed(
                    "package artifact must be a regular non-symlink file",
                ));
            }
            let canonical = fs::canonicalize(&source).map_err(activation_failed)?;
            if canonical.parent() != Some(package_root.as_path()) {
                return Err(integrity_failed("package artifact escaped package root"));
            }
            let path = transaction
                .path()
                .join(tool.name.artifact_name(&self.target));
            fs::copy(canonical, &path).map_err(activation_failed)?;
            verify_hash(&path, &tool.sha256)?;
            tools.push(StagedTool {
                name: tool.name,
                version: tool.version.clone(),
                sha256: tool.sha256.clone(),
                path,
            });
        }
        Ok(StagedPackage {
            manifest: manifest.clone(),
            transaction_dir: transaction.keep(),
            tools,
        })
    }

    fn activate(&self, staged: &StagedPackage) -> Result<(), ManagedToolError> {
        let _catalog_lock = self.catalog_lock()?;
        if staged.manifest.target() != self.target {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::UnsupportedTarget,
                "store target does not match staged manifest",
            ));
        }
        for tool in staged.manifest.tools() {
            let source = staged
                .transaction_dir
                .join(tool.name.artifact_name(&self.target));
            verify_hash(&source, &tool.sha256)?;
            let destination = self.version_path(tool);
            create_ordinary_tool_directory(
                &self.root,
                destination.parent().expect("version path has parent"),
            )?;
            match fs::symlink_metadata(&destination) {
                Ok(metadata) => {
                    require_regular_artifact(&metadata)?;
                    if let Err(error) = verify_hash(&destination, &tool.sha256) {
                        if error.code != ManagedToolErrorCode::ToolIntegrityFailed {
                            return Err(error);
                        }
                        repair_regular_artifact(&source, &destination, &tool.sha256)?;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::rename(source, destination).map_err(activation_failed)?;
                }
                Err(error) => return Err(activation_failed(error)),
            }
        }
        let bytes = serde_json::to_vec(&staged.manifest.clone().into_manifest())
            .map_err(activation_failed)?;
        self.replace_catalog(&bytes, true)
    }

    fn discard(&self, staged: &StagedPackage) {
        let _ = fs::remove_dir_all(&staged.transaction_dir);
    }

    fn inventory(&self) -> ToolInventory {
        let Ok(_catalog_lock) = self.catalog_lock() else {
            return ToolInventory::integrity_failed(&self.target);
        };
        if let Ok((_, manifest)) = self.read_catalog(&self.catalog_path()) {
            return self.ready_inventory(&manifest);
        }
        if let Ok((bytes, manifest)) = self.read_catalog(&self.previous_path()) {
            // Recovery never rotates invalid active over a verified previous catalog.
            return match self.replace_catalog(&bytes, false) {
                Ok(()) => self.ready_inventory(&manifest),
                Err(_) => ToolInventory::integrity_failed(&self.target),
            };
        }
        let absent = [self.catalog_path(), self.previous_path()].iter().all(|path| {
            matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        });
        if absent {
            ToolInventory::missing(&self.target)
        } else {
            ToolInventory::integrity_failed(&self.target)
        }
    }

    fn rollback(&self) -> Result<ToolInventory, ManagedToolError> {
        let _catalog_lock = self.catalog_lock()?;
        // Cache original previous bytes and verify every artifact BEFORE active rotation overwrites previous.
        let (bytes, manifest) = self.read_catalog(&self.previous_path())?;
        self.replace_catalog(&bytes, true)?;
        Ok(self.ready_inventory(&manifest))
    }
}

fn verify_hash(path: &Path, expected: &str) -> Result<(), ManagedToolError> {
    let metadata = fs::symlink_metadata(path).map_err(activation_failed)?;
    require_regular_artifact(&metadata)?;
    let mut file = fs::File::open(path).map_err(activation_failed)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(activation_failed)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if format!("{:x}", hash.finalize()) != expected {
        return Err(integrity_failed("artifact SHA-256 does not match manifest"));
    }
    Ok(())
}

fn create_ordinary_tool_directory(root: &Path, directory: &Path) -> Result<(), ManagedToolError> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| integrity_failed("artifact directory is outside the store"))?;
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            current.push(component);
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(activation_failed)?;
                fs::symlink_metadata(&current).map_err(activation_failed)?
            }
            Err(error) => return Err(activation_failed(error)),
        };
        if !metadata.is_dir() || is_reparse_point(&metadata) {
            return Err(integrity_failed(
                "artifact directory is not an ordinary directory",
            ));
        }
    }
    Ok(())
}

fn repair_regular_artifact(
    source: &Path,
    destination: &Path,
    expected: &str,
) -> Result<(), ManagedToolError> {
    // Replace only damaged ordinary files, after all bundle probes have succeeded.
    // A same-directory temporary file keeps the old bytes intact until atomic publication.
    let mut replacement =
        tempfile::NamedTempFile::new_in(destination.parent().expect("version path has parent"))
            .map_err(activation_failed)?;
    let mut input = fs::File::open(source).map_err(activation_failed)?;
    std::io::copy(&mut input, replacement.as_file_mut()).map_err(activation_failed)?;
    replacement
        .as_file()
        .sync_all()
        .map_err(activation_failed)?;
    verify_hash(replacement.path(), expected)?;
    require_regular_artifact(&fs::symlink_metadata(destination).map_err(activation_failed)?)?;
    atomic_replace::replace(replacement.path(), destination).map_err(activation_failed)?;
    verify_hash(destination, expected)
}

fn require_regular_artifact(metadata: &fs::Metadata) -> Result<(), ManagedToolError> {
    if !metadata.is_file() || is_reparse_point(metadata) {
        return Err(integrity_failed("artifact is not a regular file"));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn activation_failed(error: impl std::fmt::Display) -> ManagedToolError {
    ManagedToolError::new(
        ManagedToolErrorCode::ToolActivationFailed,
        error.to_string(),
    )
}
fn remove_stale_pending(path: &Path) -> Result<(), ManagedToolError> {
    // Only internally derived pending FILES are disposable; active/previous remain authoritative.
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(activation_failed(error)),
    }
}
fn integrity_failed(detail: &str) -> ManagedToolError {
    ManagedToolError::new(ManagedToolErrorCode::ToolIntegrityFailed, detail)
}
