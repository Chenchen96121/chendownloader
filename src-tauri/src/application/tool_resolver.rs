use std::{path::PathBuf, sync::Arc};

use crate::domain::managed_tools::{ManagedToolError, ToolName};

pub struct ResolvedTool {
    pub name: ToolName,
    pub version: String,
    pub path: PathBuf,
}

pub trait ManagedToolResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError>;
}

impl<T: ManagedToolResolver + ?Sized> ManagedToolResolver for &T {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        (**self).resolve_verified(name)
    }
}

impl<T: ManagedToolResolver + ?Sized> ManagedToolResolver for Arc<T> {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        (**self).resolve_verified(name)
    }
}
