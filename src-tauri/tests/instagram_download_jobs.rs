#[path = "support/output_policy.rs"]
mod policy_fixture;
#[path = "support/publication.rs"]
mod publication_fixture;

use std::{
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Barrier, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

use local_media_downloader_lib::{
    application::{
        download_activity::{DownloadActivityGate, DownloadOwner},
        download_executor::{
            DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink, StagedDownload,
        },
        download_job::{CompletedLocationOpener, JobStaging},
        instagram_analysis::{
            InstagramAnalysisService, InstagramPreviewLoader, InstagramPreviewSource,
            InstagramVideoAnalyzer, ResolvedInstagramDownload,
        },
        instagram_download_job::{
            InstagramDownloadEventEmitter, InstagramDownloadJobManager,
            InstagramDownloadJobServices, InstagramImageDownloader, InstagramOutputPublisher,
            InstagramPublication, InstagramPublicationRequest, InstagramVerifiedAsset,
            InstagramVideoDownloadPipeline, InstagramVideoDownloader,
        },
        instagram_video::{
            resolve_instagram_video, InstagramVideoSelector, ResolvedInstagramVideo,
        },
        media_url::{validate_media_url, ValidatedMediaUrl},
        output_verifier::{OutputVerifier, VerifiedOutput},
        process::CancellationToken,
    },
    domain::{
        download::{
            DownloadErrorCode, DownloadMessageKey, DownloadProgress, DownloadProgressMetrics,
            DownloadState,
        },
        format_selection::SelectionPlan,
        instagram::{
            InstagramDownloadIntent, InstagramItemOutcomeState, InstagramJobState,
            InstagramMediaItem, InstagramMediaKind,
        },
        media::{AnalysisErrorCode, FileSize, MediaFormat, MediaInfo, Platform, StreamKind},
    },
    infrastructure::{
        gallery_dl::{
            normalize_instagram_inventory, InstagramInventory, InstagramInventoryExtractor,
            ResolvedInstagramItem,
        },
        output::{instagram::FilesystemInstagramOutputPublisher, FilesystemJobStaging},
    },
};

const POST: &str = "https://www.instagram.com/p/Mixed_01/";
const MIXED: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/mixed-carousel.jsonl");

#[test]
fn activity_gate_is_atomic_across_youtube_instagram_and_tool_repair() {
    let gate = Arc::new(DownloadActivityGate::new());
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for owner in [DownloadOwner::Youtube, DownloadOwner::Instagram] {
        let gate = Arc::clone(&gate);
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            gate.try_acquire(owner)
        }));
    }
    barrier.wait();
    let leases = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(leases.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(leases.iter().filter(|result| result.is_err()).count(), 1);
    assert_eq!(
        gate.try_acquire(DownloadOwner::ToolRepair).err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
    drop(leases);
    assert!(gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
}

#[test]
fn activity_lease_releases_on_early_error_and_panic_unwind() {
    let gate = DownloadActivityGate::new();
    let early = (|| -> Result<(), DownloadErrorCode> {
        let _lease = gate.try_acquire(DownloadOwner::Youtube)?;
        Err(DownloadErrorCode::AnalysisUnavailable)
    })();
    assert_eq!(early, Err(DownloadErrorCode::AnalysisUnavailable));
    assert!(gate.try_acquire(DownloadOwner::Instagram).is_ok());

    let gate = DownloadActivityGate::new();
    let unwind = catch_unwind(AssertUnwindSafe(|| {
        let _lease = gate.try_acquire(DownloadOwner::Instagram).unwrap();
        panic!("fixture panic after acquisition");
    }));
    assert!(unwind.is_err());
    assert!(gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
}

struct FixtureInventory;

impl InstagramInventoryExtractor for FixtureInventory {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        _cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        normalize_instagram_inventory(source.media_id(), MIXED)
    }
}

struct FixtureAnalyzer;

impl InstagramVideoAnalyzer for FixtureAnalyzer {
    fn analyze_reel(
        &self,
        source: &ValidatedMediaUrl,
        compatibility_mode: bool,
        _cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        resolve_instagram_video(
            video_media(source.media_id()),
            InstagramVideoSelector::reel(),
            compatibility_mode,
        )
    }

    fn analyze_post_item(
        &self,
        _source: &ValidatedMediaUrl,
        ordinal: u16,
        expected_media_id: &str,
        compatibility_mode: bool,
        _cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        resolve_instagram_video(
            video_media(expected_media_id),
            InstagramVideoSelector::post_item(ordinal, expected_media_id)?,
            compatibility_mode,
        )
    }
}

struct NoopPreview;

impl InstagramPreviewLoader for NoopPreview {
    fn load_preview(
        &self,
        _item: &ResolvedInstagramItem,
        _protected_directory: &Path,
        _cancellation: &CancellationToken,
    ) -> Result<Option<InstagramPreviewSource>, AnalysisErrorCode> {
        Ok(None)
    }
}

fn resolved_batch(compatibility_mode: bool) -> ResolvedInstagramDownload {
    resolved_selection(&[1, 2, 3], compatibility_mode)
}

fn resolved_selection(
    selected_ordinals: &[u16],
    compatibility_mode: bool,
) -> ResolvedInstagramDownload {
    let service = InstagramAnalysisService::new(FixtureInventory, FixtureAnalyzer, NoopPreview);
    let preview = service.analyze(POST, Instant::now()).unwrap();
    service
        .resolve_download(
            &preview.analysis_id,
            InstagramDownloadIntent {
                item_ids: preview
                    .items
                    .iter()
                    .filter(|item| selected_ordinals.contains(&item.ordinal))
                    .map(|item| item.item_id.clone())
                    .collect(),
                compatibility_mode,
            },
            Instant::now(),
        )
        .unwrap()
}

fn video_media(media_id: &str) -> MediaInfo {
    MediaInfo {
        platform: Platform::Instagram,
        media_id: media_id.to_owned(),
        title: "Carousel video".to_owned(),
        creator: Some("creator".to_owned()),
        duration_seconds: Some(12.0),
        formats: vec![
            MediaFormat {
                id: "v1".to_owned(),
                container: "mp4".to_owned(),
                kind: StreamKind::VideoOnly,
                width: Some(1080),
                height: Some(1920),
                fps: Some(30.0),
                video_bitrate_kbps: Some(4_000.0),
                audio_bitrate_kbps: None,
                audio_sample_rate_hz: None,
                audio_channels: None,
                video_codec: Some("h264".to_owned()),
                audio_codec: None,
                dynamic_range: Some("SDR".to_owned()),
                file_size: Some(FileSize {
                    bytes: 100,
                    is_approximate: false,
                }),
            },
            MediaFormat {
                id: "a1".to_owned(),
                container: "m4a".to_owned(),
                kind: StreamKind::AudioOnly,
                width: None,
                height: None,
                fps: None,
                video_bitrate_kbps: None,
                audio_bitrate_kbps: Some(128.0),
                audio_sample_rate_hz: Some(48_000),
                audio_channels: Some(2),
                video_codec: None,
                audio_codec: Some("aac".to_owned()),
                dynamic_range: None,
                file_size: Some(FileSize {
                    bytes: 25,
                    is_approximate: false,
                }),
            },
        ],
    }
}

#[derive(Clone, Copy)]
enum VideoBehavior {
    Complete,
    Fail(DownloadErrorCode),
    BlockUntilCanceled,
}

struct FakeImageDownloader {
    calls: Arc<Mutex<Vec<(InstagramMediaKind, u16)>>>,
    failure: Option<(u16, DownloadErrorCode)>,
}

impl InstagramImageDownloader for FakeImageDownloader {
    fn download(
        &self,
        item: &ResolvedInstagramItem,
        directory: &Path,
        _cancellation: &CancellationToken,
    ) -> Result<InstagramVerifiedAsset, DownloadErrorCode> {
        self.calls
            .lock()
            .unwrap()
            .push((InstagramMediaKind::Image, item.public_item().ordinal));
        if self
            .failure
            .is_some_and(|(ordinal, _)| ordinal == item.public_item().ordinal)
        {
            return Err(self.failure.unwrap().1);
        }
        let container = item.public_item().container.as_str();
        let path = directory.join(format!("stage.{container}"));
        fs::write(&path, b"image").unwrap();
        InstagramVerifiedAsset::new(path, container, 5)
    }
}

struct FakeVideoDownloader {
    behavior: VideoBehavior,
    calls: Arc<Mutex<Vec<(InstagramMediaKind, u16)>>>,
    entered: Arc<(Mutex<bool>, Condvar)>,
}

impl InstagramVideoDownloader for FakeVideoDownloader {
    #[allow(clippy::too_many_arguments)]
    fn download(
        &self,
        _source: &ValidatedMediaUrl,
        selector: &InstagramVideoSelector,
        _compatibility_mode: bool,
        _job_id: &str,
        directory: &Path,
        cancellation: CancellationToken,
        _progress: &dyn DownloadProgressSink,
    ) -> Result<InstagramVerifiedAsset, DownloadErrorCode> {
        let ordinal = selector.playlist_item().unwrap_or(1);
        self.calls
            .lock()
            .unwrap()
            .push((InstagramMediaKind::Video, ordinal));
        match self.behavior {
            VideoBehavior::Fail(error) => return Err(error),
            VideoBehavior::BlockUntilCanceled => {
                *self.entered.0.lock().unwrap() = true;
                self.entered.1.notify_all();
                while !cancellation.is_canceled() {
                    thread::sleep(Duration::from_millis(2));
                }
                return Err(DownloadErrorCode::ProcessFailed);
            }
            VideoBehavior::Complete => {}
        }
        let path = directory.join("stage.mp4");
        fs::write(&path, b"video").unwrap();
        InstagramVerifiedAsset::new(path, "mp4", 5)
    }
}

struct FakePublisher {
    calls: Arc<Mutex<Vec<u16>>>,
}

impl InstagramOutputPublisher for FakePublisher {
    fn prepare(
        &self,
        destination: &Path,
        request: &InstagramPublicationRequest,
    ) -> Result<InstagramPublication, DownloadErrorCode> {
        if request.item_count() > 1 {
            let directory = destination.join("safe-post-folder");
            fs::create_dir(&directory).unwrap();
            InstagramPublication::new(
                directory.clone(),
                directory,
                Some("safe-post-folder".to_owned()),
                request.clone(),
            )
        } else {
            InstagramPublication::new(
                destination.to_owned(),
                destination.to_owned(),
                None,
                request.clone(),
            )
        }
    }

    fn publish(
        &self,
        _asset: InstagramVerifiedAsset,
        _publication: &InstagramPublication,
        item: &InstagramMediaItem,
        _cancellation: &CancellationToken,
    ) -> Result<String, DownloadErrorCode> {
        self.calls.lock().unwrap().push(item.ordinal);
        Ok(format!("{:02}.{}", item.ordinal, item.container))
    }
}

struct FakeEmitter {
    events: Mutex<Vec<local_media_downloader_lib::domain::instagram::InstagramDownloadProgress>>,
    fail: bool,
}

impl InstagramDownloadEventEmitter for FakeEmitter {
    fn emit(
        &self,
        progress: local_media_downloader_lib::domain::instagram::InstagramDownloadProgress,
    ) -> Result<(), DownloadErrorCode> {
        if self.fail {
            return Err(DownloadErrorCode::Unknown);
        }
        self.events.lock().unwrap().push(progress);
        Ok(())
    }
}

#[derive(Default)]
struct FakeOpener(Mutex<Vec<PathBuf>>);

impl CompletedLocationOpener for FakeOpener {
    fn open(&self, path: &Path) -> Result<(), DownloadErrorCode> {
        self.0.lock().unwrap().push(path.to_owned());
        Ok(())
    }
}

struct Harness {
    _root: tempfile::TempDir,
    manager: Arc<InstagramDownloadJobManager>,
    gate: Arc<DownloadActivityGate>,
    calls: Arc<Mutex<Vec<(InstagramMediaKind, u16)>>>,
    published: Arc<Mutex<Vec<u16>>>,
    entered: Arc<(Mutex<bool>, Condvar)>,
    emitter: Arc<FakeEmitter>,
    opener: Arc<FakeOpener>,
    destination: PathBuf,
}

impl Harness {
    fn new(video_behavior: VideoBehavior, emitter_fails: bool) -> Self {
        Self::with_image_failure(video_behavior, emitter_fails, None)
    }

    fn with_image_failure(
        video_behavior: VideoBehavior,
        emitter_fails: bool,
        image_failure: Option<(u16, DownloadErrorCode)>,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let emitter = Arc::new(FakeEmitter {
            events: Mutex::new(Vec::new()),
            fail: emitter_fails,
        });
        let opener = Arc::new(FakeOpener::default());
        let gate = Arc::new(DownloadActivityGate::new());
        let staging: Arc<dyn JobStaging> = Arc::new(FilesystemJobStaging::new(
            root.path().join("instagram-jobs"),
        ));
        let manager = InstagramDownloadJobManager::new(InstagramDownloadJobServices {
            activity: Arc::clone(&gate),
            image: Arc::new(FakeImageDownloader {
                calls: Arc::clone(&calls),
                failure: image_failure,
            }),
            video: Arc::new(FakeVideoDownloader {
                behavior: video_behavior,
                calls: Arc::clone(&calls),
                entered: Arc::clone(&entered),
            }),
            publisher: Arc::new(FakePublisher {
                calls: Arc::clone(&published),
            }),
            staging,
            emitter: emitter.clone(),
            opener: opener.clone(),
        });
        Self {
            _root: root,
            manager,
            gate,
            calls,
            published,
            entered,
            emitter,
            opener,
            destination,
        }
    }

    fn wait(&self, job_id: &str) {
        assert!(self.manager.wait_for_idle(Duration::from_secs(3)));
        assert!(self.manager.result(job_id).is_ok());
    }

    fn wait_for_video(&self) {
        let entered = self.entered.0.lock().unwrap();
        let (entered, timeout) = self
            .entered
            .1
            .wait_timeout_while(entered, Duration::from_secs(3), |entered| !*entered)
            .unwrap();
        assert!(*entered && !timeout.timed_out());
    }
}

#[test]
fn mixed_batch_continues_after_one_item_failure_and_reports_partial_in_order() {
    let harness = Harness::new(
        VideoBehavior::Fail(DownloadErrorCode::ContentChanged),
        false,
    );
    let started = harness
        .manager
        .start(resolved_batch(false), harness.destination.clone())
        .unwrap();
    harness.wait(&started.job_id);
    let result = harness.manager.result(&started.job_id).unwrap();

    assert_eq!(result.state, InstagramJobState::Partial);
    assert_eq!((result.completed_count, result.failed_count), (2, 1));
    assert_eq!(
        result.folder_display_name.as_deref(),
        Some("safe-post-folder")
    );
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.state)
            .collect::<Vec<_>>(),
        vec![
            InstagramItemOutcomeState::Completed,
            InstagramItemOutcomeState::Failed,
            InstagramItemOutcomeState::Completed,
        ]
    );
    assert_eq!(
        *harness.calls.lock().unwrap(),
        vec![
            (InstagramMediaKind::Image, 1),
            (InstagramMediaKind::Video, 2),
            (InstagramMediaKind::Image, 3),
        ]
    );
    assert_eq!(*harness.published.lock().unwrap(), vec![1, 3]);
    harness
        .manager
        .open_completed_location(&started.job_id)
        .unwrap();
    assert_eq!(harness.opener.0.lock().unwrap().len(), 1);
}

#[test]
fn real_publisher_accepts_a_non_first_single_item_and_sparse_post_selection() {
    for (selected, expected_names) in [
        (vec![3], vec!["03.webp"]),
        (vec![1, 3], vec!["01.jpg", "03.webp"]),
    ] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let manager = InstagramDownloadJobManager::new(InstagramDownloadJobServices {
            activity: Arc::new(DownloadActivityGate::new()),
            image: Arc::new(FakeImageDownloader {
                calls: Arc::clone(&calls),
                failure: None,
            }),
            video: Arc::new(FakeVideoDownloader {
                behavior: VideoBehavior::Complete,
                calls,
                entered,
            }),
            publisher: Arc::new(FilesystemInstagramOutputPublisher::new()),
            staging: Arc::new(FilesystemJobStaging::new(root.path().join("jobs"))),
            emitter: Arc::new(FakeEmitter {
                events: Mutex::new(Vec::new()),
                fail: false,
            }),
            opener: Arc::new(FakeOpener::default()),
        });

        let started = manager
            .start(resolved_selection(&selected, false), destination.clone())
            .unwrap();
        assert!(manager.wait_for_idle(Duration::from_secs(3)));
        let result = manager.result(&started.job_id).unwrap();
        assert_eq!(result.state, InstagramJobState::Completed);
        assert_eq!(result.completed_count as usize, selected.len());

        let output_root = if let Some(folder) = result.folder_display_name {
            destination.join(folder)
        } else {
            destination
        };
        let mut names = fs::read_dir(output_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, expected_names);
    }
}

#[test]
fn first_middle_and_last_item_failures_are_stable_and_do_not_skip_neighbors() {
    for (image_failure, video_behavior, failed_ordinal, expected_error) in [
        (
            Some((1, DownloadErrorCode::ProcessFailed)),
            VideoBehavior::Complete,
            1,
            DownloadErrorCode::ProcessFailed,
        ),
        (
            None,
            VideoBehavior::Fail(DownloadErrorCode::DiskFull),
            2,
            DownloadErrorCode::DiskFull,
        ),
        (
            Some((3, DownloadErrorCode::ContentChanged)),
            VideoBehavior::Complete,
            3,
            DownloadErrorCode::ContentChanged,
        ),
    ] {
        let harness = Harness::with_image_failure(video_behavior, false, image_failure);
        let started = harness
            .manager
            .start(resolved_batch(false), harness.destination.clone())
            .unwrap();
        harness.wait(&started.job_id);
        let result = harness.manager.result(&started.job_id).unwrap();

        assert_eq!(result.state, InstagramJobState::Partial);
        assert_eq!((result.completed_count, result.failed_count), (2, 1));
        let failed = result
            .items
            .iter()
            .find(|item| item.state == InstagramItemOutcomeState::Failed)
            .unwrap();
        assert_eq!(failed.ordinal, failed_ordinal);
        assert_eq!(failed.error_code, Some(expected_error));
        assert_eq!(harness.calls.lock().unwrap().len(), 3);
        assert_eq!(harness.published.lock().unwrap().len(), 2);
    }
}

#[test]
fn cancellation_preserves_published_items_and_never_starts_later_items() {
    let harness = Harness::new(VideoBehavior::BlockUntilCanceled, false);
    let started = harness
        .manager
        .start(resolved_batch(false), harness.destination.clone())
        .unwrap();
    harness.wait_for_video();
    harness.manager.cancel(&started.job_id).unwrap();
    harness.wait(&started.job_id);
    let result = harness.manager.result(&started.job_id).unwrap();

    assert_eq!(result.state, InstagramJobState::Canceled);
    assert_eq!((result.completed_count, result.failed_count), (1, 0));
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.state)
            .collect::<Vec<_>>(),
        vec![
            InstagramItemOutcomeState::Completed,
            InstagramItemOutcomeState::Canceled,
            InstagramItemOutcomeState::Canceled,
        ]
    );
    assert_eq!(
        *harness.calls.lock().unwrap(),
        vec![
            (InstagramMediaKind::Image, 1),
            (InstagramMediaKind::Video, 2),
        ]
    );
    assert_eq!(*harness.published.lock().unwrap(), vec![1]);
}

#[test]
fn cancel_during_publication_keeps_prior_items_and_removes_the_uncommitted_copy() {
    use std::sync::{atomic::AtomicBool, mpsc};

    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("out");
    fs::create_dir(&destination).unwrap();
    let staging_root = root.path().join("jobs");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(DownloadActivityGate::new());
    let manager = InstagramDownloadJobManager::new(InstagramDownloadJobServices {
        activity: gate.clone(),
        image: Arc::new(FakeImageDownloader {
            calls: calls.clone(),
            failure: None,
        }),
        video: Arc::new(FakeVideoDownloader {
            behavior: VideoBehavior::Complete,
            calls,
            entered: Arc::new((Mutex::new(false), Condvar::new())),
        }),
        publisher: Arc::new(FilesystemInstagramOutputPublisher::with_operations(
            publication_fixture::PausedCrossVolumeCopy {
                entered: entered_tx,
                resume: Mutex::new(resume_rx),
                paused: AtomicBool::new(false),
            },
        )),
        staging: Arc::new(FilesystemJobStaging::new(staging_root.clone())),
        emitter: Arc::new(FakeEmitter {
            events: Mutex::new(Vec::new()),
            fail: false,
        }),
        opener: Arc::new(FakeOpener::default()),
    });
    let id = manager
        .start(resolved_batch(false), destination.clone())
        .unwrap()
        .job_id;
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    manager.cancel(&id).unwrap();
    resume_tx.send(()).unwrap();
    assert!(manager.wait_for_idle(Duration::from_secs(3)));

    let result = manager.result(&id).unwrap();
    assert_eq!(result.state, InstagramJobState::Canceled);
    assert_eq!((result.completed_count, result.failed_count), (1, 0));
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.state)
            .collect::<Vec<_>>(),
        vec![
            InstagramItemOutcomeState::Completed,
            InstagramItemOutcomeState::Canceled,
            InstagramItemOutcomeState::Canceled,
        ]
    );
    let folder = destination.join(result.folder_display_name.unwrap());
    let outputs = fs::read_dir(folder)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(outputs.len(), 1, "no canceled video or .part may remain");
    assert_eq!(fs::read(outputs[0].path()).unwrap(), b"image");
    assert!(!staging_root.join(id).exists());
    assert!(gate.active_owner().is_none());
}

#[test]
fn completed_batch_result_is_stable_and_activity_releases_after_terminal_state() {
    let harness = Harness::new(VideoBehavior::Complete, false);
    let started = harness
        .manager
        .start(resolved_batch(true), harness.destination.clone())
        .unwrap();
    harness.wait(&started.job_id);
    let first = harness.manager.result(&started.job_id).unwrap();
    let second = harness.manager.result(&started.job_id).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.state, InstagramJobState::Completed);
    assert_eq!((first.completed_count, first.failed_count), (3, 0));
    assert!(harness.gate.try_acquire(DownloadOwner::Youtube).is_ok());
    assert_eq!(
        harness.emitter.events.lock().unwrap().last().unwrap().state,
        InstagramJobState::Completed
    );
}

#[test]
fn failed_start_releases_activity_without_installing_a_job() {
    let harness = Harness::new(VideoBehavior::Complete, true);
    assert_eq!(
        harness
            .manager
            .start(resolved_batch(false), harness.destination.clone()),
        Err(DownloadErrorCode::Unknown)
    );
    assert!(harness.gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
    assert!(!harness.manager.has_active_job());
}

#[test]
fn instagram_posts_have_independent_ids_results_and_queued_cancellation() {
    let harness = Harness::new(VideoBehavior::BlockUntilCanceled, false);
    harness.gate.set_limit(1).unwrap();
    let first = harness
        .manager
        .start(resolved_batch(false), harness.destination.clone())
        .unwrap();
    harness.wait_for_video();
    let second = harness
        .manager
        .start(resolved_batch(false), harness.destination.clone())
        .expect("second post should queue, not be rejected");
    assert_ne!(first.job_id, second.job_id);
    harness.manager.cancel(&second.job_id).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while harness.manager.result(&second.job_id).is_err() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        harness.manager.result(&second.job_id).unwrap().state,
        InstagramJobState::Canceled
    );
    assert_eq!(
        harness.calls.lock().unwrap().len(),
        2,
        "queued cancellation must not acquire any asset"
    );
    assert!(harness.manager.has_active_job());
    harness.manager.cancel(&first.job_id).unwrap();
    assert!(harness.manager.wait_for_idle(Duration::from_secs(2)));
    assert_eq!(
        harness
            .manager
            .result(&first.job_id)
            .unwrap()
            .completed_count,
        1
    );
    assert_eq!(
        harness
            .manager
            .result(&second.job_id)
            .unwrap()
            .completed_count,
        0
    );
}

#[derive(Default)]
struct RecordingPipelineExecutor {
    requests: Arc<Mutex<Vec<RecordedPipelineRequest>>>,
}

#[derive(Debug, Eq, PartialEq)]
struct RecordedPipelineRequest {
    job_id: String,
    staging_dir: PathBuf,
    playlist_item: Option<u16>,
    compatibility_mode: bool,
}

impl DownloadExecutor for RecordingPipelineExecutor {
    fn download(
        &self,
        request: DownloadExecutionRequest,
        _cancellation: CancellationToken,
        progress: &dyn DownloadProgressSink,
    ) -> Result<StagedDownload, DownloadErrorCode> {
        self.requests.lock().unwrap().push(RecordedPipelineRequest {
            job_id: request.job_id.clone(),
            staging_dir: request.staging_dir.clone(),
            playlist_item: request.playlist_item,
            compatibility_mode: request.plan.compatibility_mode,
        });
        progress.on_progress(
            DownloadProgress::new(
                &request.job_id,
                DownloadState::Downloading,
                DownloadProgressMetrics {
                    downloaded_bytes: Some(5),
                    total_bytes: Some(5),
                    percent: Some(100.0),
                    ..DownloadProgressMetrics::default()
                },
                DownloadMessageKey::Downloading,
            )
            .unwrap(),
        );
        fs::write(request.staging_dir.join("stage.mp4"), b"video").unwrap();
        Ok(StagedDownload {
            job_id: request.job_id,
            staging_dir: request.staging_dir,
            expected_operation: request.plan.operation,
            expected_container: request.plan.output_container.clone(),
            requested_clip: None,
            evidence: policy_fixture::evidence(&request.plan),
        })
    }
}

struct PipelineVerifier;

impl OutputVerifier for PipelineVerifier {
    fn verify(
        &self,
        staged: StagedDownload,
        _plan: &SelectionPlan,
        _cancellation: CancellationToken,
    ) -> Result<VerifiedOutput, DownloadErrorCode> {
        assert_eq!(
            staged
                .staging_dir
                .file_name()
                .and_then(|value| value.to_str()),
            Some(staged.job_id.as_str())
        );
        Ok(VerifiedOutput {
            path: fs::canonicalize(staged.staging_dir.join("stage.mp4")).unwrap(),
            container: "mp4".to_owned(),
            has_video: true,
            has_audio: true,
            duration_ms: 1_000,
            byte_length: 5,
        })
    }
}

#[derive(Default)]
struct PipelineProgress(Mutex<Vec<DownloadProgress>>);

impl DownloadProgressSink for PipelineProgress {
    fn on_progress(&self, progress: DownloadProgress) {
        self.0.lock().unwrap().push(progress);
    }
}

#[test]
fn video_pipeline_reanalyzes_the_exact_item_and_remaps_private_child_progress() {
    let root = tempfile::tempdir().unwrap();
    let item_directory = root.path().join("42").join("02");
    fs::create_dir_all(&item_directory).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let executor = RecordingPipelineExecutor {
        requests: Arc::clone(&requests),
    };
    let pipeline = InstagramVideoDownloadPipeline::new(FixtureAnalyzer, executor, PipelineVerifier);
    let progress = PipelineProgress::default();
    let source = validate_media_url(POST).unwrap();
    let selector = InstagramVideoSelector::post_item(2, "2002_99").unwrap();

    let asset = pipeline
        .download(
            &source,
            &selector,
            true,
            "42",
            &item_directory,
            CancellationToken::new(),
            &progress,
        )
        .unwrap();

    let canonical_item_directory = fs::canonicalize(&item_directory).unwrap();
    assert_eq!(
        asset.path().parent(),
        Some(canonical_item_directory.as_path())
    );
    assert_eq!(
        asset.path().file_name().and_then(|value| value.to_str()),
        Some("stage.mp4")
    );
    assert!(!item_directory.join("4202").exists());
    assert_eq!(progress.0.lock().unwrap()[0].job_id, "42");
    assert_eq!(
        *requests.lock().unwrap(),
        vec![RecordedPipelineRequest {
            job_id: "4202".to_owned(),
            staging_dir: item_directory.join("4202"),
            playlist_item: Some(2),
            compatibility_mode: true,
        }]
    );
}
