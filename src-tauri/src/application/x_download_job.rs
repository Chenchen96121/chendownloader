use super::{
    output_verifier::VerifiedOutput,
    x_source::{XRequestContext, XSourceItem},
};
use crate::{
    domain::download::DownloadErrorCode, infrastructure::image_verification::VerifiedImage,
};
use std::path::Path;

#[derive(Clone)]
pub struct XPublicationRequest {
    job_id: String,
    post_id: String,
    title: String,
    creator: Option<String>,
    items: Vec<crate::domain::x::XMediaItem>,
}
impl XPublicationRequest {
    pub fn new(
        job_id: String,
        post_id: String,
        title: String,
        creator: Option<String>,
        items: Vec<crate::domain::x::XMediaItem>,
    ) -> Result<Self, DownloadErrorCode> {
        let valid_text = |s: &str, max: usize| {
            !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
        };
        let mut ids = std::collections::HashSet::new();
        if !crate::domain::download::valid_opaque_id(&job_id)
            || !crate::domain::download::valid_opaque_id(&post_id)
            || !valid_text(&title, 512)
            || creator.as_deref().is_some_and(|s| !valid_text(s, 256))
            || items.is_empty()
            || items.len() > 32
            || items.iter().any(|i| {
                i.validate().is_err()
                    || i.availability != crate::domain::x::XItemAvailability::Ready
                    || !ids.insert(&i.item_id)
            })
            || items.windows(2).any(|p| p[0].ordinal >= p[1].ordinal)
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(Self {
            job_id,
            post_id,
            title,
            creator,
            items,
        })
    }
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
    pub fn post_id(&self) -> &str {
        &self.post_id
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn creator(&self) -> Option<&str> {
        self.creator.as_deref()
    }
    pub fn items(&self) -> &[crate::domain::x::XMediaItem] {
        &self.items
    }
}
pub struct XPublication {
    pub(crate) destination: std::path::PathBuf,
    pub(crate) directory: std::path::PathBuf,
    pub(crate) display_name: String,
    pub(crate) request: XPublicationRequest,
}
impl XPublication {
    pub fn output_directory(&self) -> &Path {
        &self.directory
    }
    pub fn open_path(&self) -> &Path {
        &self.directory
    }
    pub fn folder_display_name(&self) -> &str {
        &self.display_name
    }
}
pub trait XOutputPublisher: Send + Sync {
    fn prepare(
        &self,
        destination: &Path,
        request: &XPublicationRequest,
    ) -> Result<XPublication, DownloadErrorCode>;
    fn publish(
        &self,
        asset: XVerifiedAsset,
        publication: &XPublication,
        item: &crate::domain::x::XMediaItem,
        cancellation: &crate::application::process::CancellationToken,
    ) -> Result<String, DownloadErrorCode>;
    fn cleanup_empty(&self, publication: &XPublication) -> Result<(), DownloadErrorCode>;
}

pub enum XVerifiedAsset {
    Image(VerifiedImage),
    Video(VerifiedOutput),
}
impl XVerifiedAsset {
    pub fn path(&self) -> &Path {
        match self {
            Self::Image(i) => i.path(),
            Self::Video(v) => &v.path,
        }
    }
    pub fn container(&self) -> &str {
        match self {
            Self::Image(i) => i.container(),
            Self::Video(v) => &v.container,
        }
    }
    pub fn byte_length(&self) -> u64 {
        match self {
            Self::Image(i) => i.byte_length(),
            Self::Video(v) => v.byte_length,
        }
    }
}
pub trait XImageDownloader: Send + Sync {
    fn download(
        &self,
        item: &XSourceItem,
        directory: &Path,
        context: &XRequestContext,
    ) -> Result<XVerifiedAsset, DownloadErrorCode>;
}

use super::{
    download_activity::{DownloadActivityGate, DownloadLease, DownloadOwner},
    download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
    download_job::{CompletedLocationOpener, JobStaging},
    output_verifier::OutputVerifier,
    process::CancellationToken,
    x_analysis::{map_error, ResolvedXDownload, ResolvedXDownloadItem, XInventorySlot},
    x_source::{XInventory, XInventoryExtractor},
    x_video::XVideoAnalyzer,
};
use crate::domain::{
    download::{valid_opaque_id, DownloadProgress, DownloadState},
    media::Platform,
    x::{
        XDownloadProgress, XDownloadResult, XItemAvailability, XItemOutcome, XItemOutcomeState,
        XJobState, XMediaItem, XMediaKind,
    },
};
use std::{
    collections::{HashMap, HashSet},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
pub trait XDownloadEventEmitter: Send + Sync {
    fn emit(&self, progress: XDownloadProgress) -> Result<(), DownloadErrorCode>;
}
#[derive(Clone)]
pub struct XDownloadJobServices {
    pub activity: Arc<DownloadActivityGate>,
    pub inventory: Arc<dyn XInventoryExtractor>,
    pub image: Arc<dyn XImageDownloader>,
    pub video: Arc<dyn XVideoAnalyzer>,
    pub executor: Arc<dyn DownloadExecutor>,
    pub verifier: Arc<dyn OutputVerifier>,
    pub staging: Arc<dyn JobStaging>,
    pub publisher: Arc<dyn XOutputPublisher>,
    pub opener: Arc<dyn CompletedLocationOpener>,
    pub emitter: Arc<dyn XDownloadEventEmitter>,
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedXDownload {
    pub job_id: String,
}
struct Terminal {
    result: XDownloadResult,
    open_path: Option<PathBuf>,
}
#[derive(Default)]
struct State {
    active: HashMap<String, CancellationToken>,
    terminal: HashMap<String, Terminal>,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    idle: Condvar,
}
pub struct XDownloadJobManager {
    services: XDownloadJobServices,
    shared: Arc<Shared>,
    start_guard: Mutex<()>,
}
impl XDownloadJobManager {
    pub fn new(services: XDownloadJobServices) -> Arc<Self> {
        Arc::new(Self {
            services,
            shared: Arc::new(Shared::default()),
            start_guard: Mutex::new(()),
        })
    }
    pub fn start(
        &self,
        download: ResolvedXDownload,
        destination: PathBuf,
    ) -> Result<StartedXDownload, DownloadErrorCode> {
        validate_snapshot(&download)?;
        fs_metadata_directory(&destination)?;
        let destination = destination
            .canonicalize()
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        let _start = self
            .start_guard
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        if self
            .shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .closed
        {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        let lease = self
            .services
            .activity
            .reserve(DownloadOwner::X, &download.title)?;
        let job_id = lease.job_id();
        let request = XPublicationRequest::new(
            job_id.clone(),
            download.source.media_id().into(),
            download.title.clone(),
            download.creator.clone(),
            download.items.iter().map(|i| i.public.clone()).collect(),
        )?;
        let cancellation = CancellationToken::new();
        self.shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .active
            .insert(job_id.clone(), cancellation.clone());
        if let Err(error) = emit(
            &self.services,
            &job_id,
            XJobState::Queued,
            1,
            download.items.len() as u16,
            None,
        ) {
            self.remove_active(&job_id);
            return Err(error);
        }
        let worker = Worker {
            services: self.services.clone(),
            shared: self.shared.clone(),
            job_id: job_id.clone(),
            cancellation,
            download,
            destination,
            request,
            lease: Some(lease),
            outcomes: vec![],
            publication: None,
            staging: None,
            content_changed: false,
        };
        if thread::Builder::new()
            .name(format!("x-download-{job_id}"))
            .spawn(move || {
                let mut worker = worker;
                let error = match catch_unwind(AssertUnwindSafe(|| worker.execute())) {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(e),
                    Err(_) => Some(DownloadErrorCode::Unknown),
                };
                worker.fill_remaining(error.unwrap_or(DownloadErrorCode::Unknown));
            })
            .is_err()
        {
            self.remove_active(&job_id);
            return Err(DownloadErrorCode::ProcessFailed);
        }
        Ok(StartedXDownload { job_id })
    }
    fn remove_active(&self, id: &str) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.active.remove(id);
            self.shared.idle.notify_all();
        }
    }
    pub fn cancel(&self, id: &str) -> Result<(), DownloadErrorCode> {
        if !valid_opaque_id(id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        if let Some(token) = state.active.get(id) {
            token.cancel();
            return Ok(());
        }
        if state
            .terminal
            .get(id)
            .is_some_and(|t| t.result.state == XJobState::Canceled)
        {
            return Ok(());
        }
        Err(DownloadErrorCode::InvalidRequest)
    }
    pub fn result(&self, id: &str) -> Result<XDownloadResult, DownloadErrorCode> {
        if !valid_opaque_id(id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        self.shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .terminal
            .get(id)
            .map(|t| t.result.clone())
            .ok_or(DownloadErrorCode::InvalidRequest)
    }
    pub fn open_completed_location(&self, id: &str) -> Result<(), DownloadErrorCode> {
        if !valid_opaque_id(id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let path = self
            .shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .terminal
            .get(id)
            .and_then(|t| t.open_path.clone())
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        self.services.opener.open(&path)
    }
    pub fn shutdown(&self) {
        let _start = self.start_guard.lock();
        if let Ok(mut state) = self.shared.state.lock() {
            state.closed = true;
            for token in state.active.values() {
                token.cancel();
            }
        }
        let _ = self.wait_for_idle(Duration::from_secs(10));
    }
    pub fn wait_for_idle(&self, timeout: Duration) -> bool {
        let Ok(state) = self.shared.state.lock() else {
            return false;
        };
        self.shared
            .idle
            .wait_timeout_while(state, timeout, |s| !s.active.is_empty())
            .is_ok_and(|(s, _)| s.active.is_empty())
    }
}
impl Drop for XDownloadJobManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn validate_snapshot(download: &ResolvedXDownload) -> Result<(), DownloadErrorCode> {
    let mut identities = HashSet::new();
    if download.source.platform() != Platform::X
        || download.post_item_count == 0
        || download.post_item_count > 32
        || download.inventory_slots.len() != download.post_item_count as usize
        || download.items.is_empty()
        || download.items.len() + download.unavailable_count as usize
            > download.post_item_count as usize
    {
        return Err(DownloadErrorCode::InvalidRequest);
    }
    for (n, slot) in download.inventory_slots.iter().enumerate() {
        if slot.ordinal as usize != n + 1
            || slot.identity.as_ref().is_some_and(|id| {
                id.post_id != download.source.media_id()
                    || !valid_opaque_id(&id.media_id)
                    || !identities.insert(&id.media_id)
            })
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
    }
    for item in &download.items {
        let slot = download
            .inventory_slots
            .get(item.public.ordinal.saturating_sub(1) as usize)
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        if item.public.validate().is_err()
            || item.public.availability != XItemAvailability::Ready
            || slot.identity.as_ref() != Some(&item.identity)
            || slot.kind != item.public.kind
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
    }
    Ok(())
}
fn fs_metadata_directory(path: &Path) -> Result<std::fs::Metadata, DownloadErrorCode> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || reparse {
        return Err(DownloadErrorCode::DestinationUnavailable);
    }
    Ok(metadata)
}
fn emit(
    services: &XDownloadJobServices,
    id: &str,
    state: XJobState,
    index: u16,
    count: u16,
    metrics: Option<&DownloadProgress>,
) -> Result<(), DownloadErrorCode> {
    let downloaded = metrics.and_then(|p| p.downloaded_bytes);
    let total = metrics
        .and_then(|p| p.total_bytes)
        .filter(|total| downloaded.is_none_or(|done| done <= *total));
    let progress = XDownloadProgress {
        job_id: id.to_owned(),
        state,
        item_index: index,
        item_count: count,
        downloaded_bytes: downloaded,
        total_bytes: total,
        percent: metrics
            .and_then(|p| p.percent)
            .filter(|_| downloaded.is_some() && total.is_some()),
    };
    progress.validate()?;
    services.activity.record_x(&progress);
    catch_unwind(AssertUnwindSafe(|| services.emitter.emit(progress)))
        .unwrap_or(Err(DownloadErrorCode::ProcessFailed))
}
struct Worker {
    services: XDownloadJobServices,
    shared: Arc<Shared>,
    job_id: String,
    cancellation: CancellationToken,
    download: ResolvedXDownload,
    destination: PathBuf,
    request: XPublicationRequest,
    lease: Option<DownloadLease>,
    outcomes: Vec<XItemOutcome>,
    publication: Option<XPublication>,
    staging: Option<PathBuf>,
    content_changed: bool,
}
impl Worker {
    fn context(&self) -> XRequestContext {
        XRequestContext::new(
            self.cancellation.clone(),
            Instant::now() + Duration::from_secs(120),
            self.download.network.clone(),
        )
    }
    fn progress(&self, state: XJobState, index: usize) {
        let _ = emit(
            &self.services,
            &self.job_id,
            state,
            index as u16 + 1,
            self.download.items.len() as u16,
            None,
        );
    }
    fn execute(&mut self) -> Result<(), DownloadErrorCode> {
        self.lease
            .as_ref()
            .ok_or(DownloadErrorCode::Unknown)?
            .wait_turn(&self.cancellation)?;
        let inventory = self
            .services
            .inventory
            .extract_post(&self.download.source, &self.context())
            .map_err(map_error)?;
        if inventory.source.platform() != Platform::X
            || inventory.source.media_id() != self.download.source.media_id()
        {
            self.content_changed = true;
            return Err(DownloadErrorCode::ContentChanged);
        }
        if inventory.items.is_empty() || inventory.items.len() > 32 {
            return Err(DownloadErrorCode::ContentChanged);
        }
        let slots = inventory
            .items
            .iter()
            .map(|i| XInventorySlot {
                ordinal: i.ordinal,
                identity: i.identity.clone(),
                kind: i.kind,
            })
            .collect::<Vec<_>>();
        self.content_changed = slots != self.download.inventory_slots;
        for index in 0..self.download.items.len() {
            if self.cancellation.is_canceled() {
                break;
            }
            let selected = self.download.items[index].clone();
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.execute_one(index, &selected, &inventory)
            }))
            .unwrap_or(Err(DownloadErrorCode::Unknown));
            let outcome = match result {
                Ok(name) => XItemOutcome {
                    item_id: selected.public.item_id.clone(),
                    ordinal: selected.public.ordinal,
                    kind: selected.public.kind,
                    state: XItemOutcomeState::Completed,
                    display_name: Some(name),
                    error_code: None,
                },
                Err(error) => {
                    outcome_failure(&selected.public, self.cancellation.is_canceled(), error)
                }
            };
            self.outcomes.push(outcome);
        }
        Ok(())
    }
    fn execute_one(
        &mut self,
        index: usize,
        selected: &ResolvedXDownloadItem,
        inventory: &XInventory,
    ) -> Result<String, DownloadErrorCode> {
        let mut found = inventory
            .items
            .iter()
            .filter(|i| i.identity.as_ref() == Some(&selected.identity));
        let item = found.next().ok_or(DownloadErrorCode::ContentChanged)?;
        if found.next().is_some() || item.kind != selected.public.kind {
            return Err(DownloadErrorCode::ContentChanged);
        }
        if item.availability != XItemAvailability::Ready {
            return Err(item
                .unavailable_reason
                .map(map_error)
                .unwrap_or(DownloadErrorCode::ContentUnavailable));
        }
        if self.staging.is_none() {
            self.staging = Some(self.services.staging.create(&self.job_id)?);
        }
        if self.publication.is_none() {
            self.publication = Some(
                self.services
                    .publisher
                    .prepare(&self.destination, &self.request)?,
            );
        }
        let staging = self.staging.as_ref().ok_or(DownloadErrorCode::Unknown)?;
        let directory = staging.join(format!("{:02}", selected.public.ordinal));
        std::fs::create_dir(&directory).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        fs_metadata_directory(&directory)?;
        self.progress(XJobState::Downloading, index);
        let context = self.context();
        let asset = if item.kind == XMediaKind::Image {
            self.services.image.download(item, &directory, &context)?
        } else {
            self.download_video(index, selected, inventory, &directory, &context)?
        };
        if self.cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        self.progress(XJobState::Publishing, index);
        if self.cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        self.services.publisher.publish(
            asset,
            self.publication
                .as_ref()
                .ok_or(DownloadErrorCode::Unknown)?,
            &selected.public,
            &self.cancellation,
        )
    }
    fn download_video(
        &self,
        index: usize,
        selected: &ResolvedXDownloadItem,
        inventory: &XInventory,
        directory: &Path,
        context: &XRequestContext,
    ) -> Result<XVerifiedAsset, DownloadErrorCode> {
        let resolved = self
            .services
            .video
            .analyze_item(
                inventory,
                &selected.identity,
                self.download.compatibility_mode,
                context,
            )
            .map_err(map_error)?;
        if resolved.source().identity() != &selected.identity {
            return Err(DownloadErrorCode::ContentChanged);
        }
        let (media, plan, source) = resolved.into_parts();
        let child = self
            .job_id
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(100))
            .and_then(|n| n.checked_add(u64::from(selected.public.ordinal)))
            .map(|n| n.to_string())
            .filter(|n| valid_opaque_id(n))
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        let child_directory = directory.join(&child);
        std::fs::create_dir(&child_directory).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        let sink = ItemProgress {
            services: &self.services,
            job_id: &self.job_id,
            child_id: &child,
            index: index as u16 + 1,
            count: self.download.items.len() as u16,
            cancellation: self.cancellation.clone(),
            error: Mutex::new(None),
        };
        let video_source = media
            .formats
            .iter()
            .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
            .cloned();
        let audio_source = media
            .formats
            .iter()
            .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
            .cloned();
        let staged = self.services.executor.download(
            DownloadExecutionRequest {
                job_id: child.clone(),
                source: self.download.source.clone(),
                playlist_item: None,
                instagram_selector: None,
                tiktok_source: None,
                x_source: Some(source),
                bilibili_source: None,
                plan: plan.clone(),
                video_source,
                audio_source,
                staging_dir: child_directory.clone(),
            },
            self.cancellation.clone(),
            &sink,
        )?;
        if let Some(error) = sink
            .error
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .as_ref()
        {
            return Err(*error);
        }
        if self.cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        self.progress(XJobState::Verifying, index);
        let verified = self
            .services
            .verifier
            .verify(staged, &plan, self.cancellation.clone())?;
        if self.cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        if verified.path.canonicalize().ok().as_ref() != Some(&verified.path)
            || verified.path.parent() != child_directory.canonicalize().ok().as_deref()
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let destination = directory.join(format!("stage.{}", verified.container));
        if destination.exists() {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        // The private item directory is not concurrently written by another worker.
        std::fs::rename(&verified.path, &destination)
            .map_err(|_| DownloadErrorCode::OutputInvalid)?;
        let _ = std::fs::remove_dir(&child_directory);
        Ok(XVerifiedAsset::Video(VerifiedOutput {
            path: destination
                .canonicalize()
                .map_err(|_| DownloadErrorCode::OutputInvalid)?,
            ..verified
        }))
    }
    fn fill_remaining(&mut self, error: DownloadErrorCode) {
        for item in &self.download.items[self.outcomes.len()..] {
            self.outcomes.push(outcome_failure(
                &item.public,
                self.cancellation.is_canceled(),
                error,
            ));
        }
    }
    fn store_result(&mut self) {
        self.fill_remaining(DownloadErrorCode::Unknown);
        let completed = self
            .outcomes
            .iter()
            .filter(|o| o.state == XItemOutcomeState::Completed)
            .count() as u16;
        let failed = self
            .outcomes
            .iter()
            .filter(|o| o.state == XItemOutcomeState::Failed)
            .count() as u16;
        let canceled = self
            .outcomes
            .iter()
            .any(|o| o.state == XItemOutcomeState::Canceled);
        let state = if canceled {
            XJobState::Canceled
        } else if failed == 0 {
            XJobState::Completed
        } else if completed > 0 {
            XJobState::Partial
        } else {
            XJobState::Failed
        };
        let folder = if completed > 0 {
            self.publication
                .as_ref()
                .map(|p| p.folder_display_name().to_owned())
        } else {
            None
        };
        let open_path = if completed > 0 {
            self.publication.as_ref().map(|p| p.open_path().to_owned())
        } else {
            None
        };
        let result = XDownloadResult {
            job_id: self.job_id.clone(),
            state,
            post_item_count: self.download.post_item_count,
            unavailable_count: self.download.unavailable_count,
            content_changed: self.content_changed,
            completed_count: completed,
            failed_count: failed,
            folder_display_name: folder,
            items: self.outcomes.clone(),
        };
        if let Ok(mut state) = self.shared.state.lock() {
            state.terminal.insert(
                self.job_id.clone(),
                Terminal {
                    result: result.clone(),
                    open_path,
                },
            );
            while state.terminal.len() > 100 {
                let Some(oldest) = state
                    .terminal
                    .keys()
                    .min_by_key(|id| id.parse::<u64>().unwrap_or(0))
                    .cloned()
                else {
                    break;
                };
                state.terminal.remove(&oldest);
            }
        }
        self.services.activity.finish_details(
            &self.job_id,
            result.items.iter().find_map(|i| i.error_code),
            result.folder_display_name.clone(),
        );
        let _ = emit(
            &self.services,
            &self.job_id,
            result.state,
            result.items.len() as u16,
            result.items.len() as u16,
            None,
        );
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.store_result()));
        if self.staging.is_some() {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                self.services.staging.cleanup(&self.job_id)
            }));
        }
        if !self
            .outcomes
            .iter()
            .any(|o| o.state == XItemOutcomeState::Completed)
        {
            if let Some(publication) = &self.publication {
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    self.services.publisher.cleanup_empty(publication)
                }));
            }
        }
        drop(self.lease.take());
        if let Ok(mut state) = self.shared.state.lock() {
            state.active.remove(&self.job_id);
            self.shared.idle.notify_all();
        }
    }
}
fn outcome_failure(item: &XMediaItem, canceled: bool, error: DownloadErrorCode) -> XItemOutcome {
    XItemOutcome {
        item_id: item.item_id.clone(),
        ordinal: item.ordinal,
        kind: item.kind,
        state: if canceled {
            XItemOutcomeState::Canceled
        } else {
            XItemOutcomeState::Failed
        },
        display_name: None,
        error_code: if canceled { None } else { Some(error) },
    }
}
struct ItemProgress<'a> {
    services: &'a XDownloadJobServices,
    job_id: &'a str,
    child_id: &'a str,
    index: u16,
    count: u16,
    cancellation: CancellationToken,
    error: Mutex<Option<DownloadErrorCode>>,
}
impl DownloadProgressSink for ItemProgress<'_> {
    fn on_progress(&self, progress: DownloadProgress) {
        if self.cancellation.is_canceled() {
            return;
        }
        let state = match progress.state {
            DownloadState::Downloading => Some(XJobState::Downloading),
            DownloadState::Processing => Some(XJobState::Processing),
            _ => None,
        };
        if progress.job_id != self.child_id || state.is_none() {
            if let Ok(mut error) = self.error.lock() {
                *error = Some(DownloadErrorCode::OutputInvalid);
            }
            self.cancellation.cancel();
            return;
        }
        let _ = emit(
            self.services,
            self.job_id,
            state.unwrap_or(XJobState::Downloading),
            self.index,
            self.count,
            Some(&progress),
        );
    }
}
