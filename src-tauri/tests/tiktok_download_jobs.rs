#[path = "support/output_policy.rs"]
mod policy_fixture;
use local_media_downloader_lib::{
    application::{
        download_activity::{DownloadActivityGate, DownloadOwner, TaskPhase},
        download_executor::{
            DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink, StagedDownload,
        },
        download_job::{CompletedLocationOpener, JobStaging},
        media_url::{validate_media_url, ValidatedMediaUrl},
        output_verifier::{OutputVerifier, VerifiedOutput},
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tiktok_analysis::{ResolvedTikTokDownload, TikTokAnalysisService, TikTokPreviewLoader},
        tiktok_download_job::{
            TikTokDownloadEventEmitter, TikTokDownloadJobManager, TikTokDownloadJobServices,
            TikTokImageDownloader, TikTokVerifiedAsset,
        },
        tiktok_source::{
            TikTokAttachmentIdentity, TikTokInventory, TikTokInventoryExtractor,
            TikTokRequestContext, TikTokSourceItem,
        },
        tiktok_video::{resolve_tiktok_video, ResolvedTikTokVideo, TikTokVideoAnalyzer},
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::SelectionPlan,
        managed_tools::{ManagedToolError, ToolName},
        media::AnalysisErrorCode,
        tiktok::{
            TikTokDownloadIntent, TikTokDownloadProgress, TikTokDownloadResult,
            TikTokItemOutcomeState, TikTokJobState, TikTokPreviewPayload,
        },
    },
    infrastructure::{
        image_verification::verify_image_file,
        output::{tiktok::FilesystemTikTokOutputPublisher, FilesystemJobStaging},
        tiktok::inventory::parse_tiktok_inventory,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
struct Initial;
fn inventory(source: &ValidatedMediaUrl) -> TikTokInventory {
    use serde_json::json;
    let items=(1..=4).map(|n|{
        let image=n==1||n==3;
        json!({"itemId":format!("1000000000000000000{n}"),"identity":format!("asset-{n}"),"ordinal":n,"kind":if image{"image"}else{"video"},"width":if n==3{1080}else{1920},"height":1080,"urls":[format!("https://p16.tiktokcdn.com/asset-{n}")],"previewUrls":[],"candidates":if image{json!([])}else{json!([{"identity":"original","urls":[format!("https://v16-webapp-prime.tiktok.com/video-{n}")],"width":1920,"height":1080,"fps":30,"videoCodec":"h264","audioCodec":"aac","videoBitrateKbps":2000,"audioBitrateKbps":128,"container":"mp4","dynamicRange":"SDR"}])}})
    }).collect::<Vec<_>>();
    parse_tiktok_inventory(&json!({"postId":source.media_id(),"title":"Synthetic post","creator":"Test","duration":1,"sourcePageUrl":source.url(),"context":{"cookies":[]},"items":items}),source).unwrap()
}
impl TikTokInventoryExtractor for Initial {
    fn extract_post(
        &self,
        s: &ValidatedMediaUrl,
        _: &TikTokRequestContext,
    ) -> Result<TikTokInventory, AnalysisErrorCode> {
        Ok(inventory(s))
    }
}
struct Video(bool);
impl TikTokVideoAnalyzer for Video {
    fn analyze_item(
        &self,
        inv: &TikTokInventory,
        id: &TikTokAttachmentIdentity,
        compat: bool,
        ctx: &TikTokRequestContext,
    ) -> Result<ResolvedTikTokVideo, AnalysisErrorCode> {
        if !self.0 {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        resolve_tiktok_video(inv, id, compat, ctx)
    }
}
struct NoPreview;
impl TikTokPreviewLoader for NoPreview {
    fn load(
        &self,
        _: &TikTokSourceItem,
        _: &TikTokRequestContext,
    ) -> Result<Option<TikTokPreviewPayload>, AnalysisErrorCode> {
        Ok(None)
    }
}
fn queued(indices: &[usize], videos: bool) -> ResolvedTikTokDownload {
    let service = TikTokAnalysisService::new(Initial, Video(videos), NoPreview);
    let now = Instant::now();
    let preview = service
        .analyze(
            "https://www.tiktok.com/@example/video/12345678901234567890",
            now,
        )
        .unwrap();
    service
        .resolve_download(
            &preview.analysis_id,
            TikTokDownloadIntent {
                item_ids: indices
                    .iter()
                    .map(|n| preview.items[*n].item_id.clone())
                    .collect(),
                compatibility_mode: false,
            },
            now,
        )
        .unwrap()
}
#[derive(Clone, Copy)]
enum Change {
    None,
    Reorder,
    Key,
    Signature,
    Missing,
    WrongParent,
    Outdated,
}
struct Fresh {
    change: Change,
    calls: Arc<AtomicUsize>,
    network: Arc<local_media_downloader_lib::application::network_profile::NetworkProfile>,
}
impl TikTokInventoryExtractor for Fresh {
    fn extract_post(
        &self,
        s: &ValidatedMediaUrl,
        ctx: &TikTokRequestContext,
    ) -> Result<TikTokInventory, AnalysisErrorCode> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(Arc::ptr_eq(&self.network, &ctx.network));
        assert!(ctx.deadline > Instant::now());
        let mut inv = inventory(s);
        match self.change {
            Change::Outdated => return Err(AnalysisErrorCode::ExtractorOutdated),
            Change::None => {}
            Change::Reorder => {
                inv.items.swap(0, 2);
                for (n, i) in inv.items.iter_mut().enumerate() {
                    i.ordinal = n as u16 + 1;
                }
            }
            Change::Key => {
                inv.items[1].identity.as_mut().unwrap().media_key = Some("7_changed".into())
            }
            Change::Signature => {
                for c in &mut inv.items[1].video_candidates {
                    c.url.set_query(Some("signature=new"));
                }
            }
            Change::Missing => {
                inv.items.remove(0);
                for (n, i) in inv.items.iter_mut().enumerate() {
                    i.ordinal = n as u16 + 1;
                }
            }
            Change::WrongParent => {
                inv.source =
                    validate_media_url("https://www.tiktok.com/@example/video/999").unwrap()
            }
        }
        Ok(inv)
    }
}
struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixture".into(),
            path: PathBuf::from("C:/managed/ffprobe.exe"),
        })
    }
}
struct Probe;
impl ProcessSupervisor for Probe {
    fn run(
        &self,
        _: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        Ok(ProcessExit{code:0,stdout:br#"{"streams":[{"codec_type":"video","codec_name":"mjpeg","width":1600,"height":900}],"format":{"format_name":"jpeg_pipe"}}"#.to_vec(),stderr:vec![]})
    }
}
#[derive(Clone, Copy)]
enum Action {
    Normal,
    FailAll,
    FailSecond,
    CancelSecond,
    PanicSecond,
    ExpiredFirst,
}
struct Image {
    action: Action,
    calls: Arc<Mutex<Vec<String>>>,
}
impl TikTokImageDownloader for Image {
    fn download(
        &self,
        item: &TikTokSourceItem,
        dir: &Path,
        ctx: &TikTokRequestContext,
    ) -> Result<TikTokVerifiedAsset, DownloadErrorCode> {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(item.identity.as_ref().unwrap().media_id.clone());
            calls.len()
        };
        match (self.action, n) {
            (Action::ExpiredFirst, 1) => return Err(DownloadErrorCode::AccessDenied),
            (Action::FailAll, _) | (Action::FailSecond, 2) => {
                return Err(DownloadErrorCode::NetworkFailed)
            }
            (Action::CancelSecond, 2) => {
                ctx.cancellation.cancel();
                return Err(DownloadErrorCode::ProcessFailed);
            }
            (Action::PanicSecond, 2) => panic!("external image fixture panic"),
            _ => {}
        }
        let path = dir.join("stage.jpg");
        let mut bytes = vec![0xff, 0xd8, 0xff];
        bytes.extend(item.identity.as_ref().unwrap().media_id.as_bytes());
        fs::write(&path, bytes).unwrap();
        Ok(TikTokVerifiedAsset::Image(verify_image_file(
            &Resolver,
            &Probe,
            &path,
            None,
            1024,
            &ctx.cancellation,
        )?))
    }
}
struct Executor {
    calls: Arc<Mutex<Vec<(String, String)>>>,
}
impl DownloadExecutor for Executor {
    fn download(
        &self,
        r: DownloadExecutionRequest,
        _: CancellationToken,
        _: &dyn DownloadProgressSink,
    ) -> Result<StagedDownload, DownloadErrorCode> {
        assert!(r.instagram_selector.is_none());
        assert!(r.playlist_item.is_none());
        assert!(r.plan.clip.is_none());
        let id = r
            .tiktok_source
            .as_ref()
            .unwrap()
            .identity()
            .media_id
            .clone();
        self.calls.lock().unwrap().push((r.job_id.clone(), id));
        fs::write(r.staging_dir.join("stage.mp4"), b"verified video bytes").unwrap();
        Ok(StagedDownload {
            job_id: r.job_id,
            staging_dir: r.staging_dir,
            expected_operation: r.plan.operation,
            expected_container: r.plan.output_container.clone(),
            requested_clip: None,
            evidence: policy_fixture::evidence(&r.plan),
        })
    }
}
struct Verifier {
    calls: Arc<AtomicUsize>,
}
impl OutputVerifier for Verifier {
    fn verify(
        &self,
        s: StagedDownload,
        p: &SelectionPlan,
        _: CancellationToken,
    ) -> Result<VerifiedOutput, DownloadErrorCode> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let path = s.staging_dir.join("stage.mp4").canonicalize().unwrap();
        Ok(VerifiedOutput {
            byte_length: fs::metadata(&path).unwrap().len(),
            path,
            container: p.output_container.clone().unwrap(),
            has_video: true,
            has_audio: p.audio_format_id.is_some(),
            duration_ms: 1000,
        })
    }
}
struct Emitter {
    events: Arc<Mutex<Vec<TikTokDownloadProgress>>>,
    panic_terminal: bool,
}
impl TikTokDownloadEventEmitter for Emitter {
    fn emit(&self, p: TikTokDownloadProgress) -> Result<(), DownloadErrorCode> {
        let terminal = matches!(
            p.state,
            TikTokJobState::Completed
                | TikTokJobState::Partial
                | TikTokJobState::Failed
                | TikTokJobState::Canceled
        );
        self.events.lock().unwrap().push(p);
        if terminal && self.panic_terminal {
            panic!("fixture terminal listener");
        }
        Ok(())
    }
}
struct Opener {
    paths: Arc<Mutex<Vec<PathBuf>>>,
}
impl CompletedLocationOpener for Opener {
    fn open(&self, p: &Path) -> Result<(), DownloadErrorCode> {
        assert!(p.is_dir());
        self.paths.lock().unwrap().push(p.to_owned());
        Ok(())
    }
}
struct Staging {
    inner: FilesystemJobStaging,
    created: Arc<AtomicUsize>,
}
impl JobStaging for Staging {
    fn create(&self, id: &str) -> Result<PathBuf, DownloadErrorCode> {
        self.created.fetch_add(1, Ordering::SeqCst);
        self.inner.create(id)
    }
    fn cleanup(&self, id: &str) -> Result<(), DownloadErrorCode> {
        self.inner.cleanup(id)
    }
}
struct Harness {
    manager: Arc<TikTokDownloadJobManager>,
    gate: Arc<DownloadActivityGate>,
    output: PathBuf,
    staging: PathBuf,
    fresh: Arc<AtomicUsize>,
    created: Arc<AtomicUsize>,
    images: Arc<Mutex<Vec<String>>>,
    executions: Arc<Mutex<Vec<(String, String)>>>,
    verified: Arc<AtomicUsize>,
    opened: Arc<Mutex<Vec<PathBuf>>>,
    events: Arc<Mutex<Vec<TikTokDownloadProgress>>>,
    _root: tempfile::TempDir,
}
fn harness(
    download: &ResolvedTikTokDownload,
    change: Change,
    action: Action,
    panic_terminal: bool,
) -> Harness {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let staging = root.path().join("staging");
    let gate = Arc::new(DownloadActivityGate::new());
    let fresh = Arc::new(AtomicUsize::new(0));
    let created = Arc::new(AtomicUsize::new(0));
    let images = Arc::new(Mutex::new(vec![]));
    let executions = Arc::new(Mutex::new(vec![]));
    let verified = Arc::new(AtomicUsize::new(0));
    let opened = Arc::new(Mutex::new(vec![]));
    let events = Arc::new(Mutex::new(vec![]));
    let manager = TikTokDownloadJobManager::new(TikTokDownloadJobServices {
        activity: gate.clone(),
        inventory: Arc::new(Fresh {
            change,
            calls: fresh.clone(),
            network: download.network.clone(),
        }),
        image: Arc::new(Image {
            action,
            calls: images.clone(),
        }),
        video: Arc::new(Video(true)),
        executor: Arc::new(Executor {
            calls: executions.clone(),
        }),
        verifier: Arc::new(Verifier {
            calls: verified.clone(),
        }),
        staging: Arc::new(Staging {
            inner: FilesystemJobStaging::new(staging.clone()),
            created: created.clone(),
        }),
        publisher: Arc::new(FilesystemTikTokOutputPublisher::new()),
        opener: Arc::new(Opener {
            paths: opened.clone(),
        }),
        emitter: Arc::new(Emitter {
            events: events.clone(),
            panic_terminal,
        }),
    });
    Harness {
        manager,
        gate,
        output,
        staging,
        fresh,
        created,
        images,
        executions,
        verified,
        opened,
        events,
        _root: root,
    }
}
fn finished(h: &Harness, id: &str) -> TikTokDownloadResult {
    assert!(h.manager.wait_for_idle(Duration::from_secs(5)));
    let result = h.manager.result(id).unwrap();
    result.validate().unwrap();
    result
}
#[test]
fn tiktok_reordered_post_downloads_stable_identities_using_original_output_ordinals() {
    let download = queued(&[0, 2], false);
    let h = harness(&download, Change::Reorder, Action::Normal, false);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let r = finished(&h, &start.job_id);
    assert_eq!(r.state, TikTokJobState::Completed);
    assert_eq!(r.completed_count, 2);
    assert_eq!(r.post_item_count, 4);
    assert_eq!(r.unavailable_count, 2);
    assert!(r.content_changed);
    assert_eq!(
        *h.images.lock().unwrap(),
        vec!["10000000000000000001", "10000000000000000003"]
    );
    let folder = h.output.join(r.folder_display_name.as_ref().unwrap());
    assert!(folder.join("01.jpg").is_file());
    assert!(folder.join("03.jpg").is_file());
    h.manager.open_completed_location(&start.job_id).unwrap();
    assert_eq!(h.opened.lock().unwrap().len(), 1);
    assert!(!h.staging.join(&start.job_id).exists());
    assert!(h.gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
}
#[test]
fn tiktok_signature_refresh_is_not_content_change_but_unselected_identity_change_is() {
    for (change, expected) in [(Change::Signature, false), (Change::Key, true)] {
        let download = queued(&[0], false);
        let h = harness(&download, change, Action::Normal, false);
        let start = h.manager.start(download, h.output.clone()).unwrap();
        let r = finished(&h, &start.job_id);
        assert_eq!(r.content_changed, expected);
        assert_eq!(r.completed_count, 1);
        assert_eq!(h.fresh.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn tiktok_missing_selected_identity_fails_without_downloading_its_new_neighbor() {
    let download = queued(&[0, 2], false);
    let h = harness(&download, Change::Missing, Action::Normal, false);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let r = finished(&h, &start.job_id);
    assert_eq!(r.state, TikTokJobState::Partial);
    assert_eq!(
        r.items[0].error_code,
        Some(DownloadErrorCode::ContentChanged)
    );
    assert_eq!(*h.images.lock().unwrap(), vec!["10000000000000000003"]);
    assert!(r.content_changed);
}
#[test]
fn tiktok_partial_failure_and_cancellation_keep_already_published_items() {
    for (action, state, second) in [
        (
            Action::FailSecond,
            TikTokJobState::Partial,
            TikTokItemOutcomeState::Failed,
        ),
        (
            Action::CancelSecond,
            TikTokJobState::Canceled,
            TikTokItemOutcomeState::Canceled,
        ),
        (
            Action::PanicSecond,
            TikTokJobState::Partial,
            TikTokItemOutcomeState::Failed,
        ),
    ] {
        let download = queued(&[0, 2], false);
        let h = harness(&download, Change::None, action, false);
        let start = h.manager.start(download, h.output.clone()).unwrap();
        let r = finished(&h, &start.job_id);
        assert_eq!(r.state, state);
        assert_eq!(r.items[0].state, TikTokItemOutcomeState::Completed);
        assert_eq!(r.items[1].state, second);
        assert!(h
            .output
            .join(r.folder_display_name.unwrap())
            .join("01.jpg")
            .is_file());
        h.manager.open_completed_location(&start.job_id).unwrap();
        assert!(h.gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
    }
}
#[test]
fn tiktok_all_failures_or_wrong_parent_leave_no_empty_post_folder() {
    for (change, action) in [
        (Change::None, Action::FailAll),
        (Change::WrongParent, Action::Normal),
    ] {
        let download = queued(&[0, 2], false);
        let h = harness(&download, change, action, false);
        let start = h.manager.start(download, h.output.clone()).unwrap();
        let r = finished(&h, &start.job_id);
        assert_eq!(r.state, TikTokJobState::Failed);
        assert!(r.folder_display_name.is_none());
        assert_eq!(fs::read_dir(&h.output).unwrap().count(), 0);
        assert!(h.manager.open_completed_location(&start.job_id).is_err());
        if matches!(action, Action::FailAll) {
            assert_eq!(h.images.lock().unwrap().len(), 2);
        }
    }
}
#[test]
fn tiktok_queued_cancel_creates_no_staging_or_network_work() {
    let download = queued(&[0], false);
    let h = harness(&download, Change::None, Action::Normal, false);
    h.gate.set_limit(1).unwrap();
    let held = h.gate.reserve(DownloadOwner::Youtube, "held").unwrap();
    held.wait_turn(&CancellationToken::new()).unwrap();
    let start = h.manager.start(download, h.output.clone()).unwrap();
    h.manager.cancel(&start.job_id).unwrap();
    let r = finished(&h, &start.job_id);
    assert_eq!(r.state, TikTokJobState::Canceled);
    assert_eq!(h.fresh.load(Ordering::SeqCst), 0);
    assert_eq!(h.created.load(Ordering::SeqCst), 0);
    assert!(h.images.lock().unwrap().is_empty());
    assert_eq!(fs::read_dir(&h.output).unwrap().count(), 0);
    assert_eq!(
        h.gate.snapshot().unwrap().tasks[1].platform,
        local_media_downloader_lib::domain::media::Platform::Tiktok
    );
    drop(held);
}
#[test]
fn tiktok_video_calls_native_executor_and_verifier_before_publication() {
    let download = queued(&[1], true);
    let h = harness(&download, Change::None, Action::Normal, false);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let r = finished(&h, &start.job_id);
    assert_eq!(r.state, TikTokJobState::Completed);
    assert_eq!(h.executions.lock().unwrap()[0].1, "10000000000000000002");
    assert_eq!(h.verified.load(Ordering::SeqCst), 1);
    assert!(h
        .output
        .join(r.folder_display_name.unwrap())
        .join("02.mp4")
        .is_file());
}
#[test]
fn tiktok_expired_asset_gets_one_identity_checked_refresh_without_overwriting_success() {
    let download = queued(&[0], false);
    let h = harness(&download, Change::None, Action::ExpiredFirst, false);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let result = finished(&h, &start.job_id);
    assert_eq!(result.completed_count, 1);
    assert_eq!(h.fresh.load(Ordering::SeqCst), 2);
    assert_eq!(h.images.lock().unwrap().len(), 2);
}

#[test]
fn tiktok_hashed_native_identity_is_accepted_and_quality_downgrade_is_rejected() {
    let mut download = queued(&[1], true);
    let identity = download.items[0].identity.clone();
    let h = harness(&download, Change::None, Action::Normal, false);
    download.items[0].public.width = Some(3840);
    download.items[0].public.height = Some(2160);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let result = finished(&h, &start.job_id);
    assert_eq!(result.completed_count, 0);
    assert_eq!(
        result.items[0].error_code,
        Some(DownloadErrorCode::ContentChanged)
    );
    assert!(h.executions.lock().unwrap().is_empty());
    let mut download = queued(&[0], false);
    download.items[0].identity.media_id = "123-0123456789abcdef0123".into();
    download.inventory_slots[0].identity = Some(download.items[0].identity.clone());
    assert!(download.items[0].identity != identity);
    // A valid native hashed ID passes snapshot validation; the refresh then reports the missing asset.
    assert!(h.manager.start(download, h.output.clone()).is_ok());
}
#[test]
fn tiktok_terminal_listener_panic_cannot_strand_a_slot_or_erase_file_results() {
    let download = queued(&[0], false);
    let h = harness(&download, Change::None, Action::Normal, true);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let r = finished(&h, &start.job_id);
    assert_eq!(r.completed_count, 1);
    assert_eq!(
        h.gate.snapshot().unwrap().tasks[0].state,
        TaskPhase::Completed
    );
    assert!(h.gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
    assert!(h
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e.state == TikTokJobState::Completed));
    h.manager.shutdown();
    assert!(h
        .manager
        .start(queued(&[0], false), h.output.clone())
        .is_err());
}

#[test]
fn tiktok_mixed_fixture_keeps_and_all_original_ordinals() {
    let download = queued(&[0, 1, 2, 3], true);
    let h = harness(&download, Change::None, Action::Normal, false);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let result = finished(&h, &start.job_id);
    assert_eq!(result.state, TikTokJobState::Completed);
    assert_eq!(result.completed_count, 4);
    assert_eq!(h.verified.load(Ordering::SeqCst), 2);
    let folder = h.output.join(result.folder_display_name.unwrap());
    for name in ["01.jpg", "02.mp4", "03.jpg", "04.mp4"] {
        assert!(folder.join(name).is_file(), "{name}");
    }
    assert_eq!(
        result.items[3].kind,
        local_media_downloader_lib::domain::tiktok::TikTokMediaKind::Video
    );
}
#[test]
fn tiktok_same_post_concurrent_requests_have_separate_results_and_no_overwrite() {
    let download = queued(&[0], false);
    let h = harness(&download, Change::None, Action::Normal, false);
    let a = h.manager.start(download.clone(), h.output.clone()).unwrap();
    let b = h.manager.start(download, h.output.clone()).unwrap();
    assert_ne!(a.job_id, b.job_id);
    let first = finished(&h, &a.job_id);
    let second = finished(&h, &b.job_id);
    assert_ne!(first.folder_display_name, second.folder_display_name);
    assert_eq!(first.completed_count, 1);
    assert_eq!(second.completed_count, 1);
    assert_eq!(fs::read_dir(&h.output).unwrap().count(), 2);
}
#[test]
fn tiktok_shutdown_cancels_waiting_work_without_touching_other_platforms() {
    let download = queued(&[0], false);
    let h = harness(&download, Change::None, Action::Normal, false);
    h.gate.set_limit(1).unwrap();
    let held = h.gate.reserve(DownloadOwner::Youtube, "held").unwrap();
    held.wait_turn(&CancellationToken::new()).unwrap();
    let start = h.manager.start(download, h.output.clone()).unwrap();
    h.manager.shutdown();
    assert_eq!(
        h.manager.result(&start.job_id).unwrap().state,
        TikTokJobState::Canceled
    );
    assert_eq!(h.created.load(Ordering::SeqCst), 0);
    assert_eq!(h.fresh.load(Ordering::SeqCst), 0);
    assert_eq!(
        h.gate.snapshot().unwrap().tasks[0].state,
        TaskPhase::Preparing
    );
    drop(held);
}
#[test]
fn tiktok_forged_snapshot_is_rejected_before_queue_or_filesystem_work() {
    let mut download = queued(&[0], false);
    let h = harness(&download, Change::None, Action::Normal, false);
    download.inventory_slots[0]
        .identity
        .as_mut()
        .unwrap()
        .media_id = "9".into();
    assert!(h.manager.start(download, h.output.clone()).is_err());
    assert!(h.gate.snapshot().unwrap().tasks.is_empty());
    assert_eq!(h.created.load(Ordering::SeqCst), 0);
}
#[test]
fn tiktok_inventory_extractor_failure_keeps_its_stable_error_code() {
    let download = queued(&[0], false);
    let h = harness(&download, Change::Outdated, Action::Normal, false);
    let start = h.manager.start(download, h.output.clone()).unwrap();
    let result = finished(&h, &start.job_id);
    assert_eq!(
        result.items[0].error_code,
        Some(DownloadErrorCode::ExtractorOutdated)
    );
}
