use super::media_url::ValidatedMediaUrl;
use super::{network_profile::NetworkProfile, process::CancellationToken};
use crate::domain::{
    media::AnalysisErrorCode,
    x::{XItemAvailability, XMediaKind},
};
use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub struct XAttachmentIdentity {
    pub post_id: String,
    pub media_id: String,
    pub media_key: Option<String>,
}
#[derive(Clone)]
pub struct XImageCandidate {
    pub url: Url,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
#[derive(Clone)]
pub struct XVideoCandidate {
    pub url: Url,
    pub content_type: String,
    pub bitrate: Option<f64>,
}
#[derive(Clone)]
pub struct XSourceItem {
    pub identity: Option<XAttachmentIdentity>,
    pub ordinal: u16,
    pub kind: XMediaKind,
    pub image_candidates: Vec<XImageCandidate>,
    pub video_candidates: Vec<XVideoCandidate>,
    pub preview: Option<Url>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<f64>,
    pub availability: XItemAvailability,
    pub unavailable_reason: Option<AnalysisErrorCode>,
}
#[derive(Clone)]
pub struct XInventory {
    pub source: ValidatedMediaUrl,
    pub title: String,
    pub creator: Option<String>,
    pub items: Vec<XSourceItem>,
}
pub trait XInventoryExtractor: Send + Sync {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        context: &XRequestContext,
    ) -> Result<XInventory, AnalysisErrorCode>;
}
use std::{sync::Arc, time::Instant};

#[derive(Clone)]
pub struct XRequestContext {
    pub cancellation: CancellationToken,
    pub deadline: Instant,
    pub network: Arc<NetworkProfile>,
}
impl XRequestContext {
    pub fn new(
        cancellation: CancellationToken,
        deadline: Instant,
        network: Arc<NetworkProfile>,
    ) -> Self {
        Self {
            cancellation,
            deadline,
            network,
        }
    }
}
