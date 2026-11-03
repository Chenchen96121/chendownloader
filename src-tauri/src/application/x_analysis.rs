use super::{
    media_url::{validate_media_url, ValidatedMediaUrl},
    network_profile::NetworkProfile,
    process::CancellationToken,
    x_source::{
        XAttachmentIdentity, XInventory, XInventoryExtractor, XRequestContext, XSourceItem,
    },
    x_video::XVideoAnalyzer,
};
use crate::domain::{
    download::DownloadErrorCode,
    media::{AnalysisErrorCode, FileSize, Platform},
    x::{
        XAnalysisPreview, XCapabilities, XDownloadIntent, XItemAvailability, XMediaItem,
        XMediaKind, XPreviewPayload,
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
pub trait XPreviewLoader: Send + Sync {
    fn load(
        &self,
        item: &XSourceItem,
        context: &XRequestContext,
    ) -> Result<Option<XPreviewPayload>, AnalysisErrorCode>;
}
#[derive(Clone)]
pub struct ResolvedXDownloadItem {
    pub public: XMediaItem,
    pub identity: XAttachmentIdentity,
}
#[derive(Clone, PartialEq, Eq)]
pub struct XInventorySlot {
    pub ordinal: u16,
    pub identity: Option<XAttachmentIdentity>,
    pub kind: XMediaKind,
}
#[derive(Clone)]
pub struct ResolvedXDownload {
    pub source: ValidatedMediaUrl,
    pub title: String,
    pub creator: Option<String>,
    pub items: Vec<ResolvedXDownloadItem>,
    pub network: Arc<NetworkProfile>,
    pub compatibility_mode: bool,
    pub post_item_count: u16,
    pub unavailable_count: u16,
    pub inventory_slots: Vec<XInventorySlot>,
}
#[derive(Clone)]
struct Session {
    preview: XAnalysisPreview,
    inventory: Arc<XInventory>,
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
pub struct XAnalysisService<I, V, P> {
    inventory: I,
    video: V,
    preview: P,
    current: Mutex<State>,
}
impl<I: XInventoryExtractor, V: XVideoAnalyzer, P: XPreviewLoader> XAnalysisService<I, V, P> {
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
    pub fn analyze(&self, url: &str, now: Instant) -> Result<XAnalysisPreview, AnalysisErrorCode> {
        let source = validate_media_url(url)?;
        if source.platform() != Platform::X {
            return Err(AnalysisErrorCode::UnsupportedPlatform);
        }
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
        let context = XRequestContext::new(
            cancellation.clone(),
            Instant::now() + Duration::from_secs(120),
            network.clone(),
        );
        let inventory = self.inventory.extract_post(&source, &context)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        if inventory.source.media_id() != source.media_id()
            || inventory.source.platform() != Platform::X
        {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        let mut items = Vec::new();
        for item in &inventory.items {
            if cancellation.is_canceled() {
                return Err(AnalysisErrorCode::AnalysisUnavailable);
            }
            let mut public = XMediaItem {
                item_id: next_id()?,
                ordinal: item.ordinal,
                kind: item.kind,
                width: item.width,
                height: item.height,
                container: if item.kind == XMediaKind::Image {
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
            if item.kind != XMediaKind::Image && item.availability == XItemAvailability::Ready {
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
                        public.availability = XItemAvailability::Unavailable;
                        public.unavailable_reason = Some(error);
                    }
                }
            }
            items.push(public);
        }
        let result = XAnalysisPreview {
            analysis_id: next_id()?,
            post_id: source.media_id().into(),
            title: inventory.title.clone(),
            creator: inventory.creator.clone(),
            source_page_url: source.url().into(),
            capabilities: XCapabilities::PUBLIC_MEDIA,
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
    ) -> Result<XPreviewPayload, AnalysisErrorCode> {
        let session = self.session(id, now)?;
        let index = session
            .preview
            .items
            .iter()
            .position(|i| i.item_id == item_id && i.preview_available)
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
        let context = XRequestContext::new(
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
        intent: XDownloadIntent,
        now: Instant,
    ) -> Result<ResolvedXDownload, DownloadErrorCode> {
        intent.validate()?;
        let session = self.session(id, now).map_err(map_error)?;
        let selected = intent.item_ids.into_iter().collect::<HashSet<_>>();
        let mut items = Vec::new();
        for (index, public) in session.preview.items.iter().enumerate() {
            if selected.contains(&public.item_id) {
                if public.availability != XItemAvailability::Ready {
                    return Err(public
                        .unavailable_reason
                        .map(map_error)
                        .unwrap_or(DownloadErrorCode::FormatUnavailable));
                }
                items.push(ResolvedXDownloadItem {
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
        Ok(ResolvedXDownload {
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
                .filter(|i| i.availability == XItemAvailability::Unavailable)
                .count() as u16,
            inventory_slots: session
                .inventory
                .items
                .iter()
                .map(|item| XInventorySlot {
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
