//! Safe native covers, with one shared HTTP+verification deadline.
use super::url_policy::{validate_bilibili_asset_url, BilibiliAssetKind};
use crate::{
    application::{
        bilibili_analysis::BilibiliCoverLoader,
        bilibili_source::{BilibiliPageEvidence, BilibiliRequestContext},
        preview_encoding::encode_base64,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        bilibili::BilibiliCoverPayload, download::DownloadErrorCode, media::AnalysisErrorCode,
    },
    infrastructure::image_verification::ImageStaging,
};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub trait BilibiliCoverTransport: Send + Sync {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        context: &BilibiliRequestContext,
    ) -> Result<(), AnalysisErrorCode>;
}
pub struct HttpBilibiliCoverLoader<R, P, T = HttpBilibiliCoverTransport> {
    resolver: R,
    process: P,
    transport: T,
}
impl<R, P> HttpBilibiliCoverLoader<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self::with_transport(resolver, process, HttpBilibiliCoverTransport)
    }
}
impl<R, P, T> HttpBilibiliCoverLoader<R, P, T> {
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
        T: BilibiliCoverTransport,
    > BilibiliCoverLoader for HttpBilibiliCoverLoader<R, P, T>
{
    fn load(
        &self,
        evidence: &BilibiliPageEvidence,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        let remaining = context.remaining(Duration::from_secs(15))?;
        let bounded = BilibiliRequestContext::new(
            context.cancellation(),
            Instant::now() + remaining,
            context.network(),
        );
        let url = validate_bilibili_asset_url(
            evidence
                .cover_url()
                .ok_or(AnalysisErrorCode::FormatUnavailable)?
                .as_str(),
            BilibiliAssetKind::Cover,
        )?;
        let directory = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let staging = ImageStaging::new(directory.path()).map_err(image_error)?;
        self.transport
            .fetch(&url, &staging.directory().join("stage.download"), &bounded)?;
        bounded.remaining(Duration::from_secs(15))?;
        let image = staging
            .verify(
                &self.resolver,
                &BudgetProcess {
                    process: &self.process,
                    context: &bounded,
                },
                None,
                None,
                MAX_BYTES,
                &bounded.cancellation(),
            )
            .map_err(image_error)?;
        bounded.remaining(Duration::from_secs(15))?;
        let mime = image
            .mime_type()
            .filter(|m| matches!(*m, "image/jpeg" | "image/png" | "image/webp"))
            .ok_or(AnalysisErrorCode::UnsupportedMedia)?;
        let bytes = fs::read(image.path()).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if bytes.len() as u64 != image.byte_length() || bytes.len() as u64 > MAX_BYTES {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        image
            .verify_byte_identical_copy(image.path())
            .map_err(image_error)?;
        let result = BilibiliCoverPayload {
            mime_type: mime.into(),
            data_base64: encode_base64(&bytes),
        };
        bounded.remaining(Duration::from_secs(15))?;
        if !result.valid() {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(result)
    }
}
struct BudgetProcess<'a, P> {
    process: &'a P,
    context: &'a BilibiliRequestContext,
}
impl<P: ProcessSupervisor> ProcessSupervisor for BudgetProcess<'_, P> {
    fn run(
        &self,
        mut request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let remaining = self
            .context
            .remaining(Duration::from_secs(15))
            .map_err(process_error)?;
        request.timeout = Some(request.timeout.unwrap_or(remaining).min(remaining));
        match &mut request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => {
                *stdout_limit = (*stdout_limit).min(65536);
                *stderr_limit = (*stderr_limit).min(65536);
            }
            _ => return Err(ProcessFailureKind::IoFailed),
        }
        let result = self.process.run(request, cancellation, sink)?;
        self.context
            .remaining(Duration::from_secs(15))
            .map_err(process_error)?;
        Ok(result)
    }
}
fn process_error(error: AnalysisErrorCode) -> ProcessFailureKind {
    if error == AnalysisErrorCode::AnalysisTimeout {
        ProcessFailureKind::TimedOut
    } else {
        ProcessFailureKind::Canceled
    }
}
fn image_error(error: DownloadErrorCode) -> AnalysisErrorCode {
    match error {
        DownloadErrorCode::ToolMissing => AnalysisErrorCode::ToolMissing,
        DownloadErrorCode::ToolIntegrityFailed => AnalysisErrorCode::ToolIntegrityFailed,
        DownloadErrorCode::DownloadTimeout => AnalysisErrorCode::AnalysisTimeout,
        DownloadErrorCode::ProcessFailed => AnalysisErrorCode::ProcessFailed,
        _ => AnalysisErrorCode::InvalidMetadata,
    }
}
pub struct HttpBilibiliCoverTransport;
impl BilibiliCoverTransport for HttpBilibiliCoverTransport {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        context: &BilibiliRequestContext,
    ) -> Result<(), AnalysisErrorCode> {
        let url = validate_bilibili_asset_url(url.as_str(), BilibiliAssetKind::Cover)?;
        let timeout = context.remaining(Duration::from_secs(15))?;
        let network = context.network();
        let mut builder = reqwest::blocking::Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout);
        if let Some(proxy) =
            network.https_proxy_for_host(url.host_str().ok_or(AnalysisErrorCode::InvalidUrl)?)
        {
            builder = builder.proxy(
                reqwest::Proxy::https(proxy.as_str())
                    .map_err(|_| AnalysisErrorCode::NetworkFailed)?,
            );
        }
        let mut response = builder
            .build()
            .map_err(|_| AnalysisErrorCode::NetworkFailed)?
            .get(url)
            .header("User-Agent", "Mozilla/5.0")
            .header("Referer", "https://www.bilibili.com/")
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    AnalysisErrorCode::AnalysisTimeout
                } else {
                    AnalysisErrorCode::NetworkFailed
                }
            })?;
        match response.status().as_u16() {
            200 => {}
            401 => return Err(AnalysisErrorCode::AuthenticationRequired),
            403 => return Err(AnalysisErrorCode::AccessDenied),
            404 => return Err(AnalysisErrorCode::ContentUnavailable),
            429 => return Err(AnalysisErrorCode::RateLimited),
            _ => return Err(AnalysisErrorCode::NetworkFailed),
        }
        if response
            .content_length()
            .is_some_and(|n| n == 0 || n > MAX_BYTES)
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let mut total = 0_u64;
        let mut buffer = [0_u8; 16384];
        loop {
            context.remaining(Duration::from_secs(15))?;
            let count = response
                .read(&mut buffer)
                .map_err(|_| AnalysisErrorCode::NetworkFailed)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_BYTES {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            output
                .write_all(&buffer[..count])
                .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        }
        if total == 0 {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        context.remaining(Duration::from_secs(15))?;
        Ok(())
    }
}
