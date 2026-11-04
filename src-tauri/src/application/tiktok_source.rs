use super::media_url::ValidatedMediaUrl;
use super::{network_profile::NetworkProfile, process::CancellationToken};
use crate::domain::{
    media::AnalysisErrorCode,
    tiktok::{TikTokItemAvailability, TikTokMediaKind},
};
use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub struct TikTokAttachmentIdentity {
    pub post_id: String,
    pub media_id: String,
    pub media_key: Option<String>,
}
#[derive(Clone)]
pub struct TikTokImageCandidate {
    pub url: Url,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
#[derive(Clone)]
pub struct TikTokVideoCandidate {
    pub url: Url,
    pub content_type: String,
    pub bitrate: Option<f64>,
}
#[derive(Clone)]
pub struct TikTokSourceItem {
    pub native: Option<std::sync::Arc<TikTokNativeAsset>>,
    pub identity: Option<TikTokAttachmentIdentity>,
    pub ordinal: u16,
    pub kind: TikTokMediaKind,
    pub image_candidates: Vec<TikTokImageCandidate>,
    pub video_candidates: Vec<TikTokVideoCandidate>,
    pub preview: Option<Url>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<f64>,
    pub availability: TikTokItemAvailability,
    pub unavailable_reason: Option<AnalysisErrorCode>,
}
#[derive(Clone)]
pub struct TikTokInventory {
    pub source: ValidatedMediaUrl,
    pub title: String,
    pub creator: Option<String>,
    pub items: Vec<TikTokSourceItem>,
}
pub struct TikTokNativeAsset {
    pub data: serde_json::Value,
    pub context: serde_json::Value,
}
pub trait TikTokMetadataExtractor: Send + Sync {
    fn extract(
        &self,
        source: &ValidatedMediaUrl,
        context: &TikTokRequestContext,
    ) -> Result<serde_json::Value, AnalysisErrorCode>;
}
pub trait TikTokInventoryExtractor: Send + Sync {
    fn resolve_url(
        &self,
        input: &str,
        _: &TikTokRequestContext,
    ) -> Result<ValidatedMediaUrl, AnalysisErrorCode> {
        super::media_url::validate_media_url(input)
    }
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        context: &TikTokRequestContext,
    ) -> Result<TikTokInventory, AnalysisErrorCode>;
}
use std::{sync::Arc, time::Instant};

#[derive(Clone)]
pub struct TikTokRequestContext {
    pub prefetched: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
    pub cancellation: CancellationToken,
    pub deadline: Instant,
    pub network: Arc<NetworkProfile>,
}
impl TikTokRequestContext {
    pub fn new(
        cancellation: CancellationToken,
        deadline: Instant,
        network: Arc<NetworkProfile>,
    ) -> Self {
        Self {
            prefetched: Arc::new(std::sync::Mutex::new(None)),
            cancellation,
            deadline,
            network,
        }
    }
}
