use crate::domain::managed_tools::ToolInventory;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub app_version: String,
    pub local_only: bool,
    pub managed_tools_configured: bool,
}

impl RuntimeStatus {
    pub fn baseline(app_version: &str) -> Self {
        Self {
            app_version: app_version.to_owned(),
            local_only: true,
            managed_tools_configured: false,
        }
    }

    pub fn from_inventory(app_version: &str, inventory: &ToolInventory) -> Self {
        Self {
            app_version: app_version.to_owned(),
            local_only: true,
            managed_tools_configured: inventory.managed_tools_configured(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeStatus;

    #[test]
    fn baseline_is_local_only_and_has_no_managed_tools() {
        let status = RuntimeStatus::baseline("0.1.0");
        assert!(status.local_only);
        assert!(!status.managed_tools_configured);
        assert_eq!(status.app_version, "0.1.0");
    }
}
