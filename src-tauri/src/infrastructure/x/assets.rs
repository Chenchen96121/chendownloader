use super::url_policy::{validate_x_asset_url, XAssetKind};
use crate::application::preview_encoding::encode_base64;
use crate::{
    application::{
        process::ProcessSupervisor,
        tool_resolver::ManagedToolResolver,
        x_analysis::XPreviewLoader,
        x_download_job::{XImageDownloader, XVerifiedAsset},
        x_source::{XRequestContext, XSourceItem},
    },
    domain::{
        download::DownloadErrorCode,
        media::AnalysisErrorCode,
        x::{XItemAvailability, XMediaKind, XPreviewPayload},
    },
    infrastructure::image_verification::{ImageStaging, VerifiedImage},
};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
pub trait XAssetTransport: Send + Sync {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<(), AnalysisErrorCode>;
}
pub struct XAssetAdapter<R, P, T = HttpXAssetTransport> {
    resolver: R,
    process: P,
    transport: T,
}
impl<R, P> XAssetAdapter<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self::with_transport(resolver, process, HttpXAssetTransport)
    }
}
impl<R, P, T> XAssetAdapter<R, P, T> {
    pub fn with_transport(resolver: R, process: P, transport: T) -> Self {
        Self {
            resolver,
            process,
            transport,
        }
    }
}
impl<
        R: ManagedToolResolver + Send + Sync,
        P: ProcessSupervisor + Send + Sync,
        T: XAssetTransport,
    > XImageDownloader for XAssetAdapter<R, P, T>
{
    fn download(
        &self,
        item: &XSourceItem,
        directory: &Path,
        context: &XRequestContext,
    ) -> Result<XVerifiedAsset, DownloadErrorCode> {
        check_context(context).map_err(crate::application::x_analysis::map_error)?;
        if item.kind != XMediaKind::Image
            || item.availability != XItemAvailability::Ready
            || item.identity.is_none()
            || item.image_candidates.len() != 1
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let candidate = &item.image_candidates[0];
        let url = validate_x_asset_url(candidate.url.as_str(), XAssetKind::Image)
            .map_err(|_| DownloadErrorCode::InvalidRequest)?;
        let parameters = url.query_pairs().collect::<Vec<_>>();
        if !url.path().starts_with("/media/")
            || parameters.len() != 2
            || parameters
                .iter()
                .filter(|(k, v)| k == "name" && v == "orig")
                .count()
                != 1
            || parameters
                .iter()
                .filter(|(k, v)| {
                    k == "format" && matches!(v.as_ref(), "jpg" | "jpeg" | "png" | "webp" | "avif")
                })
                .count()
                != 1
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let staging = ImageStaging::new(directory)?;
        self.transport
            .fetch(
                &url,
                &staging.directory().join("stage.download"),
                MAX_IMAGE_BYTES,
                context,
            )
            .map_err(crate::application::x_analysis::map_error)?;
        check_context(context).map_err(crate::application::x_analysis::map_error)?;
        let image = staging.verify(
            &self.resolver,
            &self.process,
            candidate.width,
            candidate.height,
            MAX_IMAGE_BYTES,
            &context.cancellation,
        )?;
        check_context(context).map_err(crate::application::x_analysis::map_error)?;
        Ok(XVerifiedAsset::Image(image))
    }
}
impl<
        R: ManagedToolResolver + Send + Sync,
        P: ProcessSupervisor + Send + Sync,
        T: XAssetTransport,
    > XPreviewLoader for XAssetAdapter<R, P, T>
{
    fn load(
        &self,
        item: &XSourceItem,
        context: &XRequestContext,
    ) -> Result<Option<XPreviewPayload>, AnalysisErrorCode> {
        check_context(context)?;
        let Some(url) = item.preview.as_ref() else {
            return Ok(None);
        };
        let url = validate_x_asset_url(url.as_str(), XAssetKind::Image)?;
        let directory = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let staging = ImageStaging::new(directory.path()).map_err(image_analysis_error)?;
        self.transport.fetch(
            &url,
            &staging.directory().join("stage.download"),
            MAX_PREVIEW_BYTES,
            context,
        )?;
        check_context(context)?;
        // Covers can have different dimensions from the original attachment.
        let image: VerifiedImage = staging
            .verify(
                &self.resolver,
                &self.process,
                None,
                None,
                MAX_PREVIEW_BYTES,
                &context.cancellation,
            )
            .map_err(image_analysis_error)?;
        check_context(context)?;
        let bytes = fs::read(image.path()).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if bytes.len() as u64 != image.byte_length() || bytes.len() as u64 > MAX_PREVIEW_BYTES {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        image
            .verify_byte_identical_copy(image.path())
            .map_err(image_analysis_error)?;
        Ok(Some(XPreviewPayload {
            mime_type: image
                .mime_type()
                .ok_or(AnalysisErrorCode::UnsupportedMedia)?
                .into(),
            data_base64: encode_base64(&bytes),
        }))
    }
}

fn image_analysis_error(error: DownloadErrorCode) -> AnalysisErrorCode {
    match error {
        DownloadErrorCode::ToolMissing => AnalysisErrorCode::ToolMissing,
        DownloadErrorCode::ToolIntegrityFailed => AnalysisErrorCode::ToolIntegrityFailed,
        DownloadErrorCode::DownloadTimeout => AnalysisErrorCode::AnalysisTimeout,
        DownloadErrorCode::ProcessFailed => AnalysisErrorCode::ProcessFailed,
        _ => AnalysisErrorCode::InvalidMetadata,
    }
}
fn check_context(context: &XRequestContext) -> Result<(), AnalysisErrorCode> {
    if context.cancellation.is_canceled() {
        Err(AnalysisErrorCode::ProcessFailed)
    } else if Instant::now() >= context.deadline {
        Err(AnalysisErrorCode::AnalysisTimeout)
    } else {
        Ok(())
    }
}
pub struct HttpXAssetTransport;
impl XAssetTransport for HttpXAssetTransport {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<(), AnalysisErrorCode> {
        validate_x_asset_url(url.as_str(), XAssetKind::Image)?;
        check_context(context)?;
        if maximum == 0 || maximum > MAX_IMAGE_BYTES {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let timeout = context
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(AnalysisErrorCode::AnalysisTimeout)?
            .min(Duration::from_secs(120));
        let mut builder = reqwest::blocking::Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout);
        if let Some(proxy) = context
            .network
            .https_proxy_for_host(url.host_str().ok_or(AnalysisErrorCode::InvalidMetadata)?)
        {
            builder = builder.proxy(
                reqwest::Proxy::https(proxy.as_str())
                    .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?,
            );
        }
        let mut response = builder
            .build()
            .map_err(|_| AnalysisErrorCode::NetworkFailed)?
            .get(url.clone())
            .header("Referer", "https://x.com/")
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    AnalysisErrorCode::AnalysisTimeout
                } else {
                    AnalysisErrorCode::NetworkFailed
                }
            })?;
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 => AnalysisErrorCode::AuthenticationRequired,
                403 => AnalysisErrorCode::AccessDenied,
                404 => AnalysisErrorCode::ContentUnavailable,
                429 => AnalysisErrorCode::RateLimited,
                _ => AnalysisErrorCode::NetworkFailed,
            });
        }
        if response
            .content_length()
            .is_some_and(|n| n == 0 || n > maximum)
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let mut total = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            check_context(context)?;
            let n = response.read(&mut buffer).map_err(|e| {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    AnalysisErrorCode::AnalysisTimeout
                } else {
                    AnalysisErrorCode::NetworkFailed
                }
            })?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .filter(|n| *n <= maximum)
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            file.write_all(&buffer[..n])
                .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        }
        if total == 0 {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        file.sync_all()
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        check_context(context)
    }
}
