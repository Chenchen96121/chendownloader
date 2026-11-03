#[path = "support/output_policy.rs"]
mod policy_fixture;
#[path = "support/publication.rs"]
mod publication_fixture;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use local_media_downloader_lib::{
    application::{
        download_activity::DownloadActivityGate,
        download_executor::{
            DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink, StagedDownload,
        },
        download_job::{
            CompletedLocationOpener, DownloadDestinationResolver, DownloadEventEmitter,
            DownloadInputResolver, DownloadJobManager, DownloadJobServices, JobStaging,
            ResolvedDownloadInput,
        },
        media_url::validate_media_url,
        output_publisher::{OutputPublisher, PublishedOutput},
        output_verifier::{OutputVerifier, VerifiedOutput},
        process::CancellationToken,
    },
    domain::{
        download::{
            DownloadErrorCode, DownloadMessageKey, DownloadProgress, DownloadProgressMetrics,
            DownloadState,
        },
        format_selection::{
            ClipRange, SelectionIntent, SelectionMode, SelectionOperation, SelectionPlan,
            SelectionWarning,
        },
        media::{MediaFormat, MediaInfo, Platform, StreamKind},
        tool_provisioning::{ToolProvisioningErrorCode, ToolProvisioningStatus},
    },
};

fn intent() -> SelectionIntent {
    SelectionIntent {
        compatibility_mode: false,
        mode: SelectionMode::BestAvailable,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}

fn clip_intent() -> SelectionIntent {
    SelectionIntent {
        clip: Some(ClipRange {
            start_ms: 5_000,
            end_ms: 10_000,
        }),
        ..intent()
    }
}

fn plan() -> SelectionPlan {
    SelectionPlan {
        compatibility_mode: false,
        container_selection:
            local_media_downloader_lib::domain::output_policy::ContainerSelection::Automatic,
        video_processing: Some(
            local_media_downloader_lib::domain::output_policy::StreamProcessing::Copy,
        ),
        output_video_codec: Some("h264".into()),
        mode: SelectionMode::BestAvailable,
        source_format_ids: vec!["18".to_owned()],
        video_format_id: Some("18".to_owned()),
        audio_format_id: Some("18".to_owned()),
        operation: SelectionOperation::Keep,
        output_container: Some("mp4".to_owned()),
        output_audio_codec: Some(
            local_media_downloader_lib::domain::output_policy::TargetAudioCodec::Aac,
        ),
        audio_processing: Some(
            local_media_downloader_lib::domain::output_policy::AudioProcessing::Copy,
        ),
        clip: None,
        estimated_size_bytes: Some(8),
        warnings: vec![SelectionWarning::ContainerUnverified],
    }
}

fn media() -> MediaInfo {
    MediaInfo {
        platform: Platform::Youtube,
        media_id: "demoYT00001".to_owned(),
        title: "示例 Title".to_owned(),
        creator: None,
        duration_seconds: Some(60.0),
        formats: vec![MediaFormat {
            id: "18".to_owned(),
            container: "mp4".to_owned(),
            kind: StreamKind::Combined,
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            video_bitrate_kbps: Some(1_000.0),
            audio_bitrate_kbps: Some(128.0),
            audio_sample_rate_hz: Some(48_000),
            audio_channels: Some(2),
            video_codec: Some("avc1".to_owned()),
            audio_codec: Some("aac".to_owned()),
            dynamic_range: Some("SDR".to_owned()),
            file_size: None,
        }],
    }
}

struct FakeAnalysis {
    error: Option<DownloadErrorCode>,
}

impl DownloadInputResolver for FakeAnalysis {
    fn resolve_download(
        &self,
        _analysis_id: &str,
        intent: SelectionIntent,
    ) -> Result<ResolvedDownloadInput, DownloadErrorCode> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let mut selected_plan = plan();
        if intent.compatibility_mode {
            selected_plan = local_media_downloader_lib::domain::format_selection::select_formats(
                &media(),
                &intent,
            )
            .map_err(|_| DownloadErrorCode::FormatUnavailable)?;
        }
        selected_plan.clip = intent.clip;
        Ok(ResolvedDownloadInput {
            bilibili: None,
            source: validate_media_url("https://youtube.com/watch?v=demoYT00001").unwrap(),
            media: media(),
            plan: selected_plan,
        })
    }
}

struct FakeDestination {
    path: PathBuf,
    error: Option<DownloadErrorCode>,
}

impl DownloadDestinationResolver for FakeDestination {
    fn resolve_download_destination(
        &self,
        _destination_id: &str,
    ) -> Result<PathBuf, DownloadErrorCode> {
        self.error.map_or_else(|| Ok(self.path.clone()), Err)
    }
}

#[derive(Clone, Copy)]
enum FailureAt {
    None,
    Staging,
    Executor(DownloadErrorCode),
    Verifier(DownloadErrorCode),
    Publisher(DownloadErrorCode),
}

struct FakeStaging {
    root: PathBuf,
    fail: bool,
    cleaned: Mutex<Vec<String>>,
}

impl JobStaging for FakeStaging {
    fn create(&self, job_id: &str) -> Result<PathBuf, DownloadErrorCode> {
        if self.fail {
            return Err(DownloadErrorCode::DiskFull);
        }
        let path = self.root.join(job_id);
        fs::create_dir(&path).map_err(|_| DownloadErrorCode::PublicationFailed)?;
        Ok(path)
    }

    fn cleanup(&self, job_id: &str) -> Result<(), DownloadErrorCode> {
        self.cleaned.lock().unwrap().push(job_id.to_owned());
        let path = self.root.join(job_id);
        if path.exists() {
            fs::remove_dir_all(path).map_err(|_| DownloadErrorCode::PublicationFailed)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum CancellationWindow {
    Transfer,
    AfterMaterialization,
    DuringCut,
}

#[derive(Clone, Copy)]
enum ExecutorBehavior {
    Complete(Option<DownloadErrorCode>),
    CompleteClipProgress,
    WaitForCancellation(CancellationWindow),
}

struct FakeExecutor {
    behavior: ExecutorBehavior,
    entered: Arc<(Mutex<bool>, Condvar)>,
}

impl DownloadExecutor for FakeExecutor {
    fn download(
        &self,
        request: DownloadExecutionRequest,
        cancellation: CancellationToken,
        progress: &dyn DownloadProgressSink,
    ) -> Result<StagedDownload, DownloadErrorCode> {
        if matches!(
            self.behavior,
            ExecutorBehavior::CompleteClipProgress
                | ExecutorBehavior::WaitForCancellation(CancellationWindow::DuringCut)
        ) {
            progress.on_progress(
                DownloadProgress::new(
                    &request.job_id,
                    DownloadState::Downloading,
                    DownloadProgressMetrics {
                        downloaded_bytes: Some(8),
                        total_bytes: Some(8),
                        percent: Some(100.0),
                        ..DownloadProgressMetrics::default()
                    },
                    DownloadMessageKey::Downloading,
                )
                .unwrap(),
            );
            progress.on_progress(
                DownloadProgress::new(
                    &request.job_id,
                    DownloadState::Processing,
                    DownloadProgressMetrics::default(),
                    DownloadMessageKey::Processing,
                )
                .unwrap(),
            );
        }
        if matches!(
            self.behavior,
            ExecutorBehavior::WaitForCancellation(
                CancellationWindow::AfterMaterialization | CancellationWindow::DuringCut
            )
        ) {
            let source = request.staging_dir.join("source");
            fs::create_dir(&source).map_err(|_| DownloadErrorCode::DiskFull)?;
            fs::write(source.join("source.mp4"), b"complete source")
                .map_err(|_| DownloadErrorCode::DiskFull)?;
        }
        {
            let (lock, ready) = &*self.entered;
            *lock.lock().unwrap() = true;
            ready.notify_all();
        }
        if matches!(self.behavior, ExecutorBehavior::WaitForCancellation(_)) {
            while !cancellation.is_canceled() {
                thread::sleep(Duration::from_millis(2));
            }
            progress.on_progress(
                DownloadProgress::new(
                    &request.job_id,
                    DownloadState::Downloading,
                    DownloadProgressMetrics {
                        downloaded_bytes: Some(1),
                        ..DownloadProgressMetrics::default()
                    },
                    DownloadMessageKey::Downloading,
                )
                .unwrap(),
            );
            return Err(DownloadErrorCode::ProcessFailed);
        }
        if let ExecutorBehavior::Complete(Some(error)) = self.behavior {
            return Err(error);
        }
        fs::write(request.staging_dir.join("stage.mp4"), b"verified")
            .map_err(|_| DownloadErrorCode::DiskFull)?;
        Ok(StagedDownload {
            job_id: request.job_id,
            staging_dir: request.staging_dir,
            expected_operation: request.plan.operation,
            expected_container: request.plan.output_container.clone(),
            requested_clip: request.plan.clip,
            evidence: policy_fixture::evidence(&request.plan),
        })
    }
}

struct FakeVerifier {
    error: Option<DownloadErrorCode>,
    calls: Arc<Mutex<usize>>,
}

impl OutputVerifier for FakeVerifier {
    fn verify(
        &self,
        staged: StagedDownload,
        _plan: &SelectionPlan,
        _cancellation: CancellationToken,
    ) -> Result<VerifiedOutput, DownloadErrorCode> {
        *self.calls.lock().unwrap() += 1;
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(VerifiedOutput {
            path: staged.staging_dir.join("stage.mp4"),
            container: "mp4".to_owned(),
            has_video: true,
            has_audio: true,
            duration_ms: 1_000,
            byte_length: 8,
        })
    }
}

struct FakePublisher {
    error: Option<DownloadErrorCode>,
    calls: Arc<Mutex<usize>>,
}

impl OutputPublisher for FakePublisher {
    fn publish(
        &self,
        _verified: VerifiedOutput,
        destination: &Path,
        _safe_title: &str,
        _media_id: &str,
        _cancellation: &CancellationToken,
    ) -> Result<PublishedOutput, DownloadErrorCode> {
        *self.calls.lock().unwrap() += 1;
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(PublishedOutput {
            final_path: destination.join("示例 Title [demoYT00001].mp4"),
            display_name: "示例 Title [demoYT00001].mp4".to_owned(),
        })
    }
}

#[derive(Default)]
struct RecordingEmitter {
    events: Mutex<Vec<DownloadProgress>>,
    changed: Condvar,
}

impl RecordingEmitter {
    fn states(&self) -> Vec<DownloadState> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.state)
            .collect()
    }
}

impl DownloadEventEmitter for RecordingEmitter {
    fn emit(&self, progress: DownloadProgress) -> Result<(), DownloadErrorCode> {
        self.events.lock().unwrap().push(progress);
        self.changed.notify_all();
        Ok(())
    }
}

#[derive(Default)]
struct RecordingOpener(Mutex<Vec<PathBuf>>);

impl CompletedLocationOpener for RecordingOpener {
    fn open(&self, path: &Path) -> Result<(), DownloadErrorCode> {
        self.0.lock().unwrap().push(path.to_owned());
        Ok(())
    }
}

struct Harness {
    _root: tempfile::TempDir,
    destination: PathBuf,
    manager: Arc<DownloadJobManager>,
    staging: Arc<FakeStaging>,
    emitter: Arc<RecordingEmitter>,
    opener: Arc<RecordingOpener>,
    entered: Arc<(Mutex<bool>, Condvar)>,
    verifier_calls: Arc<Mutex<usize>>,
    publisher_calls: Arc<Mutex<usize>>,
}

impl Harness {
    fn new(failure: FailureAt, wait_for_cancellation: bool) -> Self {
        Self::with_resolver_errors(failure, wait_for_cancellation, None, None)
    }

    fn with_resolver_errors(
        failure: FailureAt,
        wait_for_cancellation: bool,
        analysis_error: Option<DownloadErrorCode>,
        destination_error: Option<DownloadErrorCode>,
    ) -> Self {
        let behavior = if wait_for_cancellation {
            ExecutorBehavior::WaitForCancellation(CancellationWindow::Transfer)
        } else {
            ExecutorBehavior::Complete(match failure {
                FailureAt::Executor(error) => Some(error),
                _ => None,
            })
        };
        Self::build(failure, behavior, analysis_error, destination_error)
    }

    fn with_behavior(behavior: ExecutorBehavior) -> Self {
        Self::build(FailureAt::None, behavior, None, None)
    }

    fn build(
        failure: FailureAt,
        behavior: ExecutorBehavior,
        analysis_error: Option<DownloadErrorCode>,
        destination_error: Option<DownloadErrorCode>,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let staging_root = root.path().join("jobs");
        fs::create_dir(&destination).unwrap();
        fs::create_dir(&staging_root).unwrap();
        let staging = Arc::new(FakeStaging {
            root: staging_root,
            fail: matches!(failure, FailureAt::Staging),
            cleaned: Mutex::new(Vec::new()),
        });
        let emitter = Arc::new(RecordingEmitter::default());
        let opener = Arc::new(RecordingOpener::default());
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let verifier_calls = Arc::new(Mutex::new(0));
        let publisher_calls = Arc::new(Mutex::new(0));
        let manager = DownloadJobManager::new(DownloadJobServices {
            activity: Arc::new(DownloadActivityGate::new()),
            input: Arc::new(FakeAnalysis {
                error: analysis_error,
            }),
            destination: Arc::new(FakeDestination {
                path: destination.clone(),
                error: destination_error,
            }),
            executor: Arc::new(FakeExecutor {
                behavior,
                entered: Arc::clone(&entered),
            }),
            verifier: Arc::new(FakeVerifier {
                error: match failure {
                    FailureAt::Verifier(error) => Some(error),
                    _ => None,
                },
                calls: Arc::clone(&verifier_calls),
            }),
            publisher: Arc::new(FakePublisher {
                error: match failure {
                    FailureAt::Publisher(error) => Some(error),
                    _ => None,
                },
                calls: Arc::clone(&publisher_calls),
            }),
            staging: staging.clone(),
            emitter: emitter.clone(),
            opener: opener.clone(),
        });
        Self {
            _root: root,
            destination,
            manager,
            staging,
            emitter,
            opener,
            entered,
            verifier_calls,
            publisher_calls,
        }
    }

    fn start(&self) -> Result<String, DownloadErrorCode> {
        self.manager
            .start_download("1", "2", intent())
            .map(|started| started.job_id)
    }

    fn wait_entered(&self) {
        let (lock, ready) = &*self.entered;
        let guard = lock.lock().unwrap();
        let _ = ready
            .wait_timeout_while(guard, Duration::from_secs(2), |entered| !*entered)
            .unwrap();
    }
}

#[test]
fn active_job_status_tracks_the_synchronized_registry() {
    let harness = Harness::new(FailureAt::None, true);
    assert!(!harness.manager.has_active_job());

    let job_id = harness.start().unwrap();
    harness.wait_entered();
    assert!(harness.manager.has_active_job());

    harness.manager.cancel_download(&job_id).unwrap();
    assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));
    assert!(!harness.manager.has_active_job());
}

#[test]
fn multiple_youtube_posts_can_download_and_cancel_independently() {
    let harness = Harness::new(FailureAt::None, true);
    let first = harness.start().unwrap();
    harness.wait_entered();
    let second = harness
        .start()
        .expect("a second post must be accepted while the first downloads");
    assert_ne!(first, second);
    harness.manager.cancel_download(&first).unwrap();
    assert!(harness.manager.has_active_job());
    harness.manager.cancel_download(&second).unwrap();
    assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));
    assert_eq!(
        harness.manager.download_result(&first).unwrap().state,
        DownloadState::Canceled
    );
    assert_eq!(
        harness.manager.download_result(&second).unwrap().state,
        DownloadState::Canceled
    );
}

#[test]
fn download_start_is_gated_by_the_closed_provisioning_status() {
    assert_eq!(ToolProvisioningStatus::ready().download_readiness(), Ok(()));
    for status in [
        ToolProvisioningStatus::preparing(),
        ToolProvisioningStatus::repairing(),
        ToolProvisioningStatus::failed(ToolProvisioningErrorCode::PackageMissing),
    ] {
        assert_eq!(
            status.download_readiness(),
            Err(DownloadErrorCode::ToolMissing)
        );
    }
    for code in [
        ToolProvisioningErrorCode::PackageInvalid,
        ToolProvisioningErrorCode::ToolProbeFailed,
        ToolProvisioningErrorCode::ActivationFailed,
    ] {
        assert_eq!(
            ToolProvisioningStatus::failed(code).download_readiness(),
            Err(DownloadErrorCode::ToolIntegrityFailed)
        );
    }
}

#[test]
fn runtime_rejects_download_start_before_local_components_are_ready() {
    use local_media_downloader_lib::runtime::AppState;

    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(root.path().into());

    assert_eq!(
        state.start_download("invalid analysis", "invalid destination", intent()),
        Err(DownloadErrorCode::ToolMissing)
    );
}

#[test]
fn success_emits_the_closed_sequence_retains_only_safe_completion_and_opens_parent() {
    let harness = Harness::new(FailureAt::None, false);
    let started = harness.manager.start_download("1", "2", intent()).unwrap();
    assert_eq!(
        serde_json::to_value(&started).unwrap(),
        serde_json::json!({ "jobId": "1" })
    );
    assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));
    assert_eq!(
        harness.emitter.states(),
        [
            DownloadState::Preparing,
            DownloadState::Downloading,
            DownloadState::Processing,
            DownloadState::Verifying,
            DownloadState::Completed,
        ]
    );
    assert_eq!(
        serde_json::to_value(harness.manager.download_result("1").unwrap()).unwrap(),
        serde_json::json!({
            "jobId": "1",
            "state": "completed",
            "errorCode": null,
            "displayName": "示例 Title [demoYT00001].mp4"
        })
    );
    assert_eq!(*harness.staging.cleaned.lock().unwrap(), ["1"]);
    harness.manager.open_completed_location("1").unwrap();
    assert_eq!(
        harness.opener.0.lock().unwrap().as_slice(),
        std::slice::from_ref(&harness.destination)
    );
    assert_eq!(
        harness.manager.open_completed_location("2"),
        Err(DownloadErrorCode::InvalidRequest)
    );
}

#[test]
fn local_clip_job_emits_preparing_downloading_processing_verifying_completed_once() {
    let harness = Harness::with_behavior(ExecutorBehavior::CompleteClipProgress);
    let job_id = harness
        .manager
        .start_download("1", "2", clip_intent())
        .unwrap()
        .job_id;
    assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));

    let states = harness.emitter.states();
    let collapsed = states
        .iter()
        .copied()
        .fold(Vec::new(), |mut values, state| {
            if values.last() != Some(&state) {
                values.push(state);
            }
            values
        });
    assert_eq!(
        collapsed,
        [
            DownloadState::Preparing,
            DownloadState::Downloading,
            DownloadState::Processing,
            DownloadState::Verifying,
            DownloadState::Completed,
        ]
    );
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == DownloadState::Processing)
            .count(),
        1
    );
    assert_eq!(
        states
            .iter()
            .filter(|state| matches!(
                state,
                DownloadState::Completed | DownloadState::Failed | DownloadState::Canceled
            ))
            .count(),
        1
    );
    let result = serde_json::to_value(harness.manager.download_result(&job_id).unwrap()).unwrap();
    assert_eq!(
        result,
        serde_json::json!({
            "jobId": "1",
            "state": "completed",
            "errorCode": null,
            "displayName": "示例 Title [demoYT00001].mp4"
        })
    );
    let serialized = result.to_string().to_ascii_lowercase();
    assert!(!serialized.contains("path"));
    assert!(!serialized.contains("url"));
}

#[test]
fn cancel_after_materialization_and_during_cut_publishes_nothing_and_cleans_staging() {
    for (window, compatible) in [
        (CancellationWindow::AfterMaterialization, false),
        (CancellationWindow::DuringCut, false),
        (CancellationWindow::DuringCut, true),
    ] {
        let harness = Harness::with_behavior(ExecutorBehavior::WaitForCancellation(window));
        let job_id = harness
            .manager
            .start_download(
                "1",
                "2",
                SelectionIntent {
                    compatibility_mode: compatible,
                    ..clip_intent()
                },
            )
            .unwrap()
            .job_id;
        harness.wait_entered();
        assert!(harness.staging.root.join(&job_id).join("source").is_dir());

        harness.manager.cancel_download(&job_id).unwrap();
        assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));

        assert_eq!(
            harness.manager.download_result(&job_id).unwrap().state,
            DownloadState::Canceled
        );
        assert_eq!(*harness.verifier_calls.lock().unwrap(), 0);
        assert_eq!(*harness.publisher_calls.lock().unwrap(), 0);
        assert_eq!(
            harness.staging.cleaned.lock().unwrap().as_slice(),
            std::slice::from_ref(&job_id)
        );
        assert!(!harness.staging.root.join(&job_id).exists());
        assert_eq!(
            harness.manager.open_completed_location(&job_id),
            Err(DownloadErrorCode::InvalidRequest)
        );
        assert!(harness.opener.0.lock().unwrap().is_empty());
        assert_eq!(
            harness
                .emitter
                .states()
                .iter()
                .filter(|state| matches!(
                    state,
                    DownloadState::Completed | DownloadState::Failed | DownloadState::Canceled
                ))
                .count(),
            1
        );
    }
}

#[test]
fn cancel_during_publication_discards_the_copy_and_releases_only_its_job() {
    use local_media_downloader_lib::infrastructure::output::{
        FilesystemJobStaging, FilesystemOutputPublisher,
    };
    use std::sync::{atomic::AtomicBool, mpsc};

    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("out");
    fs::create_dir(&destination).unwrap();
    let staging_root = root.path().join("jobs");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let activity = Arc::new(DownloadActivityGate::new());
    activity.set_limit(1).unwrap();
    let manager = DownloadJobManager::new(DownloadJobServices {
        activity: activity.clone(),
        input: Arc::new(FakeAnalysis { error: None }),
        destination: Arc::new(FakeDestination {
            path: destination.clone(),
            error: None,
        }),
        executor: Arc::new(FakeExecutor {
            behavior: ExecutorBehavior::Complete(None),
            entered: Arc::new((Mutex::new(false), Condvar::new())),
        }),
        verifier: Arc::new(FakeVerifier {
            error: None,
            calls: Arc::new(Mutex::new(0)),
        }),
        publisher: Arc::new(FilesystemOutputPublisher::with_operations(
            publication_fixture::PausedCrossVolumeCopy {
                entered: entered_tx,
                resume: Mutex::new(resume_rx),
                paused: AtomicBool::new(false),
            },
        )),
        staging: Arc::new(FilesystemJobStaging::new(staging_root.clone())),
        emitter: Arc::new(RecordingEmitter::default()),
        opener: Arc::new(RecordingOpener::default()),
    });
    let first = manager.start_download("1", "2", intent()).unwrap().job_id;
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let second = manager.start_download("1", "2", intent()).unwrap().job_id;
    manager.cancel_download(&first).unwrap();
    resume_tx.send(()).unwrap();
    assert!(manager.wait_for_idle(Duration::from_secs(3)));

    assert_eq!(
        manager.download_result(&first).unwrap().state,
        DownloadState::Canceled
    );
    assert_eq!(
        manager.download_result(&second).unwrap().state,
        DownloadState::Completed
    );
    let outputs = fs::read_dir(&destination)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(outputs.len(), 1, "only the uncanceled job is published");
    assert_eq!(fs::read(outputs[0].path()).unwrap(), b"verified");
    assert!(!staging_root.join(first).exists());
    assert!(!staging_root.join(second).exists());
    assert!(activity.active_owner().is_none());
}

#[test]
fn every_worker_failure_emits_one_failed_terminal_event_and_cleans_staging() {
    for failure in [
        FailureAt::Staging,
        FailureAt::Executor(DownloadErrorCode::ToolIntegrityFailed),
        FailureAt::Verifier(DownloadErrorCode::OutputInvalid),
        FailureAt::Publisher(DownloadErrorCode::PublicationFailed),
    ] {
        let harness = Harness::new(failure, false);
        let id = harness.start().unwrap();
        assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));
        let states = harness.emitter.states();
        assert_eq!(states.last(), Some(&DownloadState::Failed));
        assert_eq!(
            states
                .iter()
                .filter(|state| matches!(
                    state,
                    DownloadState::Failed | DownloadState::Canceled | DownloadState::Completed
                ))
                .count(),
            1
        );
        let result = harness.manager.download_result(&id).unwrap();
        assert_eq!(result.state, DownloadState::Failed);
        assert!(result.error_code.is_some());
        assert_eq!(*harness.staging.cleaned.lock().unwrap(), [id]);
    }
}

#[test]
fn stale_analysis_destination_and_tool_changes_fail_with_stable_codes() {
    let stale_analysis = Harness::with_resolver_errors(
        FailureAt::None,
        false,
        Some(DownloadErrorCode::AnalysisUnavailable),
        None,
    );
    assert_eq!(
        stale_analysis.start(),
        Err(DownloadErrorCode::AnalysisUnavailable)
    );
    assert!(stale_analysis.emitter.states().is_empty());

    let stale_destination = Harness::with_resolver_errors(
        FailureAt::None,
        false,
        None,
        Some(DownloadErrorCode::DestinationUnavailable),
    );
    assert_eq!(
        stale_destination.start(),
        Err(DownloadErrorCode::DestinationUnavailable)
    );

    let changed_tool = Harness::new(
        FailureAt::Executor(DownloadErrorCode::ToolIntegrityFailed),
        false,
    );
    changed_tool.start().unwrap();
    assert!(changed_tool.manager.wait_for_idle(Duration::from_secs(2)));
    assert_eq!(
        changed_tool.emitter.states().last(),
        Some(&DownloadState::Failed)
    );
}

#[test]
fn matching_cancel_is_idempotent_and_late_progress_is_suppressed() {
    let harness = Harness::new(FailureAt::None, true);
    let job_id = harness.start().unwrap();
    harness.wait_entered();
    assert_eq!(
        harness.manager.cancel_download("2"),
        Err(DownloadErrorCode::InvalidRequest)
    );
    harness.manager.cancel_download(&job_id).unwrap();
    harness.manager.cancel_download(&job_id).unwrap();
    assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));
    let states = harness.emitter.states();
    assert_eq!(states.last(), Some(&DownloadState::Canceled));
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == DownloadState::Canceled)
            .count(),
        1
    );
    assert_eq!(
        harness.manager.download_result(&job_id).unwrap().state,
        DownloadState::Canceled
    );
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == DownloadState::Downloading)
            .count(),
        1
    );
    assert_eq!(
        harness.staging.cleaned.lock().unwrap().as_slice(),
        std::slice::from_ref(&job_id)
    );
    assert_eq!(harness.manager.cancel_download(&job_id), Ok(()));
}

#[test]
fn shutdown_cancels_the_active_worker_and_completed_only_open_is_enforced() {
    let harness = Harness::new(FailureAt::None, true);
    let job_id = harness.start().unwrap();
    harness.wait_entered();
    assert_eq!(
        harness.manager.open_completed_location(&job_id),
        Err(DownloadErrorCode::InvalidRequest)
    );
    assert_eq!(
        harness.manager.download_result(&job_id),
        Err(DownloadErrorCode::InvalidRequest)
    );
    harness.manager.shutdown();
    assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));
    assert_eq!(
        harness.emitter.states().last(),
        Some(&DownloadState::Canceled)
    );
    assert!(harness.opener.0.lock().unwrap().is_empty());
}

#[test]
fn invalid_ids_are_rejected_without_consulting_services() {
    let harness = Harness::new(FailureAt::None, false);
    for invalid in ["", "0", "01", "-1", "unknown", "111111111111111111111"] {
        assert_eq!(
            harness.manager.start_download(invalid, "2", intent()).err(),
            Some(DownloadErrorCode::InvalidRequest)
        );
        assert_eq!(
            harness.manager.start_download("1", invalid, intent()).err(),
            Some(DownloadErrorCode::InvalidRequest)
        );
    }
    assert!(!harness.entered.0.lock().unwrap().to_owned());
}
