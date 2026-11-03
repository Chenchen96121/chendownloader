use std::path::{Path, PathBuf};

use crate::{
    application::tool_manifest::ValidatedManifest, domain::managed_tools::ManagedToolError,
};

pub const EMBEDDED_TOOL_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/managed-tools/manifest.json"
));

pub struct TrustedToolPackage {
    manifest: ValidatedManifest,
    package_root: PathBuf,
}

impl TrustedToolPackage {
    pub fn new(manifest: ValidatedManifest, package_root: PathBuf) -> Self {
        Self {
            manifest,
            package_root,
        }
    }

    pub fn manifest(&self) -> &ValidatedManifest {
        &self.manifest
    }

    pub fn package_root(&self) -> &Path {
        &self.package_root
    }
}

pub trait TrustedToolPackageSource: Send + Sync {
    fn load_manifest(&self) -> Result<ValidatedManifest, ManagedToolError>;

    fn load_package(
        &self,
        manifest: &ValidatedManifest,
    ) -> Result<TrustedToolPackage, ManagedToolError>;
}
