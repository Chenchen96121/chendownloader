mod download;
mod errors;
mod metadata;

use crate::application::{
    media_url::ValidatedMediaUrl,
    process::{CancellationToken, ProcessSupervisor},
    tool_resolver::ManagedToolResolver,
};
use crate::domain::{download::DownloadErrorCode, media::AnalysisErrorCode};
use std::path::Path;

pub use crate::infrastructure::image_verification::VerifiedImage as VerifiedInstagramImage;
pub(crate) use download::acquire_direct_image;
pub use metadata::{normalize_instagram_inventory, InstagramInventory, ResolvedInstagramItem};

pub struct GalleryDlInstagramAdapter<R, P> {
    resolver: R,
    process: P,
}

impl<R, P> GalleryDlInstagramAdapter<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}

pub trait InstagramInventoryExtractor {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode>;
}

pub struct FallbackInstagramInventoryExtractor<P, F> {
    primary: P,
    fallback: F,
}

impl<P, F> FallbackInstagramInventoryExtractor<P, F> {
    pub fn new(primary: P, fallback: F) -> Self {
        Self { primary, fallback }
    }
}

impl<P, F> InstagramInventoryExtractor for FallbackInstagramInventoryExtractor<P, F>
where
    P: InstagramInventoryExtractor,
    F: InstagramInventoryExtractor,
{
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        match self.primary.extract_post(source, cancellation) {
            Err(AnalysisErrorCode::AuthenticationRequired | AnalysisErrorCode::AnalysisTimeout)
                if !cancellation.is_canceled() =>
            {
                self.fallback.extract_post(source, cancellation)
            }
            result => result,
        }
    }
}

pub trait InstagramAssetAcquirer {
    fn fetch_preview(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<VerifiedInstagramImage>, AnalysisErrorCode>;

    fn download_image(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<VerifiedInstagramImage, DownloadErrorCode>;
}

pub struct FallbackInstagramAssetAcquirer<P, F> {
    primary: P,
    fallback: F,
}

impl<P, F> FallbackInstagramAssetAcquirer<P, F> {
    pub fn new(primary: P, fallback: F) -> Self {
        Self { primary, fallback }
    }
}

impl<P, F> InstagramAssetAcquirer for FallbackInstagramAssetAcquirer<P, F>
where
    P: InstagramAssetAcquirer,
    F: InstagramAssetAcquirer,
{
    fn fetch_preview(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<VerifiedInstagramImage>, AnalysisErrorCode> {
        if item.prefers_direct_asset() {
            return self
                .fallback
                .fetch_preview(item, protected_directory, cancellation);
        }
        match self
            .primary
            .fetch_preview(item, protected_directory, cancellation)
        {
            Err(AnalysisErrorCode::AuthenticationRequired) if !cancellation.is_canceled() => self
                .fallback
                .fetch_preview(item, protected_directory, cancellation),
            result => result,
        }
    }

    fn download_image(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<VerifiedInstagramImage, DownloadErrorCode> {
        if item.prefers_direct_asset() {
            return self
                .fallback
                .download_image(item, protected_directory, cancellation);
        }
        match self
            .primary
            .download_image(item, protected_directory, cancellation)
        {
            Err(DownloadErrorCode::AuthenticationRequired) if !cancellation.is_canceled() => self
                .fallback
                .download_image(item, protected_directory, cancellation),
            result => result,
        }
    }
}

impl<R, P> InstagramInventoryExtractor for GalleryDlInstagramAdapter<R, P>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        metadata::extract_post(self, source, cancellation)
    }
}

impl<R, P> InstagramAssetAcquirer for GalleryDlInstagramAdapter<R, P>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    fn fetch_preview(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<VerifiedInstagramImage>, AnalysisErrorCode> {
        download::fetch_preview(self, item, protected_directory, cancellation)
    }

    fn download_image(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<VerifiedInstagramImage, DownloadErrorCode> {
        download::download_image(self, item, protected_directory, cancellation)
    }
}
