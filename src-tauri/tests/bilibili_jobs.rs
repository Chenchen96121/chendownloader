#[path = "support/bilibili.rs"]
mod fixtures;
use local_media_downloader_lib::{
    application::{
        bilibili_analysis::{BilibiliAnalysisService, BilibiliCoverLoader},
        bilibili_download::{
            BilibiliDownloadRefresher, BilibiliDownloadSnapshot, VerifiedBilibiliDownloadSource,
        },
        bilibili_source::{BilibiliPageEvidence, BilibiliRequestContext},
        download_activity::{DownloadActivityGate, DownloadOwner, TaskPhase},
        download_executor::{
            DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink, StagedDownload,
        },
        download_job::{
            CompletedLocationOpener, DownloadDestinationResolver, DownloadEventEmitter,
            DownloadInputResolver, DownloadJobManager, DownloadJobServices, ResolvedDownloadInput,
        },
        output_publisher::{OutputPublisher, PublishedOutput},
        output_verifier::{OutputVerifier, VerifiedOutput},
        process::CancellationToken,
    },
    domain::{
        bilibili::BilibiliCoverPayload,
        download::{
            DownloadErrorCode, DownloadMessageKey, DownloadProgress, DownloadProgressMetrics,
            DownloadState,
        },
        format_selection::{SelectionIntent, SelectionMode, SelectionPlan},
        media::{AnalysisErrorCode, Platform},
        output_policy::OutputContainer,
    },
    infrastructure::output::FilesystemJobStaging,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
struct Covers;
impl BilibiliCoverLoader for Covers {
    fn load(
        &self,
        _: &BilibiliPageEvidence,
        _: &BilibiliRequestContext,
    ) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        Err(AnalysisErrorCode::AnalysisUnavailable)
    }
}
type Service = BilibiliAnalysisService<fixtures::Analyzer, Covers>;
fn intent() -> SelectionIntent {
    SelectionIntent {
        mode: SelectionMode::BestAvailable,
        compatibility_mode: false,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}
struct Input {
    service: Arc<Service>,
    id: String,
    tamper: bool,
}
impl DownloadInputResolver for Input {
    fn resolve_download(
        &self,
        _: &str,
        intent: SelectionIntent,
    ) -> Result<ResolvedDownloadInput, DownloadErrorCode> {
        let snapshot = self
            .service
            .prepare_download(&self.id, intent, Instant::now())?;
        let mut plan = snapshot.plan().clone();
        if self.tamper {
            plan.source_format_ids = vec!["other".into()];
        }
        Ok(ResolvedDownloadInput {
            source: snapshot.source().clone(),
            media: snapshot.media().clone(),
            plan,
            bilibili: Some(snapshot),
        })
    }
}
struct Refresh {
    service: Arc<Service>,
    calls: AtomicUsize,
}
impl BilibiliDownloadRefresher for Refresh {
    fn refresh(
        &self,
        snapshot: &BilibiliDownloadSnapshot,
        cancellation: CancellationToken,
    ) -> Result<VerifiedBilibiliDownloadSource, DownloadErrorCode> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.service.refresh(snapshot, cancellation)
    }
}
struct Destination(PathBuf);
impl DownloadDestinationResolver for Destination {
    fn resolve_download_destination(&self, _: &str) -> Result<PathBuf, DownloadErrorCode> {
        Ok(self.0.clone())
    }
}
#[derive(Default)]
struct Executor(AtomicUsize);
impl DownloadExecutor for Executor {
    fn download(
        &self,
        request: DownloadExecutionRequest,
        _: CancellationToken,
        _: &dyn DownloadProgressSink,
    ) -> Result<StagedDownload, DownloadErrorCode> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let source = request
            .bilibili_source
            .as_ref()
            .expect("post-slot verified source required");
        source.validate_request(
            &request.plan,
            request.video_source.as_ref(),
            request.audio_source.as_ref(),
        )?;
        Ok(StagedDownload {
            job_id: request.job_id,
            staging_dir: request.staging_dir,
            expected_operation: request.plan.operation,
            expected_container: request.plan.output_container.clone(),
            requested_clip: request.plan.clip,
            evidence:
                local_media_downloader_lib::application::local_processing::ProcessingEvidence {
                    source_video: request
                        .plan
                        .video_format_id
                        .as_deref()
                        .and_then(|id| source.original_properties(id).unwrap().0.cloned()),
                    source_audio: request
                        .plan
                        .audio_format_id
                        .as_deref()
                        .and_then(|id| source.original_properties(id).unwrap().1.cloned()),
                    actual_container: OutputContainer::Mp4,
                    video_processing: request.plan.video_processing,
                    audio_processing: request.plan.audio_processing,
                },
        })
    }
}
struct Verifier(AtomicU64);
impl OutputVerifier for Verifier {
    fn verify(
        &self,
        staged: StagedDownload,
        _: &SelectionPlan,
        _: CancellationToken,
    ) -> Result<VerifiedOutput, DownloadErrorCode> {
        Ok(VerifiedOutput {
            path: staged.staging_dir.join("stage.mp4"),
            container: "mp4".into(),
            has_video: true,
            has_audio: true,
            duration_ms: self.0.load(Ordering::SeqCst),
            byte_length: 8,
        })
    }
}
#[derive(Default)]
struct Publisher(AtomicUsize);
impl OutputPublisher for Publisher {
    fn publish(
        &self,
        _: VerifiedOutput,
        destination: &Path,
        _: &str,
        _: &str,
        _: &CancellationToken,
    ) -> Result<PublishedOutput, DownloadErrorCode> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(PublishedOutput {
            final_path: destination.join("Bili.mp4"),
            display_name: "Bili.mp4".into(),
        })
    }
}
struct Emitter;
impl DownloadEventEmitter for Emitter {
    fn emit(&self, _: DownloadProgress) -> Result<(), DownloadErrorCode> {
        Ok(())
    }
}
struct Opener;
impl CompletedLocationOpener for Opener {
    fn open(&self, _: &Path) -> Result<(), DownloadErrorCode> {
        Ok(())
    }
}
struct Harness {
    _root: tempfile::TempDir,
    service: Arc<Service>,
    id: String,
    manager: Arc<DownloadJobManager>,
    refresh: Arc<Refresh>,
    executor: Arc<Executor>,
    publisher: Arc<Publisher>,
    verifier: Arc<Verifier>,
}
fn harness(gate: Arc<DownloadActivityGate>, tamper: bool) -> Harness {
    let root = tempfile::tempdir().unwrap();
    let service = Arc::new(Service::new(
        fixtures::Analyzer(Arc::new(fixtures::FixtureState::default())),
        Covers,
    ));
    let id = service
        .analyze(fixtures::URL, intent(), Instant::now())
        .unwrap()
        .analysis_id;
    let refresh = Arc::new(Refresh {
        service: service.clone(),
        calls: AtomicUsize::new(0),
    });
    let executor = Arc::new(Executor::default());
    let publisher = Arc::new(Publisher::default());
    let verifier = Arc::new(Verifier(AtomicU64::new(90000)));
    let manager = DownloadJobManager::for_bilibili(
        DownloadJobServices {
            activity: gate,
            input: Arc::new(Input {
                service: service.clone(),
                id: id.clone(),
                tamper,
            }),
            destination: Arc::new(Destination(root.path().join("destination"))),
            executor: executor.clone(),
            verifier: verifier.clone(),
            publisher: publisher.clone(),
            staging: Arc::new(FilesystemJobStaging::new(root.path().join("jobs"))),
            emitter: Arc::new(Emitter),
            opener: Arc::new(Opener),
        },
        refresh.clone(),
    );
    Harness {
        _root: root,
        service,
        id,
        manager,
        refresh,
        executor,
        publisher,
        verifier,
    }
}
#[test]
fn bilibili_cancel_before_slot_never_refreshes() {
    let gate = Arc::new(DownloadActivityGate::new());
    gate.set_limit(1).unwrap();
    let blocker = gate.reserve(DownloadOwner::Youtube, "YT").unwrap();
    blocker.wait_turn(&CancellationToken::new()).unwrap();
    let h = harness(gate, false);
    let job = h.manager.start_download("1", "2", intent()).unwrap().job_id;
    h.manager.cancel_download(&job).unwrap();
    assert!(h.manager.wait_for_idle(Duration::from_secs(3)));
    assert_eq!(
        h.manager.download_result(&job).unwrap().state,
        DownloadState::Canceled
    );
    assert_eq!(h.refresh.calls.load(Ordering::SeqCst), 0);
    assert_eq!(h.executor.0.load(Ordering::SeqCst), 0);
    assert_eq!(h.publisher.0.load(Ordering::SeqCst), 0);
}

#[test]
fn bilibili_shutdown_cancels_queued_jobs_without_refresh_or_publication() {
    let gate = Arc::new(DownloadActivityGate::new());
    gate.set_limit(1).unwrap();
    let blocker = gate.reserve(DownloadOwner::Instagram, "IG").unwrap();
    blocker.wait_turn(&CancellationToken::new()).unwrap();
    let h = harness(gate, false);
    let job = h.manager.start_download("1", "2", intent()).unwrap().job_id;
    h.manager.shutdown();
    assert!(h.manager.wait_for_idle(Duration::from_secs(3)));
    assert_eq!(
        h.manager.download_result(&job).unwrap().state,
        DownloadState::Canceled
    );
    assert_eq!(h.refresh.calls.load(Ordering::SeqCst), 0);
    assert_eq!(h.executor.0.load(Ordering::SeqCst), 0);
    assert_eq!(h.publisher.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        h.manager.start_download("1", "2", intent()).err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
}
#[test]
fn bilibili_queued_snapshot_survives_observed_ui_expiry_and_refreshes_once() {
    let gate = Arc::new(DownloadActivityGate::new());
    gate.set_limit(1).unwrap();
    let blocker = gate.reserve(DownloadOwner::X, "X").unwrap();
    blocker.wait_turn(&CancellationToken::new()).unwrap();
    let h = harness(gate.clone(), false);
    let job = h.manager.start_download("1", "2", intent()).unwrap().job_id;
    assert!(matches!(
        h.service
            .preview(&h.id, intent(), Instant::now() + Duration::from_secs(901)),
        Err(AnalysisErrorCode::AnalysisExpired)
    ));
    assert_eq!(h.refresh.calls.load(Ordering::SeqCst), 0);
    drop(blocker);
    assert!(h.manager.wait_for_idle(Duration::from_secs(3)));
    assert_eq!(
        h.manager.download_result(&job).unwrap().state,
        DownloadState::Completed
    );
    assert_eq!(h.refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.executor.0.load(Ordering::SeqCst), 1);
    assert_eq!(h.publisher.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        gate.snapshot().unwrap().tasks.last().unwrap().platform,
        Platform::Bilibili
    );
    assert_eq!(
        gate.snapshot().unwrap().tasks.last().unwrap().state,
        TaskPhase::Completed
    );
}
#[test]
fn bilibili_registry_and_progress_cannot_cross_owners() {
    let gate = Arc::new(DownloadActivityGate::new());
    gate.set_limit(1).unwrap();
    let foreign = gate.reserve(DownloadOwner::Youtube, "YT").unwrap();
    foreign.wait_turn(&CancellationToken::new()).unwrap();
    let h = harness(gate.clone(), false);
    let job = h.manager.start_download("1", "2", intent()).unwrap().job_id;
    assert_eq!(
        h.manager.cancel_download(&foreign.job_id()),
        Err(DownloadErrorCode::InvalidRequest)
    );
    assert_eq!(
        h.manager.download_result(&foreign.job_id()).err(),
        Some(DownloadErrorCode::InvalidRequest)
    );
    assert_eq!(
        h.manager.open_completed_location(&foreign.job_id()),
        Err(DownloadErrorCode::InvalidRequest)
    );
    drop(foreign);
    assert!(h.manager.wait_for_idle(Duration::from_secs(3)));
    let lease = gate.reserve(DownloadOwner::Bilibili, "next").unwrap();
    lease.wait_turn(&CancellationToken::new()).unwrap();
    let progress = DownloadProgress::new(
        &lease.job_id(),
        DownloadState::Downloading,
        DownloadProgressMetrics::default(),
        DownloadMessageKey::Downloading,
    )
    .unwrap();
    gate.record_single_video(DownloadOwner::Youtube, &progress);
    assert_eq!(
        gate.snapshot().unwrap().tasks.last().unwrap().state,
        TaskPhase::Preparing
    );
    gate.record_instagram(
        &local_media_downloader_lib::domain::instagram::InstagramDownloadProgress {
            job_id: lease.job_id(),
            state: local_media_downloader_lib::domain::instagram::InstagramJobState::Downloading,
            item_index: 1,
            item_count: 1,
            downloaded_bytes: Some(1),
            total_bytes: Some(10),
            percent: Some(10.0),
        },
    );
    assert_eq!(
        gate.snapshot().unwrap().tasks.last().unwrap().state,
        TaskPhase::Preparing
    );
    gate.record_single_video(DownloadOwner::Bilibili, &progress);
    assert_eq!(
        gate.snapshot().unwrap().tasks.last().unwrap().state,
        TaskPhase::Downloading
    );
    assert_eq!(gate.active_owner(), Some(DownloadOwner::Bilibili));
    assert_ne!(job, lease.job_id());
}
#[test]
fn bilibili_rejects_inconsistent_snapshot_before_reserving_and_truncated_output_before_publication()
{
    let gate = Arc::new(DownloadActivityGate::new());
    let h = harness(gate.clone(), true);
    assert_eq!(
        h.manager.start_download("1", "2", intent()).err(),
        Some(DownloadErrorCode::InvalidRequest)
    );
    assert!(gate.snapshot().unwrap().tasks.is_empty());
    let h = harness(gate, false);
    h.verifier.0.store(1000, Ordering::SeqCst);
    let job = h.manager.start_download("1", "2", intent()).unwrap().job_id;
    assert!(h.manager.wait_for_idle(Duration::from_secs(3)));
    assert_eq!(
        h.manager.download_result(&job).unwrap().error_code,
        Some(DownloadErrorCode::OutputInvalid)
    );
    assert_eq!(h.publisher.0.load(Ordering::SeqCst), 0);
}
#[test]
fn bilibili_jobs_share_limit_not_ownership() {
    for limit in 1..=4 {
        let gate = Arc::new(DownloadActivityGate::new());
        gate.set_limit(limit).unwrap();
        let (entered, receiver) = mpsc::channel();
        let mut releases = Vec::new();
        let mut workers = Vec::new();
        for owner in [
            DownloadOwner::Youtube,
            DownloadOwner::Instagram,
            DownloadOwner::X,
            DownloadOwner::Bilibili,
        ] {
            let lease = gate.reserve(owner, "task").unwrap();
            let id = lease.job_id();
            let tx = entered.clone();
            let (release, rx) = mpsc::channel();
            releases.push(release);
            workers.push(thread::spawn(move || {
                lease.wait_turn(&CancellationToken::new()).unwrap();
                tx.send(id).unwrap();
                rx.recv_timeout(Duration::from_secs(3)).unwrap();
            }));
        }
        let mut first = Vec::new();
        for _ in 0..limit {
            first.push(receiver.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        first.sort();
        assert_eq!(
            first,
            (1..=limit).map(|n| n.to_string()).collect::<Vec<_>>()
        );
        assert_eq!(
            gate.snapshot()
                .unwrap()
                .tasks
                .iter()
                .filter(|t| t.state == TaskPhase::Preparing)
                .count(),
            usize::from(limit)
        );
        for next in usize::from(limit)..4 {
            releases[next - usize::from(limit)].send(()).unwrap();
            assert_eq!(
                receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
                (next + 1).to_string()
            );
        }
        for tx in releases.iter().skip(4 - usize::from(limit)) {
            let _ = tx.send(());
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }
}
