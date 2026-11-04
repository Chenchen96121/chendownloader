use crate::{
    application::{
        network_profile::NetworkProfile,
        tiktok_source::{
            TikTokAttachmentIdentity, TikTokInventory, TikTokNativeAsset, TikTokRequestContext,
        },
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::{select_formats, SelectionIntent, SelectionMode, SelectionPlan},
        media::{AnalysisErrorCode, FileSize, MediaFormat, MediaInfo, Platform, StreamKind},
    },
};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct VerifiedTikTokDownloadSource {
    identity: TikTokAttachmentIdentity,
    pub(crate) native: Arc<TikTokNativeAsset>,
    pub(crate) candidates: Vec<Value>,
    pub(crate) formats: Vec<MediaFormat>,
    plan: SelectionPlan,
    network: Arc<NetworkProfile>,
}
impl VerifiedTikTokDownloadSource {
    pub fn identity(&self) -> &TikTokAttachmentIdentity {
        &self.identity
    }
    pub fn network(&self) -> Arc<NetworkProfile> {
        self.network.clone()
    }
    pub(crate) fn asset(&self, id: &str) -> Result<Value, DownloadErrorCode> {
        let index = self
            .formats
            .iter()
            .position(|f| f.id == id)
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        if !self
            .plan
            .source_format_ids
            .iter()
            .any(|selected| selected == id)
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let mut asset = self.candidates[index].clone();
        let object = asset
            .as_object_mut()
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        object.insert("kind".into(), "video".into());
        object.remove("requestContext");
        Ok(asset)
    }
    pub(crate) fn context(&self, id: &str) -> Result<Value, DownloadErrorCode> {
        let index = self
            .formats
            .iter()
            .position(|f| f.id == id)
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        Ok(self.candidates[index]
            .get("requestContext")
            .cloned()
            .unwrap_or_else(|| self.native.context.clone()))
    }
    pub(crate) fn validate_request(
        &self,
        plan: &SelectionPlan,
        video: Option<&MediaFormat>,
        audio: Option<&MediaFormat>,
    ) -> Result<(), DownloadErrorCode> {
        if plan != &self.plan
            || video
                != self
                    .formats
                    .iter()
                    .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
            || audio
                != self
                    .formats
                    .iter()
                    .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(())
    }
}
pub struct ResolvedTikTokVideo {
    media: MediaInfo,
    plan: SelectionPlan,
    source: VerifiedTikTokDownloadSource,
}
impl ResolvedTikTokVideo {
    pub fn media(&self) -> &MediaInfo {
        &self.media
    }
    pub fn plan(&self) -> &SelectionPlan {
        &self.plan
    }
    pub fn source(&self) -> &VerifiedTikTokDownloadSource {
        &self.source
    }
    pub fn into_parts(self) -> (MediaInfo, SelectionPlan, VerifiedTikTokDownloadSource) {
        (self.media, self.plan, self.source)
    }
}
pub trait TikTokVideoAnalyzer: Send + Sync {
    fn analyze_item(
        &self,
        inventory: &TikTokInventory,
        identity: &TikTokAttachmentIdentity,
        compatibility_mode: bool,
        context: &TikTokRequestContext,
    ) -> Result<ResolvedTikTokVideo, AnalysisErrorCode>;
}
pub trait TikTokCandidateProbe: Send + Sync {
    fn probe(
        &self,
        native: &TikTokNativeAsset,
        candidate: &Value,
        context: &TikTokRequestContext,
    ) -> Result<Value, AnalysisErrorCode>;
}
pub fn resolve_tiktok_video(
    inventory: &TikTokInventory,
    identity: &TikTokAttachmentIdentity,
    compatibility: bool,
    context: &TikTokRequestContext,
) -> Result<ResolvedTikTokVideo, AnalysisErrorCode> {
    let item = inventory
        .items
        .iter()
        .find(|item| item.identity.as_ref() == Some(identity))
        .ok_or(AnalysisErrorCode::ContentChanged)?;
    let native = item
        .native
        .clone()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let candidates = native
        .data
        .get("candidates")
        .and_then(Value::as_array)
        .filter(|rows| !rows.is_empty() && rows.len() <= 512)
        .ok_or(AnalysisErrorCode::FormatUnavailable)?
        .clone();
    let formats = candidates
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let dim = |key: &str| {
                row.get(key)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            let codec = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_owned);
            let file_size = row
                .get("fileSize")
                .filter(|v| !v.is_null())
                .and_then(|v| serde_json::from_value::<FileSize>(v.clone()).ok());
            MediaFormat {
                id: format!("fmt-{index}"),
                container: "mp4".into(),
                kind: if row.get("audioAbsent").and_then(Value::as_bool) == Some(true) {
                    StreamKind::VideoOnly
                } else {
                    StreamKind::Combined
                },
                width: dim("width"),
                height: dim("height"),
                fps: row.get("fps").and_then(Value::as_f64),
                video_bitrate_kbps: row.get("videoBitrateKbps").and_then(Value::as_f64),
                audio_bitrate_kbps: row.get("audioBitrateKbps").and_then(Value::as_f64),
                audio_sample_rate_hz: row
                    .get("audioSampleRateHz")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok()),
                audio_channels: row
                    .get("audioChannels")
                    .and_then(Value::as_u64)
                    .and_then(|n| u8::try_from(n).ok()),
                video_codec: codec("videoCodec"),
                audio_codec: codec("audioCodec"),
                dynamic_range: row
                    .get("dynamicRange")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                file_size,
            }
        })
        .collect::<Vec<_>>();
    let media = MediaInfo {
        platform: Platform::Tiktok,
        media_id: identity.media_id.clone(),
        title: inventory.title.clone(),
        creator: inventory.creator.clone(),
        duration_seconds: item.duration_seconds,
        formats,
    };
    media.validate()?;
    let plan = select_formats(
        &media,
        &SelectionIntent {
            mode: if media
                .formats
                .iter()
                .all(|f| f.kind == StreamKind::VideoOnly)
            {
                SelectionMode::VideoOnly
            } else {
                SelectionMode::BestAvailable
            },
            compatibility_mode: compatibility,
            video_format_id: None,
            output_container: None,
            clip: None,
        },
    )
    .map_err(|_| AnalysisErrorCode::FormatUnavailable)?;
    let source = VerifiedTikTokDownloadSource {
        identity: identity.clone(),
        native,
        candidates,
        formats: media.formats.clone(),
        plan: plan.clone(),
        network: context.network.clone(),
    };
    Ok(ResolvedTikTokVideo {
        media,
        plan,
        source,
    })
}

pub struct NativeTikTokVideoAnalyzer;
impl TikTokVideoAnalyzer for NativeTikTokVideoAnalyzer {
    fn analyze_item(
        &self,
        inventory: &TikTokInventory,
        identity: &TikTokAttachmentIdentity,
        compatibility_mode: bool,
        context: &TikTokRequestContext,
    ) -> Result<ResolvedTikTokVideo, AnalysisErrorCode> {
        resolve_tiktok_video(inventory, identity, compatibility_mode, context)
    }
}
