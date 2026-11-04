use std::{
    collections::HashMap,
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use crate::{
    domain::{
        download::{valid_opaque_id, DownloadErrorCode, DownloadProgress, DownloadState},
        instagram::{
            InstagramDownloadProgress, InstagramDownloadResult, InstagramItemOutcome,
            InstagramItemOutcomeState, InstagramJobState, InstagramMediaItem, InstagramSourceKind,
        },
    },
    infrastructure::gallery_dl::{InstagramAssetAcquirer, ResolvedInstagramItem},
};

use super::{
    download_activity::{DownloadActivityGate, DownloadLease, DownloadOwner},
    download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
    download_job::{CompletedLocationOpener, JobStaging},
    instagram_analysis::{
        map_analysis_download_error, InstagramVideoAnalyzer, ResolvedInstagramDownload,
        ResolvedInstagramDownloadItem, ResolvedInstagramDownloadKind,
    },
    instagram_video::InstagramVideoSelector,
    media_url::ValidatedMediaUrl,
    output_verifier::OutputVerifier,
    process::CancellationToken,
};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedInstagramDownload {
    pub job_id: String,
}

pub struct InstagramVerifiedAsset {
    path: PathBuf,
    container: String,
    byte_length: u64,
}

impl InstagramVerifiedAsset {
    pub fn new(
        path: PathBuf,
        container: impl Into<String>,
        byte_length: u64,
    ) -> Result<Self, DownloadErrorCode> {
        let container = container.into();
        if path.as_os_str().is_empty()
            || byte_length == 0
            || byte_length > MAX_SAFE_INTEGER
            || !safe_container(&container)
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(Self {
            path,
            container,
            byte_length,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn container(&self) -> &str {
        &self.container
    }

    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }
}

pub trait InstagramImageDownloader: Send + Sync {
    fn download(
        &self,
        item: &ResolvedInstagramItem,
        directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<InstagramVerifiedAsset, DownloadErrorCode>;
}

impl<T> InstagramImageDownloader for T
where
    T: InstagramAssetAcquirer + Send + Sync,
{
    fn download(
        &self,
        item: &ResolvedInstagramItem,
        directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<InstagramVerifiedAsset, DownloadErrorCode> {
        let image = InstagramAssetAcquirer::download_image(self, item, directory, cancellation)?;
        InstagramVerifiedAsset::new(
            image.path().to_owned(),
            image.container(),
            image.byte_length(),
        )
    }
}

pub trait InstagramVideoDownloader: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn download(
        &self,
        source: &ValidatedMediaUrl,
        selector: &InstagramVideoSelector,
        compatibility_mode: bool,
        job_id: &str,
        directory: &Path,
        cancellation: CancellationToken,
        progress: &dyn DownloadProgressSink,
    ) -> Result<InstagramVerifiedAsset, DownloadErrorCode>;
}

pub struct InstagramVideoDownloadPipeline<A, E, V> {
    analyzer: A,
    executor: E,
    verifier: V,
}

impl<A, E, V> InstagramVideoDownloadPipeline<A, E, V> {
    pub fn new(analyzer: A, executor: E, verifier: V) -> Self {
        Self {
            analyzer,
            executor,
            verifier,
        }
    }
}

impl<A, E, V> InstagramVideoDownloader for InstagramVideoDownloadPipeline<A, E, V>
where
    A: InstagramVideoAnalyzer,
    E: DownloadExecutor,
    V: OutputVerifier,
{
    fn download(
        &self,
        source: &ValidatedMediaUrl,
        selector: &InstagramVideoSelector,
        compatibility_mode: bool,
        job_id: &str,
        directory: &Path,
        cancellation: CancellationToken,
        progress: &dyn DownloadProgressSink,
    ) -> Result<InstagramVerifiedAsset, DownloadErrorCode> {
        let resolved = match selector {
            InstagramVideoSelector::Reel => {
                self.analyzer
                    .analyze_reel(source, compatibility_mode, &cancellation)
            }
            InstagramVideoSelector::PostItem {
                ordinal,
                expected_media_id,
            } => self.analyzer.analyze_post_item(
                source,
                *ordinal,
                expected_media_id,
                compatibility_mode,
                &cancellation,
            ),
        }
        .map_err(map_analysis_download_error)?;
        if resolved.selector() != selector {
            return Err(DownloadErrorCode::ContentChanged);
        }
        let (media, resolved_selector, plan) = resolved.into_parts();
        let item_ordinal = resolved_selector.playlist_item().unwrap_or(1);
        let child_job_id = child_job_id(job_id, item_ordinal)?;
        let child_directory = directory.join(&child_job_id);
        create_item_directory(&child_directory)?;
        let video_source = media
            .formats
            .iter()
            .find(|row| Some(&row.id) == plan.video_format_id.as_ref())
            .cloned();
        let audio_source = media
            .formats
            .iter()
            .find(|row| Some(&row.id) == plan.audio_format_id.as_ref())
            .cloned();
        let remapped_progress = RemappedVideoProgress {
            child_job_id: &child_job_id,
            parent_job_id: job_id,
            inner: progress,
        };
        let staged = self.executor.download(
            DownloadExecutionRequest {
                job_id: child_job_id.clone(),
                source: source.clone(),
                playlist_item: resolved_selector.playlist_item(),
                instagram_selector: Some(resolved_selector),
                tiktok_source: None,
                x_source: None,
                bilibili_source: None,
                plan: plan.clone(),
                video_source,
                audio_source,
                staging_dir: child_directory.clone(),
            },
            cancellation.clone(),
            &remapped_progress,
        )?;
        let verified = self.verifier.verify(staged, &plan, cancellation)?;
        let destination = directory.join(format!("stage.{}", verified.container));
        if destination.exists() {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        fs::rename(&verified.path, &destination).map_err(|error| {
            if matches!(error.raw_os_error(), Some(28 | 112)) {
                DownloadErrorCode::DiskFull
            } else {
                DownloadErrorCode::OutputInvalid
            }
        })?;
        let _ = fs::remove_dir(&child_directory);
        let destination = destination
            .canonicalize()
            .map_err(|_| DownloadErrorCode::OutputInvalid)?;
        InstagramVerifiedAsset::new(destination, verified.container, verified.byte_length)
    }
}

struct RemappedVideoProgress<'a> {
    child_job_id: &'a str,
    parent_job_id: &'a str,
    inner: &'a dyn DownloadProgressSink,
}

impl DownloadProgressSink for RemappedVideoProgress<'_> {
    fn on_progress(&self, mut progress: DownloadProgress) {
        if progress.job_id == self.child_job_id {
            progress.job_id = self.parent_job_id.to_owned();
        }
        self.inner.on_progress(progress);
    }
}

fn child_job_id(parent_job_id: &str, ordinal: u16) -> Result<String, DownloadErrorCode> {
    let parent = parent_job_id
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(DownloadErrorCode::InvalidRequest)?;
    let value = parent
        .checked_mul(100)
        .and_then(|value| value.checked_add(u64::from(ordinal)))
        .ok_or(DownloadErrorCode::Unknown)?;
    let value = value.to_string();
    if valid_opaque_id(&value) {
        Ok(value)
    } else {
        Err(DownloadErrorCode::Unknown)
    }
}

#[derive(Clone)]
pub struct InstagramPublicationRequest {
    job_id: String,
    source_kind: InstagramSourceKind,
    shortcode: String,
    title: String,
    creator: Option<String>,
    item_count: u16,
    selected_ordinals: Vec<u16>,
}

impl InstagramPublicationRequest {
    pub fn new(
        job_id: impl Into<String>,
        source_kind: InstagramSourceKind,
        shortcode: impl Into<String>,
        title: impl Into<String>,
        creator: Option<String>,
        item_count: u16,
    ) -> Result<Self, DownloadErrorCode> {
        Self::new_with_ordinals(
            job_id,
            source_kind,
            shortcode,
            title,
            creator,
            (1..=item_count).collect(),
        )
    }

    fn new_with_ordinals(
        job_id: impl Into<String>,
        source_kind: InstagramSourceKind,
        shortcode: impl Into<String>,
        title: impl Into<String>,
        creator: Option<String>,
        selected_ordinals: Vec<u16>,
    ) -> Result<Self, DownloadErrorCode> {
        let item_count = u16::try_from(selected_ordinals.len())
            .ok()
            .filter(|count| (1..=32).contains(count))
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        let value = Self {
            job_id: job_id.into(),
            source_kind,
            shortcode: shortcode.into(),
            title: title.into(),
            creator,
            item_count,
            selected_ordinals,
        };
        if !valid_opaque_id(&value.job_id)
            || value.shortcode.is_empty()
            || value.shortcode.len() > 64
            || !value
                .shortcode
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || value.title.trim().is_empty()
            || value.title.chars().count() > 512
            || value.creator.as_deref().is_some_and(|creator| {
                creator.trim().is_empty()
                    || creator != creator.trim()
                    || creator.chars().count() > 100
            })
            || (value.source_kind == InstagramSourceKind::Reel && value.item_count != 1)
            || value
                .selected_ordinals
                .iter()
                .any(|ordinal| *ordinal == 0 || *ordinal > 32)
            || value
                .selected_ordinals
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        Ok(value)
    }

    fn from_download(
        job_id: &str,
        download: &ResolvedInstagramDownload,
    ) -> Result<Self, DownloadErrorCode> {
        let source_kind = download
            .source()
            .instagram_kind()
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        let selected_ordinals = download
            .items()
            .iter()
            .map(|item| item.public_item().ordinal)
            .collect();
        Self::new_with_ordinals(
            job_id,
            source_kind,
            download.source().media_id(),
            download.title(),
            download.creator().map(str::to_owned),
            selected_ordinals,
        )
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub const fn source_kind(&self) -> InstagramSourceKind {
        self.source_kind
    }

    pub fn shortcode(&self) -> &str {
        &self.shortcode
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn creator(&self) -> Option<&str> {
        self.creator.as_deref()
    }

    pub const fn item_count(&self) -> u16 {
        self.item_count
    }

    pub fn accepts_ordinal(&self, ordinal: u16) -> bool {
        self.selected_ordinals.binary_search(&ordinal).is_ok()
    }
}

pub struct InstagramPublication {
    output_directory: PathBuf,
    open_path: PathBuf,
    folder_display_name: Option<String>,
    request: InstagramPublicationRequest,
}

impl InstagramPublication {
    pub fn new(
        output_directory: PathBuf,
        open_path: PathBuf,
        folder_display_name: Option<String>,
        request: InstagramPublicationRequest,
    ) -> Result<Self, DownloadErrorCode> {
        if output_directory.as_os_str().is_empty()
            || open_path.as_os_str().is_empty()
            || folder_display_name
                .as_deref()
                .is_some_and(|name| !safe_display_name(name))
            || (request.item_count > 1) != folder_display_name.is_some()
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(Self {
            output_directory,
            open_path,
            folder_display_name,
            request,
        })
    }

    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }

    pub fn open_path(&self) -> &Path {
        &self.open_path
    }

    pub fn folder_display_name(&self) -> Option<&str> {
        self.folder_display_name.as_deref()
    }

    pub fn request(&self) -> &InstagramPublicationRequest {
        &self.request
    }
}

pub trait InstagramOutputPublisher: Send + Sync {
    fn prepare(
        &self,
        destination: &Path,
        request: &InstagramPublicationRequest,
    ) -> Result<InstagramPublication, DownloadErrorCode>;

    fn publish(
        &self,
        asset: InstagramVerifiedAsset,
        publication: &InstagramPublication,
        item: &InstagramMediaItem,
        cancellation: &CancellationToken,
    ) -> Result<String, DownloadErrorCode>;
}

pub trait InstagramDownloadEventEmitter: Send + Sync {
    fn emit(&self, progress: InstagramDownloadProgress) -> Result<(), DownloadErrorCode>;
}

struct TrackingEmitter {
    inner: Arc<dyn InstagramDownloadEventEmitter>,
    activity: Arc<DownloadActivityGate>,
}
impl InstagramDownloadEventEmitter for TrackingEmitter {
    fn emit(&self, progress: InstagramDownloadProgress) -> Result<(), DownloadErrorCode> {
        self.activity.record_instagram(&progress);
        self.inner.emit(progress)
    }
}

pub struct InstagramDownloadJobServices {
    pub activity: Arc<DownloadActivityGate>,
    pub image: Arc<dyn InstagramImageDownloader>,
    pub video: Arc<dyn InstagramVideoDownloader>,
    pub publisher: Arc<dyn InstagramOutputPublisher>,
    pub staging: Arc<dyn JobStaging>,
    pub emitter: Arc<dyn InstagramDownloadEventEmitter>,
    pub opener: Arc<dyn CompletedLocationOpener>,
}

impl Clone for InstagramDownloadJobServices {
    fn clone(&self) -> Self {
        Self {
            activity: Arc::clone(&self.activity),
            image: Arc::clone(&self.image),
            video: Arc::clone(&self.video),
            publisher: Arc::clone(&self.publisher),
            staging: Arc::clone(&self.staging),
            emitter: Arc::clone(&self.emitter),
            opener: Arc::clone(&self.opener),
        }
    }
}

struct TerminalJob {
    result: InstagramDownloadResult,
    open_path: Option<PathBuf>,
}

struct BatchOutcome {
    result: InstagramDownloadResult,
    open_path: Option<PathBuf>,
}

#[derive(Default)]
struct JobState {
    active: HashMap<String, CancellationToken>,
    terminal: HashMap<String, TerminalJob>,
    closed: bool,
}

#[derive(Default)]
struct SharedJobs {
    state: Mutex<JobState>,
    idle: Condvar,
}

pub struct InstagramDownloadJobManager {
    services: InstagramDownloadJobServices,
    shared: Arc<SharedJobs>,
    start_guard: Mutex<()>,
}

impl InstagramDownloadJobManager {
    pub fn new(mut services: InstagramDownloadJobServices) -> Arc<Self> {
        services.emitter = Arc::new(TrackingEmitter {
            inner: Arc::clone(&services.emitter),
            activity: Arc::clone(&services.activity),
        });
        Arc::new(Self {
            services,
            shared: Arc::new(SharedJobs::default()),
            start_guard: Mutex::new(()),
        })
    }

    pub fn start(
        &self,
        download: ResolvedInstagramDownload,
        destination: PathBuf,
    ) -> Result<StartedInstagramDownload, DownloadErrorCode> {
        if destination.as_os_str().is_empty() || download.items().is_empty() {
            return Err(DownloadErrorCode::InvalidRequest);
        }
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
        let activity = self
            .services
            .activity
            .reserve(DownloadOwner::Instagram, download.title())?;
        let job_id = activity.job_id();
        let request = InstagramPublicationRequest::from_download(&job_id, &download)?;
        let cancellation = CancellationToken::new();
        {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| DownloadErrorCode::Unknown)?;
            state.active.insert(job_id.clone(), cancellation.clone());
        }
        if let Err(error) = emit_progress(
            self.services.emitter.as_ref(),
            &job_id,
            InstagramJobState::Queued,
            1,
            request.item_count,
            None,
        ) {
            self.clear_failed_start(&job_id);
            return Err(error);
        }

        let services = self.services.clone();
        let shared = Arc::clone(&self.shared);
        let worker_job_id = job_id.clone();
        let spawn = thread::Builder::new()
            .name(format!("instagram-download-{job_id}"))
            .spawn(move || {
                run_job(
                    services,
                    shared,
                    worker_job_id,
                    cancellation,
                    download,
                    destination,
                    request,
                    activity,
                );
            });
        if spawn.is_err() {
            self.clear_failed_start(&job_id);
            return Err(DownloadErrorCode::ProcessFailed);
        }
        Ok(StartedInstagramDownload { job_id })
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        if !valid_opaque_id(job_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        if let Some(cancellation) = state.active.get(job_id) {
            cancellation.cancel();
            return Ok(());
        }
        if state.terminal.get(job_id).is_some_and(|terminal| {
            terminal.result.job_id == job_id && terminal.result.state == InstagramJobState::Canceled
        }) {
            return Ok(());
        }
        Err(DownloadErrorCode::InvalidRequest)
    }

    pub fn result(&self, job_id: &str) -> Result<InstagramDownloadResult, DownloadErrorCode> {
        if !valid_opaque_id(job_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        self.shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .terminal
            .get(job_id)
            .map(|terminal| terminal.result.clone())
            .ok_or(DownloadErrorCode::InvalidRequest)
    }

    pub fn open_completed_location(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        if !valid_opaque_id(job_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let path = self
            .shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .terminal
            .get(job_id)
            .and_then(|terminal| terminal.open_path.clone())
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        self.services.opener.open(&path)
    }

    pub fn has_active_job(&self) -> bool {
        self.shared
            .state
            .lock()
            .map(|state| !state.active.is_empty())
            .unwrap_or(true)
    }

    pub fn shutdown(&self) {
        let _start = self.start_guard.lock();
        if let Ok(mut state) = self.shared.state.lock() {
            state.closed = true;
            for cancellation in state.active.values() {
                cancellation.cancel();
            }
        }
        let _ = self.wait_for_idle(Duration::from_secs(10));
    }

    #[doc(hidden)]
    pub fn wait_for_idle(&self, timeout: Duration) -> bool {
        let Ok(state) = self.shared.state.lock() else {
            return false;
        };
        let Ok((state, _)) = self
            .shared
            .idle
            .wait_timeout_while(state, timeout, |state| !state.active.is_empty())
        else {
            return false;
        };
        state.active.is_empty()
    }

    fn clear_failed_start(&self, job_id: &str) {
        if let Ok(mut state) = self.shared.state.lock() {
            if state.active.remove(job_id).is_some() {
                self.shared.idle.notify_all();
            }
        }
    }
}

impl Drop for InstagramDownloadJobManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_job(
    services: InstagramDownloadJobServices,
    shared: Arc<SharedJobs>,
    job_id: String,
    cancellation: CancellationToken,
    download: ResolvedInstagramDownload,
    destination: PathBuf,
    request: InstagramPublicationRequest,
    activity: DownloadLease,
) {
    let fallback = download
        .items()
        .iter()
        .map(|item| item.public_item().clone())
        .collect::<Vec<_>>();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if let Err(error) = activity.wait_turn(&cancellation) {
            let mut outcomes = Vec::new();
            append_canceled(&mut outcomes, download.items());
            return BatchOutcome {
                result: if cancellation.is_canceled() {
                    build_result(&job_id, outcomes, None)
                } else {
                    failed_result(&job_id, &fallback, error)
                },
                open_path: None,
            };
        }
        execute_batch(
            &services,
            &shared,
            &job_id,
            cancellation.clone(),
            &download,
            &destination,
            &request,
        )
    }))
    .unwrap_or_else(|_| BatchOutcome {
        result: failed_result(&job_id, &fallback, DownloadErrorCode::Unknown),
        open_path: None,
    });
    let _ = services.staging.cleanup(&job_id);
    let last_index = outcome
        .result
        .items
        .iter()
        .rposition(|item| {
            matches!(
                item.state,
                InstagramItemOutcomeState::Completed
                    | InstagramItemOutcomeState::Failed
                    | InstagramItemOutcomeState::Canceled
            )
        })
        .map(|index| index as u16 + 1)
        .unwrap_or(1);
    let should_finish = if let Ok(mut state) = shared.state.lock() {
        let matches = state.active.contains_key(&job_id);
        if matches {
            state.terminal.insert(
                job_id.clone(),
                TerminalJob {
                    result: outcome.result.clone(),
                    open_path: outcome.open_path.clone(),
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
        matches
    } else {
        false
    };
    if should_finish {
        services.activity.finish_details(
            &job_id,
            outcome.result.items.iter().find_map(|item| item.error_code),
            outcome.result.folder_display_name.clone().or_else(|| {
                outcome
                    .result
                    .items
                    .iter()
                    .find_map(|item| item.display_name.clone())
            }),
        );
        let _ = emit_progress(
            services.emitter.as_ref(),
            &job_id,
            outcome.result.state,
            last_index,
            request.item_count,
            None,
        );
    }
    drop(activity);
    if let Ok(mut state) = shared.state.lock() {
        if state.active.remove(&job_id).is_some() {
            shared.idle.notify_all();
        }
    }
}

fn execute_batch(
    services: &InstagramDownloadJobServices,
    shared: &Arc<SharedJobs>,
    job_id: &str,
    cancellation: CancellationToken,
    download: &ResolvedInstagramDownload,
    destination: &Path,
    request: &InstagramPublicationRequest,
) -> BatchOutcome {
    let public = download
        .items()
        .iter()
        .map(|item| item.public_item().clone())
        .collect::<Vec<_>>();
    let staging = match services.staging.create(job_id) {
        Ok(staging) => staging,
        Err(error) => {
            return BatchOutcome {
                result: failed_result(job_id, &public, error),
                open_path: None,
            }
        }
    };
    let publication = match services.publisher.prepare(destination, request) {
        Ok(publication) => publication,
        Err(error) => {
            return BatchOutcome {
                result: failed_result(job_id, &public, error),
                open_path: None,
            }
        }
    };
    let mut outcomes = Vec::with_capacity(download.items().len());
    for (index, item) in download.items().iter().enumerate() {
        let item_index = index as u16 + 1;
        if cancellation.is_canceled() {
            append_canceled(&mut outcomes, &download.items()[index..]);
            break;
        }
        let directory = staging.join(format!("{:02}", item.public_item().ordinal));
        let item_result = create_item_directory(&directory).and_then(|()| {
            emit_progress(
                services.emitter.as_ref(),
                job_id,
                InstagramJobState::Downloading,
                item_index,
                request.item_count,
                None,
            )?;
            let progress = InstagramItemProgress::new(
                job_id,
                item_index,
                request.item_count,
                Arc::clone(shared),
                Arc::clone(&services.emitter),
                cancellation.clone(),
            );
            let asset = execute_item(
                services,
                job_id,
                download,
                item,
                &directory,
                cancellation.clone(),
                &progress,
            );
            if let Some(error) = progress.error() {
                return Err(error);
            }
            let asset = asset?;
            if cancellation.is_canceled() {
                return Err(DownloadErrorCode::ProcessFailed);
            }
            emit_progress(
                services.emitter.as_ref(),
                job_id,
                InstagramJobState::Verifying,
                item_index,
                request.item_count,
                None,
            )?;
            emit_progress(
                services.emitter.as_ref(),
                job_id,
                InstagramJobState::Publishing,
                item_index,
                request.item_count,
                None,
            )?;
            services
                .publisher
                .publish(asset, &publication, item.public_item(), &cancellation)
        });
        match item_result {
            Ok(display_name) => outcomes.push(InstagramItemOutcome {
                item_id: item.public_item().item_id.clone(),
                ordinal: item.public_item().ordinal,
                kind: item.public_item().kind,
                state: InstagramItemOutcomeState::Completed,
                display_name: Some(display_name),
                error_code: None,
            }),
            Err(_) if cancellation.is_canceled() => {
                append_canceled(&mut outcomes, &download.items()[index..]);
                break;
            }
            Err(error) => outcomes.push(InstagramItemOutcome {
                item_id: item.public_item().item_id.clone(),
                ordinal: item.public_item().ordinal,
                kind: item.public_item().kind,
                state: InstagramItemOutcomeState::Failed,
                display_name: None,
                error_code: Some(error),
            }),
        }
    }
    let result = build_result(job_id, outcomes, Some(&publication));
    if result.completed_count == 0 && publication.folder_display_name().is_some() {
        let _ = fs::remove_dir(publication.output_directory());
    }
    let open_path = (result.completed_count > 0).then(|| publication.open_path().to_owned());
    BatchOutcome { result, open_path }
}

#[allow(clippy::too_many_arguments)]
fn execute_item(
    services: &InstagramDownloadJobServices,
    job_id: &str,
    download: &ResolvedInstagramDownload,
    item: &ResolvedInstagramDownloadItem,
    directory: &Path,
    cancellation: CancellationToken,
    progress: &dyn DownloadProgressSink,
) -> Result<InstagramVerifiedAsset, DownloadErrorCode> {
    match item.kind() {
        ResolvedInstagramDownloadKind::Image(image) => {
            services.image.download(image, directory, &cancellation)
        }
        ResolvedInstagramDownloadKind::Video(selector) => services.video.download(
            download.source(),
            selector,
            download.compatibility_mode(),
            job_id,
            directory,
            cancellation,
            progress,
        ),
    }
}

fn append_canceled(
    outcomes: &mut Vec<InstagramItemOutcome>,
    items: &[ResolvedInstagramDownloadItem],
) {
    outcomes.extend(items.iter().map(|item| InstagramItemOutcome {
        item_id: item.public_item().item_id.clone(),
        ordinal: item.public_item().ordinal,
        kind: item.public_item().kind,
        state: InstagramItemOutcomeState::Canceled,
        display_name: None,
        error_code: None,
    }));
}

fn failed_result(
    job_id: &str,
    items: &[InstagramMediaItem],
    error: DownloadErrorCode,
) -> InstagramDownloadResult {
    build_result(
        job_id,
        items
            .iter()
            .map(|item| InstagramItemOutcome {
                item_id: item.item_id.clone(),
                ordinal: item.ordinal,
                kind: item.kind,
                state: InstagramItemOutcomeState::Failed,
                display_name: None,
                error_code: Some(error),
            })
            .collect(),
        None,
    )
}

fn build_result(
    job_id: &str,
    items: Vec<InstagramItemOutcome>,
    publication: Option<&InstagramPublication>,
) -> InstagramDownloadResult {
    let completed_count = items
        .iter()
        .filter(|item| item.state == InstagramItemOutcomeState::Completed)
        .count() as u16;
    let failed_count = items
        .iter()
        .filter(|item| item.state == InstagramItemOutcomeState::Failed)
        .count() as u16;
    let canceled = items
        .iter()
        .any(|item| item.state == InstagramItemOutcomeState::Canceled);
    let state = if canceled {
        InstagramJobState::Canceled
    } else if completed_count as usize == items.len() {
        InstagramJobState::Completed
    } else if completed_count > 0 {
        InstagramJobState::Partial
    } else {
        InstagramJobState::Failed
    };
    let folder_display_name = (completed_count > 0)
        .then(|| publication.and_then(InstagramPublication::folder_display_name))
        .flatten()
        .map(str::to_owned);
    InstagramDownloadResult {
        job_id: job_id.to_owned(),
        state,
        completed_count,
        failed_count,
        folder_display_name,
        items,
    }
}

fn create_item_directory(directory: &Path) -> Result<(), DownloadErrorCode> {
    fs::create_dir(directory).map_err(|error| {
        if matches!(error.raw_os_error(), Some(28 | 112)) {
            DownloadErrorCode::DiskFull
        } else {
            DownloadErrorCode::PublicationFailed
        }
    })
}

fn emit_progress(
    emitter: &dyn InstagramDownloadEventEmitter,
    job_id: &str,
    state: InstagramJobState,
    item_index: u16,
    item_count: u16,
    metrics: Option<&DownloadProgress>,
) -> Result<(), DownloadErrorCode> {
    let progress = InstagramDownloadProgress {
        job_id: job_id.to_owned(),
        state,
        item_index,
        item_count,
        downloaded_bytes: metrics.and_then(|value| value.downloaded_bytes),
        total_bytes: metrics.and_then(|value| value.total_bytes),
        percent: metrics.and_then(|value| value.percent),
    };
    progress.validate()?;
    emitter.emit(progress)
}

struct InstagramItemProgress {
    job_id: String,
    item_index: u16,
    item_count: u16,
    shared: Arc<SharedJobs>,
    emitter: Arc<dyn InstagramDownloadEventEmitter>,
    cancellation: CancellationToken,
    error: Mutex<Option<DownloadErrorCode>>,
}

impl InstagramItemProgress {
    fn new(
        job_id: &str,
        item_index: u16,
        item_count: u16,
        shared: Arc<SharedJobs>,
        emitter: Arc<dyn InstagramDownloadEventEmitter>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            job_id: job_id.to_owned(),
            item_index,
            item_count,
            shared,
            emitter,
            cancellation,
            error: Mutex::new(None),
        }
    }

    fn error(&self) -> Option<DownloadErrorCode> {
        self.error.lock().ok().and_then(|error| *error)
    }

    fn is_current(&self) -> bool {
        !self.cancellation.is_canceled()
            && self
                .shared
                .state
                .lock()
                .ok()
                .is_some_and(|state| state.active.contains_key(&self.job_id))
    }

    fn reject(&self, error: DownloadErrorCode) {
        if let Ok(mut slot) = self.error.lock() {
            slot.get_or_insert(error);
        }
        self.cancellation.cancel();
    }
}

impl DownloadProgressSink for InstagramItemProgress {
    fn on_progress(&self, progress: DownloadProgress) {
        if !self.is_current() {
            return;
        }
        let state = match progress.state {
            DownloadState::Downloading => InstagramJobState::Downloading,
            DownloadState::Processing => InstagramJobState::Processing,
            _ => {
                self.reject(DownloadErrorCode::OutputInvalid);
                return;
            }
        };
        if progress.job_id != self.job_id
            || emit_progress(
                self.emitter.as_ref(),
                &self.job_id,
                state,
                self.item_index,
                self.item_count,
                Some(&progress),
            )
            .is_err()
        {
            self.reject(DownloadErrorCode::OutputInvalid);
        }
    }
}

fn safe_container(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn safe_display_name(value: &str) -> bool {
    !value.is_empty()
        && value == value.trim()
        && value.chars().count() <= 160
        && !value.chars().any(|character| {
            character.is_control()
                || matches!(character, '/' | '\\' | ':')
                || matches!(
                    character,
                    '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
}
