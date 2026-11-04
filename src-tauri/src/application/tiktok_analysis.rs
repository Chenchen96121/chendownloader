use super::{
    media_url::ValidatedMediaUrl,
    network_profile::NetworkProfile,
    process::CancellationToken,
    tiktok_source::{
        TikTokAttachmentIdentity, TikTokInventory, TikTokInventoryExtractor, TikTokRequestContext,
        TikTokSourceItem,
    },
    tiktok_video::TikTokVideoAnalyzer,
};
use crate::domain::{
    download::DownloadErrorCode,
    media::{AnalysisErrorCode, FileSize, Platform},
    tiktok::{
        TikTokAnalysisPreview, TikTokCapabilities, TikTokDownloadIntent, TikTokItemAvailability,
        TikTokMediaItem, TikTokMediaKind, TikTokPreviewPayload,
    },
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
pub trait TikTokPreviewLoader: Send + Sync {
    fn load(
        &self,
        item: &TikTokSourceItem,
        context: &TikTokRequestContext,
    ) -> Result<Option<TikTokPreviewPayload>, AnalysisErrorCode>;
}
#[derive(Clone)]
pub struct ResolvedTikTokDownloadItem {
    pub public: TikTokMediaItem,
    pub identity: TikTokAttachmentIdentity,
}
#[derive(Clone, PartialEq, Eq)]
pub struct TikTokInventorySlot {
    pub ordinal: u16,
    pub identity: Option<TikTokAttachmentIdentity>,
    pub kind: TikTokMediaKind,
}
#[derive(Clone)]
pub struct ResolvedTikTokDownload {
    pub source: ValidatedMediaUrl,
    pub title: String,
    pub creator: Option<String>,
    pub items: Vec<ResolvedTikTokDownloadItem>,
    pub network: Arc<NetworkProfile>,
    pub compatibility_mode: bool,
    pub post_item_count: u16,
    pub unavailable_count: u16,
    pub inventory_slots: Vec<TikTokInventorySlot>,
}
#[derive(Clone)]
struct Session {
    preview: TikTokAnalysisPreview,
    inventory: Arc<TikTokInventory>,
    network: Arc<NetworkProfile>,
    created: Instant,
    cancellation: CancellationToken,
}
struct State {
    closed: bool,
    generation: u64,
    cancellation: CancellationToken,
    session: Option<Session>,
}
pub struct TikTokAnalysisService<I, V, P> {
    inventory: I,
    video: V,
    preview: P,
    current: Mutex<State>,
}
impl<I: TikTokInventoryExtractor, V: TikTokVideoAnalyzer, P: TikTokPreviewLoader>
    TikTokAnalysisService<I, V, P>
{
    pub fn new(inventory: I, video: V, preview: P) -> Self {
        Self {
            inventory,
            video,
            preview,
            current: Mutex::new(State {
                closed: false,
                generation: 0,
                cancellation: CancellationToken::new(),
                session: None,
            }),
        }
    }
    pub fn analyze(
        &self,
        url: &str,
        now: Instant,
    ) -> Result<TikTokAnalysisPreview, AnalysisErrorCode> {
        let (generation, cancellation) = {
            let mut current = self
                .current
                .lock()
                .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
            if current.closed {
                return Err(AnalysisErrorCode::AnalysisUnavailable);
            }
            current.cancellation.cancel();
            current.generation = current
                .generation
                .checked_add(1)
                .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
            current.cancellation = CancellationToken::new();
            current.session = None;
            (current.generation, current.cancellation.clone())
        };
        let network = Arc::new(NetworkProfile::current());
        let context = TikTokRequestContext::new(
            cancellation.clone(),
            Instant::now() + Duration::from_secs(120),
            network.clone(),
        );
        let source = self.inventory.resolve_url(url, &context)?;
        if source.platform() != Platform::Tiktok {
            return Err(AnalysisErrorCode::UnsupportedPlatform);
        }
        let inventory = self.inventory.extract_post(&source, &context)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        if inventory.source.media_id() != source.media_id()
            || inventory.source.platform() != Platform::Tiktok
        {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        let mut items = Vec::new();
        for item in &inventory.items {
            if cancellation.is_canceled() {
                return Err(AnalysisErrorCode::AnalysisUnavailable);
            }
            let mut public = TikTokMediaItem {
                item_id: next_id()?,
                ordinal: item.ordinal,
                kind: item.kind,
                width: item.width,
                height: item.height,
                container: if item.kind == TikTokMediaKind::Image {
                    item.image_candidates
                        .first()
                        .and_then(|c| {
                            c.url
                                .query_pairs()
                                .find(|(k, _)| k == "format")
                                .map(|(_, v)| v.into_owned())
                        })
                        .unwrap_or_else(|| "unknown".into())
                } else {
                    "unknown".into()
                },
                file_size: None,
                fps: None,
                video_bitrate_kbps: None,
                audio_bitrate_kbps: None,
                video_codec: None,
                audio_codec: None,
                dynamic_range: None,
                preview_available: item.preview.is_some(),
                availability: item.availability,
                unavailable_reason: item.unavailable_reason,
            };
            if item.kind != TikTokMediaKind::Image
                && item.availability == TikTokItemAvailability::Ready
            {
                let result = item
                    .identity
                    .as_ref()
                    .ok_or(AnalysisErrorCode::InvalidMetadata)
                    .and_then(|id| self.video.analyze_item(&inventory, id, false, &context));
                match result {
                    Ok(video) => {
                        let plan = video.plan();
                        let v = video
                            .media()
                            .formats
                            .iter()
                            .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
                            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
                        let a = video
                            .media()
                            .formats
                            .iter()
                            .find(|f| Some(&f.id) == plan.audio_format_id.as_ref());
                        public.width = v.width;
                        public.height = v.height;
                        public.fps = v.fps;
                        public.video_codec = v.video_codec.clone();
                        public.audio_codec = a.and_then(|a| a.audio_codec.clone());
                        public.video_bitrate_kbps = v.video_bitrate_kbps;
                        public.audio_bitrate_kbps = a.and_then(|a| a.audio_bitrate_kbps);
                        public.dynamic_range = v.dynamic_range.clone();
                        public.container = plan
                            .output_container
                            .clone()
                            .unwrap_or_else(|| v.container.clone());
                        public.file_size =
                            plan.estimated_size_bytes
                                .filter(|n| *n > 0)
                                .map(|bytes| FileSize {
                                    bytes,
                                    is_approximate: true,
                                });
                    }
                    Err(error) => {
                        public.availability = TikTokItemAvailability::Unavailable;
                        public.unavailable_reason = Some(error);
                    }
                }
            }
            items.push(public);
        }
        let result = TikTokAnalysisPreview {
            analysis_id: next_id()?,
            post_id: source.media_id().into(),
            title: inventory.title.clone(),
            creator: inventory.creator.clone(),
            source_page_url: inventory.source.url().into(),
            capabilities: TikTokCapabilities::PUBLIC_MEDIA,
            items,
        };
        result.validate()?;
        let mut current = self
            .current
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        if current.closed || current.generation != generation || cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        current.session = Some(Session {
            preview: result.clone(),
            inventory: Arc::new(inventory),
            network,
            created: now,
            cancellation,
        });
        Ok(result)
    }
    fn session(&self, id: &str, now: Instant) -> Result<Session, AnalysisErrorCode> {
        let mut current = self
            .current
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let session = current
            .session
            .as_ref()
            .filter(|s| s.preview.analysis_id == id && !s.cancellation.is_canceled())
            .ok_or(AnalysisErrorCode::AnalysisExpired)?;
        if now.saturating_duration_since(session.created) > Duration::from_secs(900) {
            // Submitted jobs own independent snapshots; releasing this expired
            // UI session must neither retain its pool nor cancel those jobs.
            current.session = None;
            return Err(AnalysisErrorCode::AnalysisExpired);
        }
        Ok(session.clone())
    }
    pub fn load_preview(
        &self,
        id: &str,
        item_id: &str,
        now: Instant,
    ) -> Result<TikTokPreviewPayload, AnalysisErrorCode> {
        let session = self.session(id, now)?;
        let index = session
            .preview
            .items
            .iter()
            .position(|i| i.item_id == item_id && i.preview_available)
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
        let context = TikTokRequestContext::new(
            session.cancellation.clone(),
            Instant::now() + Duration::from_secs(15),
            session.network.clone(),
        );
        let payload = self
            .preview
            .load(&session.inventory.items[index], &context)?
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
        self.session(id, now)?;
        serde_json::from_value(
            serde_json::to_value(payload).map_err(|_| AnalysisErrorCode::InvalidMetadata)?,
        )
        .map_err(|_| AnalysisErrorCode::InvalidMetadata)
    }
    pub fn resolve_source(
        &self,
        id: &str,
        now: Instant,
    ) -> Result<ValidatedMediaUrl, AnalysisErrorCode> {
        Ok(self.session(id, now)?.inventory.source.clone())
    }
    pub fn resolve_download(
        &self,
        id: &str,
        intent: TikTokDownloadIntent,
        now: Instant,
    ) -> Result<ResolvedTikTokDownload, DownloadErrorCode> {
        intent.validate()?;
        let session = self.session(id, now).map_err(map_error)?;
        let selected = intent.item_ids.into_iter().collect::<HashSet<_>>();
        let mut items = Vec::new();
        for (index, public) in session.preview.items.iter().enumerate() {
            if selected.contains(&public.item_id) {
                if public.availability != TikTokItemAvailability::Ready {
                    return Err(public
                        .unavailable_reason
                        .map(map_error)
                        .unwrap_or(DownloadErrorCode::FormatUnavailable));
                }
                items.push(ResolvedTikTokDownloadItem {
                    public: public.clone(),
                    identity: session.inventory.items[index]
                        .identity
                        .clone()
                        .ok_or(DownloadErrorCode::AnalysisUnavailable)?,
                });
            }
        }
        if items.len() != selected.len() {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(ResolvedTikTokDownload {
            source: session.inventory.source.clone(),
            title: session.preview.title.clone(),
            creator: session.preview.creator.clone(),
            items,
            network: session.network.clone(),
            compatibility_mode: intent.compatibility_mode,
            post_item_count: session.preview.items.len() as u16,
            unavailable_count: session
                .preview
                .items
                .iter()
                .filter(|i| i.availability == TikTokItemAvailability::Unavailable)
                .count() as u16,
            inventory_slots: session
                .inventory
                .items
                .iter()
                .map(|item| TikTokInventorySlot {
                    ordinal: item.ordinal,
                    identity: item.identity.clone(),
                    kind: item.kind,
                })
                .collect(),
        })
    }
    pub fn clear(&self) {
        if let Ok(mut state) = self.current.lock() {
            state.cancellation.cancel();
            state.session = None;
            state.generation = state.generation.saturating_add(1)
        }
    }
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.current.lock() {
            state.closed = true;
            state.cancellation.cancel();
            state.session = None;
        }
    }
}
fn next_id() -> Result<String, AnalysisErrorCode> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map(|n| n.to_string())
        .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)
}
pub(crate) fn map_error(error: AnalysisErrorCode) -> DownloadErrorCode {
    match error {
        AnalysisErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        AnalysisErrorCode::ToolIntegrityFailed => DownloadErrorCode::ToolIntegrityFailed,
        AnalysisErrorCode::ContentChanged => DownloadErrorCode::ContentChanged,
        AnalysisErrorCode::AnalysisExpired => DownloadErrorCode::AnalysisUnavailable,
        AnalysisErrorCode::FormatUnavailable => DownloadErrorCode::FormatUnavailable,
        AnalysisErrorCode::AuthenticationRequired => DownloadErrorCode::AuthenticationRequired,
        AnalysisErrorCode::AccessDenied => DownloadErrorCode::AccessDenied,
        AnalysisErrorCode::RateLimited => DownloadErrorCode::RateLimited,
        AnalysisErrorCode::ExtractorOutdated => DownloadErrorCode::ExtractorOutdated,
        AnalysisErrorCode::NetworkFailed => DownloadErrorCode::NetworkFailed,
        AnalysisErrorCode::AnalysisTimeout => DownloadErrorCode::DownloadTimeout,
        AnalysisErrorCode::ProcessFailed => DownloadErrorCode::ProcessFailed,
        AnalysisErrorCode::ContentUnavailable => DownloadErrorCode::ContentUnavailable,
        _ => DownloadErrorCode::AnalysisUnavailable,
    }
}
