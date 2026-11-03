//! Native Bilibili evidence and a single analysis network/deadline snapshot.
use super::{
    media_url::ValidatedMediaUrl, network_profile::NetworkProfile, process::CancellationToken,
};
use crate::domain::{bilibili::BilibiliVideoRequest, media::AnalysisErrorCode};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub struct BilibiliRequestContext {
    cancellation: CancellationToken,
    deadline: Instant,
    network: Arc<NetworkProfile>,
    expected_identity: Option<BilibiliPageIdentity>,
    expected_formats: Vec<crate::domain::media::MediaFormat>,
}
impl BilibiliRequestContext {
    pub fn new(
        cancellation: CancellationToken,
        deadline: Instant,
        network: Arc<NetworkProfile>,
    ) -> Self {
        Self {
            cancellation,
            deadline,
            network,
            expected_identity: None,
            expected_formats: Vec::new(),
        }
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub fn network(&self) -> Arc<NetworkProfile> {
        Arc::clone(&self.network)
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub fn remaining(&self, cap: Duration) -> Result<Duration, AnalysisErrorCode> {
        if self.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .map(|d| d.min(cap))
            .ok_or(AnalysisErrorCode::AnalysisTimeout)
    }
    pub(crate) fn bind_refresh(
        mut self,
        identity: BilibiliPageIdentity,
        formats: Vec<crate::domain::media::MediaFormat>,
    ) -> Self {
        self.expected_identity = Some(identity);
        self.expected_formats = formats;
        self
    }
    pub(crate) fn expected_identity(&self) -> Option<&BilibiliPageIdentity> {
        self.expected_identity.as_ref()
    }
    pub(crate) fn expected_formats(&self) -> &[crate::domain::media::MediaFormat] {
        &self.expected_formats
    }
}
#[derive(Clone, PartialEq)]
pub struct BilibiliPageIdentity {
    pub bvid: String,
    pub aid: String,
    pub part: u16,
    pub cid: String,
    pub duration_seconds: f64,
}
#[derive(Clone)]
pub struct BilibiliPageEvidence {
    pub(crate) identity: BilibiliPageIdentity,
    pub(crate) source: ValidatedMediaUrl,
    pub(crate) cover: Option<url::Url>,
}
impl BilibiliPageEvidence {
    pub fn identity(&self) -> &BilibiliPageIdentity {
        &self.identity
    }
    pub fn source(&self) -> &ValidatedMediaUrl {
        &self.source
    }
    pub fn cover_url(&self) -> Option<&url::Url> {
        self.cover.as_ref()
    }
}
pub trait BilibiliPageLoader: Send + Sync {
    fn inspect(
        &self,
        request: &BilibiliVideoRequest,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliPageEvidence, AnalysisErrorCode>;
}
impl<T: BilibiliPageLoader + ?Sized> BilibiliPageLoader for Arc<T> {
    fn inspect(
        &self,
        request: &BilibiliVideoRequest,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliPageEvidence, AnalysisErrorCode> {
        (**self).inspect(request, context)
    }
}

pub enum BilibiliAnalysisScope {
    Discover,
    RefreshSelected(Vec<String>),
}

pub trait BilibiliAnalyzer: Send + Sync {
    fn analyze(
        &self,
        request: &BilibiliVideoRequest,
        scope: BilibiliAnalysisScope,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliAnalyzedSource, AnalysisErrorCode>;
}
impl<T: BilibiliAnalyzer + ?Sized> BilibiliAnalyzer for Arc<T> {
    fn analyze(
        &self,
        request: &BilibiliVideoRequest,
        scope: BilibiliAnalysisScope,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliAnalyzedSource, AnalysisErrorCode> {
        (**self).analyze(request, scope, context)
    }
}

#[derive(Clone)]
pub struct BilibiliStreamSource {
    pub(crate) declared_fps: Option<f64>,
    pub(crate) format: crate::domain::media::MediaFormat,
    pub(crate) url: url::Url,
    pub(crate) video:
        Option<crate::infrastructure::ffprobe::stream_properties::VideoStreamProperties>,
    pub(crate) audio:
        Option<crate::infrastructure::ffprobe::stream_properties::AudioStreamProperties>,
}
impl BilibiliStreamSource {
    pub fn format(&self) -> &crate::domain::media::MediaFormat {
        &self.format
    }
    pub fn url(&self) -> &url::Url {
        &self.url
    }
    pub fn video(
        &self,
    ) -> Option<&crate::infrastructure::ffprobe::stream_properties::VideoStreamProperties> {
        self.video.as_ref()
    }
    pub fn audio(
        &self,
    ) -> Option<&crate::infrastructure::ffprobe::stream_properties::AudioStreamProperties> {
        self.audio.as_ref()
    }
}

#[derive(Clone)]
pub struct BilibiliAnalyzedSource {
    pub(crate) request: BilibiliVideoRequest,
    pub(crate) evidence: BilibiliPageEvidence,
    pub(crate) media: crate::domain::media::MediaInfo,
    pub(crate) streams: Vec<BilibiliStreamSource>,
}
impl BilibiliAnalyzedSource {
    pub fn media(&self) -> &crate::domain::media::MediaInfo {
        &self.media
    }
    pub fn evidence(&self) -> &BilibiliPageEvidence {
        &self.evidence
    }
    pub fn request(&self) -> &BilibiliVideoRequest {
        &self.request
    }
    pub fn stream(&self, id: &str) -> Option<&BilibiliStreamSource> {
        self.streams.iter().find(|s| s.format.id == id)
    }
}
