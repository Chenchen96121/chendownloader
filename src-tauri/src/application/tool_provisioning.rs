use crate::{
    application::{
        portable_package::TrustedToolPackageSource,
        tool_bootstrap::{ManagedToolStore, ToolBootstrapService, ToolVersionProbe},
    },
    domain::managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolInventory},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvisioningMode {
    Startup,
    Repair,
}

pub trait ToolProvisioner {
    fn provision(&self, mode: ProvisioningMode) -> Result<ToolInventory, ManagedToolError>;
}

pub struct OfflineToolProvisioner<'a, S, P>
where
    S: TrustedToolPackageSource,
    P: ToolVersionProbe,
{
    source: &'a S,
    store: &'a dyn ManagedToolStore,
    probe: &'a P,
    target: &'a str,
}

impl<'a, S, P> OfflineToolProvisioner<'a, S, P>
where
    S: TrustedToolPackageSource,
    P: ToolVersionProbe,
{
    pub fn new(
        source: &'a S,
        store: &'a dyn ManagedToolStore,
        probe: &'a P,
        target: &'a str,
    ) -> Self {
        Self {
            source,
            store,
            probe,
            target,
        }
    }
}

impl<S, P> ToolProvisioner for OfflineToolProvisioner<'_, S, P>
where
    S: TrustedToolPackageSource,
    P: ToolVersionProbe,
{
    fn provision(&self, mode: ProvisioningMode) -> Result<ToolInventory, ManagedToolError> {
        let manifest = self.source.load_manifest()?;
        if mode == ProvisioningMode::Startup {
            if let Some(inventory) = self.store.matching_inventory(&manifest) {
                return Ok(inventory);
            }
        }

        let package = self.source.load_package(&manifest)?;
        if package.manifest() != &manifest {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::ToolIntegrityFailed,
                "trusted package manifest changed during provisioning",
            ));
        }
        ToolBootstrapService::new(self.store, self.probe, self.target)
            .bootstrap_validated(package.package_root(), package.manifest())
    }
}
