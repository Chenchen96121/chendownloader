use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    application::{
        instagram_video::{InstagramVideoSelector, ResolvedInstagramVideo},
        media_url::{validate_media_url, ValidatedMediaUrl},
        process::{CancellationToken, ProcessSupervisor},
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        download::DownloadErrorCode,
        instagram::{
            InstagramAnalysisPreview, InstagramCapabilities, InstagramDownloadIntent,
            InstagramItemAvailability, InstagramMediaItem, InstagramMediaKind,
            InstagramPreviewPayload, InstagramUrlKind,
        },
        media::{AnalysisErrorCode, FileSize, MediaFormat, Platform, StreamKind},
    },
    infrastructure::{
        gallery_dl::{
            InstagramAssetAcquirer, InstagramInventory, InstagramInventoryExtractor,
            ResolvedInstagramItem,
        },
        ytdlp::YtDlpInstagramVideoAnalyzer,
    },
};

const SESSION_LIFETIME: Duration = Duration::from_secs(15 * 60);
const ANALYSIS_TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;

pub trait InstagramVideoAnalyzer: Send + Sync {
    fn analyze_reel(
        &self,
        source: &ValidatedMediaUrl,
        compatibility_mode: bool,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode>;

    fn analyze_post_item(
        &self,
        source: &ValidatedMediaUrl,
        ordinal: u16,
        expected_media_id: &str,
        compatibility_mode: bool,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode>;
}

impl<R, P> InstagramVideoAnalyzer for YtDlpInstagramVideoAnalyzer<R, P>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    fn analyze_reel(
        &self,
        source: &ValidatedMediaUrl,
        compatibility_mode: bool,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        YtDlpInstagramVideoAnalyzer::analyze_reel(self, source, compatibility_mode, cancellation)
    }

    fn analyze_post_item(
        &self,
        source: &ValidatedMediaUrl,
        ordinal: u16,
        expected_media_id: &str,
        compatibility_mode: bool,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        YtDlpInstagramVideoAnalyzer::analyze_post_item(
            self,
            source,
            ordinal,
            expected_media_id,
            compatibility_mode,
            cancellation,
        )
    }
}

pub struct InstagramPreviewSource {
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

pub trait InstagramPreviewLoader: Send + Sync {
    fn load_preview(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<InstagramPreviewSource>, AnalysisErrorCode>;
}

impl<T> InstagramPreviewLoader for T
where
    T: InstagramAssetAcquirer + Send + Sync,
{
    fn load_preview(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<InstagramPreviewSource>, AnalysisErrorCode> {
        let Some(verified) =
            InstagramAssetAcquirer::fetch_preview(self, item, protected_directory, cancellation)?
        else {
            return Ok(None);
        };
        let mime_type = verified
            .mime_type()
            .ok_or(AnalysisErrorCode::FormatUnavailable)?
            .to_owned();
        let bytes = fs::read(verified.path()).map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        Ok(Some(InstagramPreviewSource { mime_type, bytes }))
    }
}

#[derive(Clone)]
pub enum ResolvedInstagramDownloadKind {
    Image(Box<ResolvedInstagramItem>),
    Video(InstagramVideoSelector),
}

#[derive(Clone)]
pub struct ResolvedInstagramDownloadItem {
    public: InstagramMediaItem,
    kind: ResolvedInstagramDownloadKind,
}

impl ResolvedInstagramDownloadItem {
    pub fn public_item(&self) -> &InstagramMediaItem {
        &self.public
    }

    pub const fn ordinal(&self) -> u16 {
        self.public.ordinal
    }

    pub const fn kind(&self) -> &ResolvedInstagramDownloadKind {
        &self.kind
    }
}

pub struct ResolvedInstagramDownload {
    source: ValidatedMediaUrl,
    title: String,
    creator: Option<String>,
    compatibility_mode: bool,
    items: Vec<ResolvedInstagramDownloadItem>,
}

impl ResolvedInstagramDownload {
    pub fn source(&self) -> &ValidatedMediaUrl {
        &self.source
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn creator(&self) -> Option<&str> {
        self.creator.as_deref()
    }

    pub const fn compatibility_mode(&self) -> bool {
        self.compatibility_mode
    }

    pub fn items(&self) -> &[ResolvedInstagramDownloadItem] {
        &self.items
    }
}

struct SessionItem {
    public: InstagramMediaItem,
    inventory_index: Option<usize>,
    video_selector: Option<InstagramVideoSelector>,
}

struct AnalysisSession {
    analysis_id: String,
    source: ValidatedMediaUrl,
    title: String,
    creator: Option<String>,
    created_at: Instant,
    inventory: Option<InstagramInventory>,
    items: Vec<SessionItem>,
    previews: Mutex<HashMap<String, InstagramPreviewPayload>>,
    _cache: tempfile::TempDir,
}

impl AnalysisSession {
    fn preview(&self) -> Result<InstagramAnalysisPreview, AnalysisErrorCode> {
        let value = InstagramAnalysisPreview {
            analysis_id: self.analysis_id.clone(),
            source_kind: self
                .source
                .instagram_kind()
                .ok_or(AnalysisErrorCode::InvalidMetadata)?,
            shortcode: self.source.media_id().to_owned(),
            title: self.title.clone(),
            creator: self.creator.clone(),
            source_page_url: self.source.url().to_owned(),
            capabilities: InstagramCapabilities::PUBLIC_MEDIA,
            items: self.items.iter().map(|item| item.public.clone()).collect(),
        };
        value.validate()?;
        Ok(value)
    }

    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created_at) >= SESSION_LIFETIME
    }
}

struct CurrentAnalysis {
    generation: u64,
    next_id: u64,
    cancellation: CancellationToken,
    session: Option<std::sync::Arc<AnalysisSession>>,
}

impl Default for CurrentAnalysis {
    fn default() -> Self {
        Self {
            generation: 0,
            next_id: 1,
            cancellation: CancellationToken::new(),
            session: None,
        }
    }
}

pub struct InstagramAnalysisService<I, V, P> {
    inventory: I,
    video: V,
    preview: P,
    cache_root: Option<PathBuf>,
    current: Mutex<CurrentAnalysis>,
}

impl<I, V, P> InstagramAnalysisService<I, V, P> {
    pub fn new(inventory: I, video: V, preview: P) -> Self {
        Self {
            inventory,
            video,
            preview,
            cache_root: None,
            current: Mutex::new(CurrentAnalysis::default()),
        }
    }

    pub fn new_in(inventory: I, video: V, preview: P, cache_root: PathBuf) -> Self {
        Self {
            inventory,
            video,
            preview,
            cache_root: Some(cache_root),
            current: Mutex::new(CurrentAnalysis::default()),
        }
    }

    #[doc(hidden)]
    pub fn preview_loader(&self) -> &P {
        &self.preview
    }

    pub fn clear(&self) {
        let previous = self.current.lock().ok().and_then(|mut current| {
            current.cancellation.cancel();
            current.generation = current.generation.saturating_add(1);
            current.session.take()
        });
        drop(previous);
    }
}

impl<I, V, P> InstagramAnalysisService<I, V, P>
where
    I: InstagramInventoryExtractor + Send + Sync,
    V: InstagramVideoAnalyzer,
    P: InstagramPreviewLoader,
{
    pub fn analyze(
        &self,
        url: &str,
        now: Instant,
    ) -> Result<InstagramAnalysisPreview, AnalysisErrorCode> {
        let source = validate_media_url(url)?;
        if source.platform() != Platform::Instagram {
            return Err(AnalysisErrorCode::UnsupportedPlatform);
        }
        let (generation, analysis_id, cancellation, previous) = {
            let mut current = self
                .current
                .lock()
                .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
            current.cancellation.cancel();
            current.generation = current
                .generation
                .checked_add(1)
                .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
            let generation = current.generation;
            let analysis_id = current.next_id.to_string();
            current.next_id = current
                .next_id
                .checked_add(1)
                .filter(|value| *value <= 9_007_199_254_740_991)
                .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
            current.cancellation = CancellationToken::new();
            let cancellation = current.cancellation.clone();
            let previous = current.session.take();
            (generation, analysis_id, cancellation, previous)
        };
        drop(previous);

        let deadline = AnalysisDeadline::new(cancellation.clone(), ANALYSIS_TOTAL_TIMEOUT);
        let cache = self.create_cache()?;
        let result = match source.instagram_kind() {
            Some(InstagramUrlKind::Post) => {
                self.analyze_post(analysis_id, source, now, cache, &cancellation)
            }
            Some(InstagramUrlKind::Reel) => {
                self.analyze_reel(analysis_id, source, now, cache, &cancellation)
            }
            None => Err(AnalysisErrorCode::UnsupportedMedia),
        };
        if deadline.finish() {
            return Err(AnalysisErrorCode::AnalysisTimeout);
        }
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisExpired);
        }
        let session = result?;
        let session = std::sync::Arc::new(session);
        let preview = session.preview()?;
        let mut current = self
            .current
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        if current.generation != generation || current.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisExpired);
        }
        current.session = Some(session);
        Ok(preview)
    }

    pub fn load_preview(
        &self,
        analysis_id: &str,
        item_id: &str,
        now: Instant,
    ) -> Result<InstagramPreviewPayload, AnalysisErrorCode> {
        let (generation, cancellation, session) = self.current_session(analysis_id, now)?;
        if let Some(cached) = session
            .previews
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?
            .get(item_id)
            .cloned()
        {
            return Ok(cached);
        }
        let entry = session
            .items
            .iter()
            .find(|item| item.public.item_id == item_id)
            .ok_or(AnalysisErrorCode::UnsupportedMedia)?;
        if !entry.public.preview_available {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        let index = entry
            .inventory_index
            .ok_or(AnalysisErrorCode::FormatUnavailable)?;
        let resolved = session
            .inventory
            .as_ref()
            .and_then(|inventory| inventory.items().get(index))
            .ok_or(AnalysisErrorCode::InvalidMetadata)?
            .clone();
        let directory = session._cache.path().join(format!("preview-{item_id}"));
        fs::create_dir(&directory).map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let loaded = self
            .preview
            .load_preview(&resolved, &directory, &cancellation);
        if let Err(error) = self.assert_current(generation, analysis_id, now) {
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
        let source = match loaded {
            Ok(Some(source)) => source,
            Ok(None) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(AnalysisErrorCode::FormatUnavailable);
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(error);
            }
        };
        if source.bytes.is_empty()
            || source.bytes.len() > MAX_PREVIEW_BYTES
            || !matches!(
                source.mime_type.as_str(),
                "image/jpeg" | "image/png" | "image/webp" | "image/avif"
            )
        {
            let _ = fs::remove_dir_all(&directory);
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        let payload = InstagramPreviewPayload {
            mime_type: source.mime_type,
            data_base64: encode_base64(&source.bytes),
        };
        session
            .previews
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?
            .insert(item_id.to_owned(), payload.clone());
        Ok(payload)
    }

    pub fn resolve_source(
        &self,
        analysis_id: &str,
        now: Instant,
    ) -> Result<ValidatedMediaUrl, AnalysisErrorCode> {
        let (_, _, session) = self.current_session(analysis_id, now)?;
        Ok(session.source.clone())
    }

    pub fn resolve_download(
        &self,
        analysis_id: &str,
        intent: InstagramDownloadIntent,
        now: Instant,
    ) -> Result<ResolvedInstagramDownload, DownloadErrorCode> {
        intent.validate()?;
        let (_, _, session) = self
            .current_session(analysis_id, now)
            .map_err(map_session_download_error)?;
        let requested = intent.item_ids.into_iter().collect::<HashSet<_>>();
        let mut items = Vec::with_capacity(requested.len());
        for entry in &session.items {
            if !requested.contains(&entry.public.item_id) {
                continue;
            }
            if entry.public.availability != InstagramItemAvailability::Ready {
                return Err(entry
                    .public
                    .unavailable_reason
                    .map(map_analysis_download_error)
                    .unwrap_or(DownloadErrorCode::FormatUnavailable));
            }
            let kind = match entry.public.kind {
                InstagramMediaKind::Image => {
                    let index = entry
                        .inventory_index
                        .ok_or(DownloadErrorCode::AnalysisUnavailable)?;
                    let resolved = session
                        .inventory
                        .as_ref()
                        .and_then(|inventory| inventory.items().get(index))
                        .ok_or(DownloadErrorCode::AnalysisUnavailable)?
                        .clone();
                    ResolvedInstagramDownloadKind::Image(Box::new(resolved))
                }
                InstagramMediaKind::Video => ResolvedInstagramDownloadKind::Video(
                    entry
                        .video_selector
                        .clone()
                        .ok_or(DownloadErrorCode::AnalysisUnavailable)?,
                ),
            };
            items.push(ResolvedInstagramDownloadItem {
                public: entry.public.clone(),
                kind,
            });
        }
        if items.len() != requested.len() {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(ResolvedInstagramDownload {
            source: session.source.clone(),
            title: session.title.clone(),
            creator: session.creator.clone(),
            compatibility_mode: intent.compatibility_mode,
            items,
        })
    }

    fn analyze_post(
        &self,
        analysis_id: String,
        source: ValidatedMediaUrl,
        now: Instant,
        cache: tempfile::TempDir,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisSession, AnalysisErrorCode> {
        let inventory = self.inventory.extract_post(&source, cancellation)?;
        if inventory.shortcode() != source.media_id() {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        let mut items = Vec::with_capacity(inventory.items().len());
        for (index, resolved) in inventory.items().iter().enumerate() {
            let mut public = resolved.public_item().clone();
            let mut selector = None;
            if public.kind == InstagramMediaKind::Video {
                let expected = resolved.media_id();
                let candidate = InstagramVideoSelector::post_item(public.ordinal, expected)?;
                selector = Some(candidate);
                let result = self.video.analyze_post_item(
                    &source,
                    public.ordinal,
                    expected,
                    false,
                    cancellation,
                );
                match result {
                    Ok(video) => project_video(&mut public, &video)?,
                    Err(error) => {
                        public.availability = InstagramItemAvailability::Unavailable;
                        public.unavailable_reason = Some(error);
                    }
                }
            }
            items.push(SessionItem {
                public,
                inventory_index: Some(index),
                video_selector: selector,
            });
        }
        Ok(AnalysisSession {
            analysis_id,
            source,
            title: inventory.title().to_owned(),
            creator: inventory.creator().map(str::to_owned),
            created_at: now,
            inventory: Some(inventory),
            items,
            previews: Mutex::new(HashMap::new()),
            _cache: cache,
        })
    }

    fn analyze_reel(
        &self,
        analysis_id: String,
        source: ValidatedMediaUrl,
        now: Instant,
        cache: tempfile::TempDir,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisSession, AnalysisErrorCode> {
        let video = self.video.analyze_reel(&source, false, cancellation)?;
        let mut public = InstagramMediaItem {
            item_id: analysis_id.clone(),
            ordinal: 1,
            kind: InstagramMediaKind::Video,
            width: None,
            height: None,
            container: "mp4".to_owned(),
            file_size: None,
            fps: None,
            video_bitrate_kbps: None,
            audio_bitrate_kbps: None,
            video_codec: None,
            audio_codec: None,
            dynamic_range: None,
            preview_available: false,
            availability: InstagramItemAvailability::Ready,
            unavailable_reason: None,
        };
        project_video(&mut public, &video)?;
        public.validate()?;
        let title = video.media().title.clone();
        let creator = video.media().creator.clone();
        Ok(AnalysisSession {
            analysis_id,
            source,
            title,
            creator,
            created_at: now,
            inventory: None,
            items: vec![SessionItem {
                public,
                inventory_index: None,
                video_selector: Some(InstagramVideoSelector::reel()),
            }],
            previews: Mutex::new(HashMap::new()),
            _cache: cache,
        })
    }

    fn create_cache(&self) -> Result<tempfile::TempDir, AnalysisErrorCode> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("lmd-instagram-analysis-");
        if let Some(root) = &self.cache_root {
            fs::create_dir_all(root).map_err(|_| AnalysisErrorCode::ProcessFailed)?;
            builder
                .tempdir_in(root)
                .map_err(|_| AnalysisErrorCode::ProcessFailed)
        } else {
            builder
                .tempdir()
                .map_err(|_| AnalysisErrorCode::ProcessFailed)
        }
    }

    fn current_session(
        &self,
        analysis_id: &str,
        now: Instant,
    ) -> Result<(u64, CancellationToken, std::sync::Arc<AnalysisSession>), AnalysisErrorCode> {
        if !crate::domain::download::valid_opaque_id(analysis_id) {
            return Err(AnalysisErrorCode::AnalysisExpired);
        }
        let mut current = self
            .current
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let session = current
            .session
            .as_ref()
            .filter(|session| session.analysis_id == analysis_id)
            .cloned()
            .ok_or(AnalysisErrorCode::AnalysisExpired)?;
        if session.expired(now) {
            current.cancellation.cancel();
            current.generation = current.generation.saturating_add(1);
            current.session = None;
            return Err(AnalysisErrorCode::AnalysisExpired);
        }
        Ok((current.generation, current.cancellation.clone(), session))
    }

    fn assert_current(
        &self,
        generation: u64,
        analysis_id: &str,
        now: Instant,
    ) -> Result<(), AnalysisErrorCode> {
        let current = self
            .current
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let valid = current.generation == generation
            && !current.cancellation.is_canceled()
            && current
                .session
                .as_ref()
                .is_some_and(|session| session.analysis_id == analysis_id && !session.expired(now));
        if valid {
            Ok(())
        } else {
            Err(AnalysisErrorCode::AnalysisExpired)
        }
    }
}

struct AnalysisDeadline {
    stop: Option<mpsc::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
    timed_out: Arc<AtomicBool>,
}

impl AnalysisDeadline {
    fn new(cancellation: CancellationToken, timeout: Duration) -> Self {
        let (stop, receiver) = mpsc::channel();
        let timed_out = Arc::new(AtomicBool::new(false));
        let worker_timed_out = Arc::clone(&timed_out);
        let worker = thread::spawn(move || {
            if matches!(
                receiver.recv_timeout(timeout),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                worker_timed_out.store(true, Ordering::Release);
                cancellation.cancel();
            }
        });
        Self {
            stop: Some(stop),
            worker: Some(worker),
            timed_out,
        }
    }

    fn finish(mut self) -> bool {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.timed_out.load(Ordering::Acquire)
    }
}

impl Drop for AnalysisDeadline {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl<I, V, P> Drop for InstagramAnalysisService<I, V, P> {
    fn drop(&mut self) {
        if let Ok(mut current) = self.current.lock() {
            current.cancellation.cancel();
            current.session = None;
        }
    }
}

fn project_video(
    public: &mut InstagramMediaItem,
    video: &ResolvedInstagramVideo,
) -> Result<(), AnalysisErrorCode> {
    let plan = video.plan();
    let media = video.media();
    let video_format = selected_format(media.formats.as_slice(), plan.video_format_id.as_deref())
        .or_else(|| {
            plan.source_format_ids.iter().find_map(|id| {
                media
                    .formats
                    .iter()
                    .find(|format| &format.id == id && format.kind != StreamKind::AudioOnly)
            })
        })
        .ok_or(AnalysisErrorCode::FormatUnavailable)?;
    let audio_format = selected_format(media.formats.as_slice(), plan.audio_format_id.as_deref())
        .or_else(|| (video_format.kind == StreamKind::Combined).then_some(video_format));
    public.width = video_format.width;
    public.height = video_format.height;
    public.fps = video_format.fps;
    public.video_bitrate_kbps = video_format.video_bitrate_kbps;
    public.audio_bitrate_kbps = audio_format.and_then(|format| format.audio_bitrate_kbps);
    public.video_codec = video_format.video_codec.clone();
    public.audio_codec = audio_format.and_then(|format| format.audio_codec.clone());
    public.dynamic_range = video_format.dynamic_range.clone();
    public.container = plan
        .output_container
        .clone()
        .unwrap_or_else(|| video_format.container.clone());
    public.file_size = plan.estimated_size_bytes.map(|bytes| FileSize {
        bytes,
        is_approximate: true,
    });
    public.availability = InstagramItemAvailability::Ready;
    public.unavailable_reason = None;
    public.validate()
}

fn selected_format<'a>(formats: &'a [MediaFormat], id: Option<&str>) -> Option<&'a MediaFormat> {
    formats.iter().find(|format| Some(format.id.as_str()) == id)
}

fn map_session_download_error(error: AnalysisErrorCode) -> DownloadErrorCode {
    match error {
        AnalysisErrorCode::AnalysisExpired | AnalysisErrorCode::AnalysisUnavailable => {
            DownloadErrorCode::AnalysisUnavailable
        }
        other => map_analysis_download_error(other),
    }
}

pub(crate) fn map_analysis_download_error(error: AnalysisErrorCode) -> DownloadErrorCode {
    match error {
        AnalysisErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        AnalysisErrorCode::ToolIntegrityFailed => DownloadErrorCode::ToolIntegrityFailed,
        AnalysisErrorCode::AuthenticationRequired => DownloadErrorCode::AuthenticationRequired,
        AnalysisErrorCode::AccessDenied => DownloadErrorCode::AccessDenied,
        AnalysisErrorCode::ContentUnavailable => DownloadErrorCode::ContentUnavailable,
        AnalysisErrorCode::ContentChanged => DownloadErrorCode::ContentChanged,
        AnalysisErrorCode::RateLimited => DownloadErrorCode::RateLimited,
        AnalysisErrorCode::ExtractorOutdated => DownloadErrorCode::ExtractorOutdated,
        AnalysisErrorCode::NetworkFailed => DownloadErrorCode::NetworkFailed,
        AnalysisErrorCode::AnalysisTimeout => DownloadErrorCode::DownloadTimeout,
        AnalysisErrorCode::ProcessFailed => DownloadErrorCode::ProcessFailed,
        AnalysisErrorCode::FormatUnavailable => DownloadErrorCode::FormatUnavailable,
        AnalysisErrorCode::AnalysisExpired | AnalysisErrorCode::AnalysisUnavailable => {
            DownloadErrorCode::AnalysisUnavailable
        }
        _ => DownloadErrorCode::Unknown,
    }
}

use super::preview_encoding::encode_base64;
