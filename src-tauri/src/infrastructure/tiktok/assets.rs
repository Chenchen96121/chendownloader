use super::{inventory::network_json, tool::TikTokToolResolver};
use crate::{
    application::{
        preview_encoding::encode_base64,
        process::ProcessSupervisor,
        tiktok_analysis::TikTokPreviewLoader,
        tiktok_source::{TikTokRequestContext, TikTokSourceItem},
        tool_resolver::ManagedToolResolver,
    },
    domain::{download::DownloadErrorCode, media::AnalysisErrorCode, tiktok::TikTokPreviewPayload},
    infrastructure::image_verification::{ImageStaging, VerifiedImage},
};
use serde_json::json;
use std::{fs, path::Path, sync::Arc, time::Instant};

pub struct TikTokAssetAdapter<R, P> {
    resolver: R,
    process: P,
    tool: Arc<TikTokToolResolver>,
}
impl<R, P> TikTokAssetAdapter<R, P> {
    pub fn new(resolver: R, process: P, tool: Arc<TikTokToolResolver>) -> Self {
        Self {
            resolver,
            process,
            tool,
        }
    }
}
impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync>
    TikTokAssetAdapter<R, P>
{
    pub(crate) fn fetch_image(
        &self,
        item: &TikTokSourceItem,
        directory: &Path,
        preview: bool,
        context: &TikTokRequestContext,
    ) -> Result<VerifiedImage, DownloadErrorCode> {
        if context.cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        let native = item
            .native
            .as_ref()
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        let url = native
            .data
            .get("sourcePageUrl")
            .and_then(|v| v.as_str())
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        let mut asset = native.data.clone();
        let object = asset
            .as_object_mut()
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        let maximum = if preview {
            8 * 1024 * 1024
        } else {
            512 * 1024 * 1024
        };
        object.insert("kind".into(), "image".into());
        object.insert("maximumBytes".into(), maximum.into());
        if preview {
            object.insert(
                "urls".into(),
                json!([item
                    .preview
                    .as_ref()
                    .ok_or(DownloadErrorCode::FormatUnavailable)?
                    .as_str()]),
            );
            object.remove("fileSize");
            object.remove("candidates");
        }
        let staging = ImageStaging::new(directory)?;
        let file = staging.directory().join("stage.part");
        let seconds = context
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs();
        if seconds == 0 {
            return Err(DownloadErrorCode::DownloadTimeout);
        }
        // A private file response keeps an 8 MiB preview below the 4 MiB JSONL limit.
        self.tool.run_download(&json!({"protocolVersion":1,"operation":"download","url":url,"network":network_json(context),"context":native.context,"asset":asset,"stagingPath":file}),seconds,context.cancellation.clone(),None)?;
        staging.verify(
            &self.resolver,
            &self.process,
            if preview { None } else { item.width },
            if preview { None } else { item.height },
            maximum,
            &context.cancellation,
        )
    }
}
impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync> TikTokPreviewLoader
    for TikTokAssetAdapter<R, P>
{
    fn load(
        &self,
        item: &TikTokSourceItem,
        context: &TikTokRequestContext,
    ) -> Result<Option<TikTokPreviewPayload>, AnalysisErrorCode> {
        if item.preview.is_none() {
            return Ok(None);
        }
        let directory = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let image = self
            .fetch_image(item, directory.path(), true, context)
            .map_err(|error| match error {
                DownloadErrorCode::ToolMissing => AnalysisErrorCode::ToolMissing,
                DownloadErrorCode::ToolIntegrityFailed => AnalysisErrorCode::ToolIntegrityFailed,
                DownloadErrorCode::NetworkFailed => AnalysisErrorCode::NetworkFailed,
                DownloadErrorCode::DownloadTimeout => AnalysisErrorCode::AnalysisTimeout,
                _ => AnalysisErrorCode::InvalidMetadata,
            })?;
        if u64::from(image.width()) * u64::from(image.height()) > 16_000_000 {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        let bytes = fs::read(image.path()).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if bytes.len() > 8 * 1024 * 1024 || bytes.len() as u64 != image.byte_length() {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(Some(TikTokPreviewPayload {
            mime_type: image
                .mime_type()
                .ok_or(AnalysisErrorCode::InvalidMetadata)?
                .into(),
            data_base64: encode_base64(&bytes),
        }))
    }
}

impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync>
    crate::application::tiktok_download_job::TikTokImageDownloader for TikTokAssetAdapter<R, P>
{
    fn download(
        &self,
        item: &TikTokSourceItem,
        directory: &Path,
        context: &TikTokRequestContext,
    ) -> Result<crate::application::tiktok_download_job::TikTokVerifiedAsset, DownloadErrorCode>
    {
        self.fetch_image(item, directory, false, context)
            .map(crate::application::tiktok_download_job::TikTokVerifiedAsset::Image)
    }
}
