use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    application::{
        portable_package::{TrustedToolPackage, TrustedToolPackageSource},
        tool_manifest::{validate_manifest, ValidatedManifest},
    },
    domain::managed_tools::{ManagedToolError, ManagedToolErrorCode, ManagedToolManifest},
};

pub struct FilesystemPortableToolPackage {
    executable: PathBuf,
    target: String,
    manifest_bytes: Arc<[u8]>,
}

impl FilesystemPortableToolPackage {
    pub fn from_executable(executable: &Path, target: &str, manifest_bytes: Arc<[u8]>) -> Self {
        Self {
            executable: executable.to_path_buf(),
            target: target.to_owned(),
            manifest_bytes,
        }
    }

    fn package_root(&self) -> Result<PathBuf, ManagedToolError> {
        self.executable
            .parent()
            .map(|parent| parent.join("resources").join("managed-tools"))
            .ok_or_else(|| invalid_package(ManagedToolErrorCode::ToolMissing))
    }
}

impl TrustedToolPackageSource for FilesystemPortableToolPackage {
    fn load_manifest(&self) -> Result<ValidatedManifest, ManagedToolError> {
        let manifest: ManagedToolManifest = serde_json::from_slice(&self.manifest_bytes)
            .map_err(|_| invalid_package(ManagedToolErrorCode::InvalidToolManifest))?;
        validate_manifest(&manifest, &self.target)
    }

    fn load_package(
        &self,
        manifest: &ValidatedManifest,
    ) -> Result<TrustedToolPackage, ManagedToolError> {
        if *manifest != self.load_manifest()? {
            return Err(invalid_package(ManagedToolErrorCode::InvalidToolManifest));
        }
        let root = self.package_root()?;
        let app_root = self
            .executable
            .parent()
            .ok_or_else(|| invalid_package(ManagedToolErrorCode::ToolMissing))?;
        let resources = app_root.join("resources");
        require_ordinary_directory(app_root)?;
        require_ordinary_directory(&resources)?;
        require_ordinary_directory(&root)?;

        let mut expected = HashSet::from([OsString::from("manifest.json")]);
        for tool in manifest.tools() {
            let name = OsString::from(tool.name.artifact_name(&self.target));
            let path = root.join(&name);
            require_ordinary_file(&path, ManagedToolErrorCode::ToolMissing)?;
            expected.insert(name);
        }
        require_ordinary_file(
            &root.join("manifest.json"),
            ManagedToolErrorCode::ToolMissing,
        )?;
        for entry in
            fs::read_dir(&root).map_err(|_| invalid_package(ManagedToolErrorCode::ToolMissing))?
        {
            let entry =
                entry.map_err(|_| invalid_package(ManagedToolErrorCode::ToolIntegrityFailed))?;
            if !expected.remove(&entry.file_name()) {
                return Err(invalid_package(ManagedToolErrorCode::ToolIntegrityFailed));
            }
        }
        if !expected.is_empty() {
            return Err(invalid_package(ManagedToolErrorCode::ToolMissing));
        }

        let visible = fs::read(root.join("manifest.json"))
            .map_err(|_| invalid_package(ManagedToolErrorCode::ToolMissing))?;
        if visible.as_slice() != self.manifest_bytes.as_ref() {
            return Err(invalid_package(ManagedToolErrorCode::ToolIntegrityFailed));
        }
        let canonical_app = fs::canonicalize(app_root)
            .map_err(|_| invalid_package(ManagedToolErrorCode::ToolIntegrityFailed))?;
        let canonical_resources = fs::canonicalize(resources)
            .map_err(|_| invalid_package(ManagedToolErrorCode::ToolIntegrityFailed))?;
        let root = fs::canonicalize(root)
            .map_err(|_| invalid_package(ManagedToolErrorCode::ToolMissing))?;
        if canonical_resources.parent() != Some(canonical_app.as_path())
            || root.parent() != Some(canonical_resources.as_path())
        {
            return Err(invalid_package(ManagedToolErrorCode::ToolIntegrityFailed));
        }
        for tool in manifest.tools() {
            let path = root.join(tool.name.artifact_name(&self.target));
            let canonical = fs::canonicalize(path)
                .map_err(|_| invalid_package(ManagedToolErrorCode::ToolIntegrityFailed))?;
            if canonical.parent() != Some(root.as_path()) {
                return Err(invalid_package(ManagedToolErrorCode::ToolIntegrityFailed));
            }
        }
        Ok(TrustedToolPackage::new(manifest.clone(), root))
    }
}

fn require_ordinary_directory(path: &Path) -> Result<(), ManagedToolError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| invalid_package(ManagedToolErrorCode::ToolMissing))?;
    if !metadata.is_dir() || is_reparse_point(&metadata) {
        return Err(invalid_package(ManagedToolErrorCode::ToolIntegrityFailed));
    }
    Ok(())
}

fn require_ordinary_file(
    path: &Path,
    missing_code: ManagedToolErrorCode,
) -> Result<(), ManagedToolError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            invalid_package(missing_code)
        } else {
            invalid_package(ManagedToolErrorCode::ToolIntegrityFailed)
        }
    })?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(invalid_package(ManagedToolErrorCode::ToolIntegrityFailed));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn invalid_package(code: ManagedToolErrorCode) -> ManagedToolError {
    ManagedToolError::new(code, "bundled managed-tool package is invalid")
}
