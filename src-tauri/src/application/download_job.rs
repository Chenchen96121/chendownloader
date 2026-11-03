use std::{
    collections::HashMap,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use crate::domain::{
    download::{
        valid_opaque_id, DownloadErrorCode, DownloadMessageKey, DownloadProgress,
        DownloadProgressMetrics, DownloadState,
    },
    format_selection::{SelectionIntent, SelectionPlan},
    media::MediaInfo,
};

use super::{
    download_activity::{DownloadActivityGate, DownloadLease, DownloadOwner},
    download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
    media_url::ValidatedMediaUrl,
    output_publisher::{OutputPublisher, PublishedOutput},
    output_verifier::OutputVerifier,
    process::CancellationToken,
};

pub struct ResolvedDownloadInput {
    pub source: ValidatedMediaUrl,
    pub media: MediaInfo,
    pub plan: SelectionPlan,
    pub bilibili: Option<super::bilibili_download::BilibiliDownloadSnapshot>,
}

pub trait DownloadInputResolver: Send + Sync {
    fn resolve_download(
        &self,
        analysis_id: &str,
        intent: SelectionIntent,
    ) -> Result<ResolvedDownloadInput, DownloadErrorCode>;
}

pub trait DownloadDestinationResolver: Send + Sync {
    fn resolve_download_destination(
        &self,
        destination_id: &str,
    ) -> Result<PathBuf, DownloadErrorCode>;
}

pub trait JobStaging: Send + Sync {
    fn create(&self, job_id: &str) -> Result<PathBuf, DownloadErrorCode>;
    fn cleanup(&self, job_id: &str) -> Result<(), DownloadErrorCode>;
}

pub trait DownloadEventEmitter: Send + Sync {
    fn emit(&self, progress: DownloadProgress) -> Result<(), DownloadErrorCode>;
}

struct TrackingEmitter {
    inner: Arc<dyn DownloadEventEmitter>,
    activity: Arc<DownloadActivityGate>,
    owner: DownloadOwner,
}
impl DownloadEventEmitter for TrackingEmitter {
    fn emit(&self, progress: DownloadProgress) -> Result<(), DownloadErrorCode> {
        self.activity.record_single_video(self.owner, &progress);
        self.inner.emit(progress)
    }
}

fn trim_history<T>(history: &mut HashMap<String, T>) {
    while history.len() > 100 {
        let Some(oldest) = history
            .keys()
            .min_by_key(|id| id.parse::<u64>().unwrap_or(0))
            .cloned()
        else {
            break;
        };
        history.remove(&oldest);
    }
}
fn state_display_name(shared: &SharedJobRegistry, job_id: &str) -> Option<String> {
    shared.state.lock().ok().and_then(|state| {
        state
            .terminal
            .get(job_id)
            .and_then(|job| job.display_name.clone())
    })
}

pub trait CompletedLocationOpener: Send + Sync {
    fn open(&self, path: &Path) -> Result<(), DownloadErrorCode>;
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedDownload {
    pub job_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    pub job_id: String,
    pub state: DownloadState,
    pub error_code: Option<DownloadErrorCode>,
    pub display_name: Option<String>,
}

pub struct DownloadJobServices {
    pub activity: Arc<DownloadActivityGate>,
    pub input: Arc<dyn DownloadInputResolver>,
    pub destination: Arc<dyn DownloadDestinationResolver>,
    pub executor: Arc<dyn DownloadExecutor>,
    pub verifier: Arc<dyn OutputVerifier>,
    pub publisher: Arc<dyn OutputPublisher>,
    pub staging: Arc<dyn JobStaging>,
    pub emitter: Arc<dyn DownloadEventEmitter>,
    pub opener: Arc<dyn CompletedLocationOpener>,
}

impl Clone for DownloadJobServices {
    fn clone(&self) -> Self {
        Self {
            activity: Arc::clone(&self.activity),
            input: Arc::clone(&self.input),
            destination: Arc::clone(&self.destination),
            executor: Arc::clone(&self.executor),
            verifier: Arc::clone(&self.verifier),
            publisher: Arc::clone(&self.publisher),
            staging: Arc::clone(&self.staging),
            emitter: Arc::clone(&self.emitter),
            opener: Arc::clone(&self.opener),
        }
    }
}

struct TerminalJob {
    job_id: String,
    state: DownloadState,
    error_code: Option<DownloadErrorCode>,
    completed_parent: Option<PathBuf>,
    display_name: Option<String>,
}

#[derive(Default)]
struct JobRegistryState {
    active: HashMap<String, CancellationToken>,
    terminal: HashMap<String, TerminalJob>,
    closed: bool,
}

#[derive(Default)]
struct SharedJobRegistry {
    state: Mutex<JobRegistryState>,
    idle: Condvar,
}

pub struct DownloadJobManager {
    services: DownloadJobServices,
    shared: Arc<SharedJobRegistry>,
    start_guard: Mutex<()>,
    owner: DownloadOwner,
    refresher: Option<Arc<dyn super::bilibili_download::BilibiliDownloadRefresher>>,
}

impl DownloadJobManager {
    pub fn new(services: DownloadJobServices) -> Arc<Self> {
        Self::with_owner(services, DownloadOwner::Youtube, None)
    }
    pub fn for_bilibili(
        services: DownloadJobServices,
        refresher: Arc<dyn super::bilibili_download::BilibiliDownloadRefresher>,
    ) -> Arc<Self> {
        Self::with_owner(services, DownloadOwner::Bilibili, Some(refresher))
    }
    fn with_owner(
        mut services: DownloadJobServices,
        owner: DownloadOwner,
        refresher: Option<Arc<dyn super::bilibili_download::BilibiliDownloadRefresher>>,
    ) -> Arc<Self> {
        services.emitter = Arc::new(TrackingEmitter {
            inner: Arc::clone(&services.emitter),
            activity: Arc::clone(&services.activity),
            owner,
        });
        Arc::new(Self {
            services,
            shared: Arc::new(SharedJobRegistry::default()),
            start_guard: Mutex::new(()),
            owner,
            refresher,
        })
    }

    pub fn start_download(
        &self,
        analysis_id: &str,
        destination_id: &str,
        intent: SelectionIntent,
    ) -> Result<StartedDownload, DownloadErrorCode> {
        if !valid_opaque_id(analysis_id) || !valid_opaque_id(destination_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let _start = self
            .start_guard
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| DownloadErrorCode::Unknown)?;
            if state.closed {
                return Err(DownloadErrorCode::DownloadInProgress);
            }
        }

        let input = self.services.input.resolve_download(analysis_id, intent)?;
        let consistent = match (self.owner, input.bilibili.as_ref()) {
            (DownloadOwner::Youtube, None) => {
                input.source.platform() == crate::domain::media::Platform::Youtube
                    && input.media.platform == crate::domain::media::Platform::Youtube
            }
            (DownloadOwner::Bilibili, Some(snapshot)) => {
                input.source.platform() == crate::domain::media::Platform::Bilibili
                    && input.source.url() == snapshot.source().url()
                    && &input.media == snapshot.media()
                    && &input.plan == snapshot.plan()
            }
            _ => false,
        };
        if !consistent {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let destination = self
            .services
            .destination
            .resolve_download_destination(destination_id)?;
        let activity = self
            .services
            .activity
            .reserve(self.owner, &input.media.title)?;
        let job_id = activity.job_id();
        let cancellation = CancellationToken::new();
        {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| DownloadErrorCode::Unknown)?;
            state.active.insert(job_id.clone(), cancellation.clone());
        }
        if let Err(error) = emit_simple(
            self.services.emitter.as_ref(),
            &job_id,
            DownloadState::Preparing,
        ) {
            self.clear_failed_start(&job_id);
            return Err(error);
        }

        let worker_job_id = job_id.clone();
        let shared = Arc::clone(&self.shared);
        let services = self.services.clone();
        let refresher = self.refresher.clone();
        let spawn = thread::Builder::new()
            .name(format!("media-download-{job_id}"))
            .spawn(move || {
                run_job(
                    services,
                    shared,
                    worker_job_id,
                    cancellation,
                    input,
                    destination,
                    activity,
                    refresher,
                );
            });
        if spawn.is_err() {
            self.clear_failed_start(&job_id);
            return Err(DownloadErrorCode::ProcessFailed);
        }
        Ok(StartedDownload { job_id })
    }

    pub fn cancel_download(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
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
            terminal.job_id == job_id && terminal.state == DownloadState::Canceled
        }) {
            return Ok(());
        }
        Err(DownloadErrorCode::InvalidRequest)
    }

    pub fn download_result(&self, job_id: &str) -> Result<DownloadResult, DownloadErrorCode> {
        if !valid_opaque_id(job_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        let terminal = state
            .terminal
            .get(job_id)
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        Ok(DownloadResult {
            job_id: terminal.job_id.clone(),
            state: terminal.state,
            error_code: terminal.error_code,
            display_name: terminal.display_name.clone(),
        })
    }

    pub fn open_completed_location(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        if !valid_opaque_id(job_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let parent = {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| DownloadErrorCode::Unknown)?;
            state
                .terminal
                .get(job_id)
                .filter(|terminal| terminal.state == DownloadState::Completed)
                .and_then(|terminal| terminal.completed_parent.clone())
                .ok_or(DownloadErrorCode::InvalidRequest)?
        };
        self.services.opener.open(&parent)
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

impl Drop for DownloadJobManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_job(
    services: DownloadJobServices,
    shared: Arc<SharedJobRegistry>,
    job_id: String,
    cancellation: CancellationToken,
    input: ResolvedDownloadInput,
    destination: PathBuf,
    activity: DownloadLease,
    refresher: Option<Arc<dyn super::bilibili_download::BilibiliDownloadRefresher>>,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        activity.wait_turn(&cancellation)?;
        let bilibili_source = if let Some(refresher) = &refresher {
            if cancellation.is_canceled() {
                return Err(DownloadErrorCode::ProcessFailed);
            }
            let refreshed = refresher.refresh(
                input
                    .bilibili
                    .as_ref()
                    .ok_or(DownloadErrorCode::InvalidRequest)?,
                cancellation.clone(),
            );
            if cancellation.is_canceled() {
                return Err(DownloadErrorCode::ProcessFailed);
            }
            Some(refreshed?)
        } else {
            None
        };
        execute_job(
            &services,
            &shared,
            &job_id,
            cancellation.clone(),
            input,
            &destination,
            bilibili_source,
        )
    }))
    .unwrap_or(Err(DownloadErrorCode::Unknown));
    let _ = services.staging.cleanup(&job_id);

    let outcome = match result {
        Ok(published) => WorkerOutcome::Completed(published),
        Err(error) if cancellation.is_canceled() && error == DownloadErrorCode::ProcessFailed => {
            WorkerOutcome::Canceled
        }
        Err(error) => WorkerOutcome::Failed(error),
    };
    let (state_value, error, parent, display_name) = match &outcome {
        WorkerOutcome::Completed(published) => (
            DownloadState::Completed,
            None,
            published.final_path.parent().map(Path::to_owned),
            Some(published.display_name.clone()),
        ),
        WorkerOutcome::Failed(error) => (DownloadState::Failed, Some(*error), None, None),
        WorkerOutcome::Canceled => (DownloadState::Canceled, None, None, None),
    };

    let should_finish = if let Ok(mut state) = shared.state.lock() {
        let matches = state.active.contains_key(&job_id);
        if matches {
            state.terminal.insert(
                job_id.clone(),
                TerminalJob {
                    job_id: job_id.clone(),
                    state: state_value,
                    error_code: error,
                    completed_parent: parent,
                    display_name,
                },
            );
            trim_history(&mut state.terminal);
        }
        matches
    } else {
        false
    };
    if should_finish {
        services
            .activity
            .finish_details(&job_id, error, state_display_name(&shared, &job_id));
        let _ = emit_simple(services.emitter.as_ref(), &job_id, state_value);
    }
    drop(activity);
    if let Ok(mut state) = shared.state.lock() {
        if state.active.remove(&job_id).is_some() {
            shared.idle.notify_all();
        }
    }
}

fn execute_job(
    services: &DownloadJobServices,
    shared: &Arc<SharedJobRegistry>,
    job_id: &str,
    cancellation: CancellationToken,
    input: ResolvedDownloadInput,
    destination: &Path,
    bilibili_source: Option<super::bilibili_download::VerifiedBilibiliDownloadSource>,
) -> Result<PublishedOutput, DownloadErrorCode> {
    let staging_dir = services.staging.create(job_id)?;
    if cancellation.is_canceled() {
        return Err(DownloadErrorCode::ProcessFailed);
    }
    emit_simple(
        services.emitter.as_ref(),
        job_id,
        DownloadState::Downloading,
    )?;
    let progress = JobProgressForwarder::new(
        job_id,
        Arc::clone(shared),
        Arc::clone(&services.emitter),
        cancellation.clone(),
    );
    let ResolvedDownloadInput {
        source,
        media,
        plan,
        bilibili,
    } = input;
    let full_duration = bilibili
        .as_ref()
        .map(|snapshot| snapshot.duration_seconds());
    drop(bilibili);
    let download = services.executor.download(
        DownloadExecutionRequest {
            job_id: job_id.to_owned(),
            source,
            playlist_item: None,
            instagram_selector: None,
            x_source: None,
            bilibili_source,
            plan: plan.clone(),
            video_source: media
                .formats
                .iter()
                .find(|row| Some(&row.id) == plan.video_format_id.as_ref())
                .cloned(),
            audio_source: media
                .formats
                .iter()
                .find(|row| Some(&row.id) == plan.audio_format_id.as_ref())
                .cloned(),
            staging_dir,
        },
        cancellation.clone(),
        &progress,
    );
    if let Some(error) = progress.error() {
        return Err(error);
    }
    let staged = download?;
    if cancellation.is_canceled() {
        return Err(DownloadErrorCode::ProcessFailed);
    }
    progress.ensure_processing()?;
    emit_simple(services.emitter.as_ref(), job_id, DownloadState::Verifying)?;
    let verified = services
        .verifier
        .verify(staged, &plan, cancellation.clone())?;
    if cancellation.is_canceled() {
        return Err(DownloadErrorCode::ProcessFailed);
    }
    if let Some(full) = full_duration {
        let duration = verified.duration_ms as f64 / 1000.0;
        let valid = if let Some(clip) = plan.clip {
            verified.duration_ms > 0
                && verified.duration_ms
                    <= clip
                        .end_ms
                        .saturating_sub(clip.start_ms)
                        .saturating_add(10_000)
        } else {
            (duration - full).abs() <= 2.0_f64.max(full * 0.01)
        };
        if !valid {
            return Err(DownloadErrorCode::OutputInvalid);
        }
    }
    services.publisher.publish(
        verified,
        destination,
        &media.title,
        &media.media_id,
        &cancellation,
    )
}

enum WorkerOutcome {
    Completed(PublishedOutput),
    Failed(DownloadErrorCode),
    Canceled,
}

struct ForwardState {
    state: DownloadState,
    downloaded_bytes: Option<u64>,
    error: Option<DownloadErrorCode>,
}

struct JobProgressForwarder {
    job_id: String,
    shared: Arc<SharedJobRegistry>,
    emitter: Arc<dyn DownloadEventEmitter>,
    cancellation: CancellationToken,
    state: Mutex<ForwardState>,
}

impl JobProgressForwarder {
    fn new(
        job_id: &str,
        shared: Arc<SharedJobRegistry>,
        emitter: Arc<dyn DownloadEventEmitter>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            job_id: job_id.to_owned(),
            shared,
            emitter,
            cancellation,
            state: Mutex::new(ForwardState {
                state: DownloadState::Downloading,
                downloaded_bytes: None,
                error: None,
            }),
        }
    }

    fn error(&self) -> Option<DownloadErrorCode> {
        self.state.lock().ok().and_then(|state| state.error)
    }

    fn ensure_processing(&self) -> Result<(), DownloadErrorCode> {
        let mut state = self.state.lock().map_err(|_| DownloadErrorCode::Unknown)?;
        if let Some(error) = state.error {
            return Err(error);
        }
        if state.state == DownloadState::Processing {
            return Ok(());
        }
        let progress = DownloadProgress::new(
            &self.job_id,
            DownloadState::Processing,
            DownloadProgressMetrics::default(),
            DownloadMessageKey::Processing,
        )?;
        self.emitter.emit(progress)?;
        state.state = DownloadState::Processing;
        Ok(())
    }

    fn is_current(&self) -> bool {
        if self.cancellation.is_canceled() {
            return false;
        }
        self.shared
            .state
            .lock()
            .ok()
            .is_some_and(|state| state.active.contains_key(&self.job_id))
    }

    fn reject(&self, state: &mut ForwardState, error: DownloadErrorCode) {
        state.error.get_or_insert(error);
        self.cancellation.cancel();
    }
}

impl DownloadProgressSink for JobProgressForwarder {
    fn on_progress(&self, progress: DownloadProgress) {
        if !self.is_current() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            self.cancellation.cancel();
            return;
        };
        if state.error.is_some()
            || progress.job_id != self.job_id
            || !matches!(
                progress.state,
                DownloadState::Downloading | DownloadState::Processing
            )
            || (state.state == DownloadState::Processing
                && progress.state != DownloadState::Processing)
            || progress
                .downloaded_bytes
                .zip(state.downloaded_bytes)
                .is_some_and(|(next, previous)| next < previous)
        {
            self.reject(&mut state, DownloadErrorCode::OutputInvalid);
            return;
        }
        if self.emitter.emit(progress.clone()).is_err() {
            self.reject(&mut state, DownloadErrorCode::Unknown);
            return;
        }
        state.state = progress.state;
        if progress.downloaded_bytes.is_some() {
            state.downloaded_bytes = progress.downloaded_bytes;
        }
    }
}

fn emit_simple(
    emitter: &dyn DownloadEventEmitter,
    job_id: &str,
    state: DownloadState,
) -> Result<(), DownloadErrorCode> {
    let message_key = match state {
        DownloadState::Preparing => DownloadMessageKey::Preparing,
        DownloadState::Downloading => DownloadMessageKey::Downloading,
        DownloadState::Processing => DownloadMessageKey::Processing,
        DownloadState::Verifying => DownloadMessageKey::Verifying,
        DownloadState::Completed => DownloadMessageKey::Completed,
        DownloadState::Failed => DownloadMessageKey::Failed,
        DownloadState::Canceled => DownloadMessageKey::Canceled,
    };
    emitter.emit(DownloadProgress::new(
        job_id,
        state,
        DownloadProgressMetrics::default(),
        message_key,
    )?)
}
