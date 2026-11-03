use std::{path::PathBuf, time::Instant};

use crate::{
    application::destination::{
        approve_selected_destination, ApprovedDestination, DestinationRegistry,
    },
    application::destination_settings::{DefaultDestinationStatus, DestinationSettings},
    application::download_job::DownloadDestinationResolver,
    domain::download::{DestinationApproval, DownloadErrorCode},
    infrastructure::destination::FilesystemDestinationValidator,
};

pub struct DestinationRuntime {
    registry: DestinationRegistry<FilesystemDestinationValidator>,
    settings: DestinationSettings<FilesystemDestinationValidator>,
}

impl DownloadDestinationResolver for DestinationRuntime {
    fn resolve_download_destination(
        &self,
        destination_id: &str,
    ) -> Result<PathBuf, DownloadErrorCode> {
        self.resolve(destination_id)
            .map(ApprovedDestination::into_path)
    }
}

impl DestinationRuntime {
    pub fn new(app_local_data: PathBuf) -> Self {
        Self {
            registry: DestinationRegistry::new(FilesystemDestinationValidator),
            settings: DestinationSettings::new(
                app_local_data.join("settings/default-destination.json"),
                FilesystemDestinationValidator,
            ),
        }
    }

    pub fn load_default(&self) -> DefaultDestinationStatus {
        self.settings.status()
    }

    pub fn choose_default(
        &self,
        selected: Option<PathBuf>,
    ) -> Result<Option<DefaultDestinationStatus>, DownloadErrorCode> {
        self.settings.choose(selected)
    }

    pub fn approve_default(&self) -> Result<DestinationApproval, DownloadErrorCode> {
        self.settings.approve(&self.registry, Instant::now())
    }

    pub fn approve_selected(
        &self,
        selected: Option<PathBuf>,
    ) -> Result<Option<DestinationApproval>, DownloadErrorCode> {
        approve_selected_destination(&self.registry, selected, Instant::now())
    }

    pub fn resolve(&self, destination_id: &str) -> Result<ApprovedDestination, DownloadErrorCode> {
        self.registry.resolve(destination_id, Instant::now())
    }

    pub fn clear(&self) {
        self.registry.clear();
    }
}
