#[path = "support/bilibili.rs"]
mod fixtures;
use fixtures::{Analyzer, FixtureState, PNG, URL};
use local_media_downloader_lib::{
    application::{
        bilibili_analysis::{BilibiliAnalysisService, BilibiliCoverLoader},
        bilibili_download::BilibiliDownloadRefresher,
        bilibili_source::{
            BilibiliAnalysisScope, BilibiliAnalyzedSource, BilibiliAnalyzer, BilibiliPageEvidence,
            BilibiliRequestContext,
        },
        network_profile::NetworkProfile,
        process::CancellationToken,
    },
    domain::{
        bilibili::{BilibiliCoverPayload, BilibiliVideoRequest},
        download::DownloadErrorCode,
        format_selection::{SelectionIntent, SelectionMode},
        media::AnalysisErrorCode,
    },
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

fn intent() -> SelectionIntent {
    SelectionIntent {
        mode: SelectionMode::BestAvailable,
        compatibility_mode: false,
        video_format_id: None,
        output_container: None,
        clip: None,
    }
}
#[derive(Default)]
struct Covers {
    calls: AtomicUsize,
    profiles: Mutex<Vec<Arc<NetworkProfile>>>,
}
impl BilibiliCoverLoader for Covers {
    fn load(
        &self,
        _: &BilibiliPageEvidence,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.profiles.lock().unwrap().push(context.network());
        Ok(BilibiliCoverPayload {
            mime_type: "image/png".into(),
            data_base64: PNG.into(),
        })
    }
}
fn service() -> (
    BilibiliAnalysisService<Analyzer, Arc<Covers>>,
    Arc<FixtureState>,
    Arc<Covers>,
) {
    let state = Arc::new(FixtureState::default());
    let covers = Arc::new(Covers::default());
    (
        BilibiliAnalysisService::new(Analyzer(state.clone()), covers.clone()),
        state,
        covers,
    )
}

#[test]
fn bilibili_queued_snapshot_outlives_ui_but_not_identity() {
    let (s, state, covers) = service();
    let now = Instant::now();
    let preview = s.analyze(URL, intent(), now).unwrap();
    assert_eq!(preview.part, 2);
    assert_eq!(preview.media.media_id, "BV1xx411c7mD_p2");
    let snapshot = s
        .prepare_download(&preview.analysis_id, intent(), now)
        .unwrap();
    assert_eq!(snapshot.plan().video_format_id.as_deref(), Some("100026"));
    assert_eq!(snapshot.plan().audio_format_id.as_deref(), Some("30280"));
    s.cover(&preview.analysis_id, now).unwrap();
    s.cover(&preview.analysis_id, now).unwrap();
    assert_eq!(covers.calls.load(Ordering::SeqCst), 1);
    assert!(Arc::ptr_eq(
        &state.profiles.lock().unwrap()[0],
        &covers.profiles.lock().unwrap()[0]
    ));
    assert!(matches!(
        s.preview(
            &preview.analysis_id,
            intent(),
            now + Duration::from_secs(901)
        ),
        Err(AnalysisErrorCode::AnalysisExpired)
    ));
    state.metadata.lock().unwrap()["formats"][0]["url"] =
        "https://refreshed.bilivideo.cn/video.m4s?sign=NEW".into();
    let verified = s.refresh(&snapshot, CancellationToken::new()).unwrap();
    assert_eq!(
        verified.format_url("100026").unwrap().as_str(),
        "https://refreshed.bilivideo.cn/video.m4s?sign=NEW"
    );
    assert!(verified
        .validate_request(
            snapshot.plan(),
            snapshot.media().formats.iter().find(|f| f.id == "100026"),
            snapshot.media().formats.iter().find(|f| f.id == "30280")
        )
        .is_ok());
    let info: Value =
        serde_json::from_slice(&verified.format_info_json("100026").unwrap()).unwrap();
    assert_eq!(info["formats"].as_array().unwrap().len(), 1);
    assert_eq!(info["formats"][0]["format_id"], "100026");
    assert!(verified.format_info_json("not-selected").is_err());
    let before = state.probe_calls.load(Ordering::SeqCst);
    state.page.lock().unwrap()["videoData"]["pages"][1]["cid"] = 999999.into();
    assert!(matches!(
        s.refresh(&snapshot, CancellationToken::new()),
        Err(DownloadErrorCode::ContentChanged)
    ));
    assert_eq!(state.probe_calls.load(Ordering::SeqCst), before);
}

#[test]
fn bilibili_verified_fps_is_frozen_separately_from_platform_hint() {
    let (service, state, _) = service();
    state.metadata.lock().unwrap()["formats"][0]["fps"] = json!(24.39);
    state.video.lock().unwrap()["streams"][0]["avg_frame_rate"] = "24/1".into();
    let now = Instant::now();
    let preview = service.analyze(URL, intent(), now).unwrap();
    assert_eq!(
        preview
            .media
            .formats
            .iter()
            .find(|format| format.id == "100026")
            .unwrap()
            .fps,
        Some(24.0),
        "the real stream rate, not an API estimate, must be frozen before selection"
    );
    let snapshot = service
        .prepare_download(&preview.analysis_id, intent(), now)
        .unwrap();
    assert!(
        service.refresh(&snapshot, CancellationToken::new()).is_ok(),
        "unchanged API hint and actual source must refresh successfully"
    );
    state.video.lock().unwrap()["streams"][0]["avg_frame_rate"] = "25/1".into();
    assert!(
        matches!(
            service.refresh(&snapshot, CancellationToken::new()),
            Err(DownloadErrorCode::ContentChanged)
        ),
        "actual FPS changes must not pass after freezing"
    );
    state.video.lock().unwrap()["streams"][0]["avg_frame_rate"] = "24/1".into();
    state.metadata.lock().unwrap()["formats"][0]["fps"] = json!(60);
    let before = state.probe_calls.load(Ordering::SeqCst);
    assert!(matches!(
        service.refresh(&snapshot, CancellationToken::new()),
        Err(DownloadErrorCode::ContentChanged)
    ));
    assert_eq!(
        state.probe_calls.load(Ordering::SeqCst),
        before,
        "changed platform declaration must still stop before a media probe"
    );
}

#[test]
fn bilibili_refresh_rejects_known_changed_formats_before_media_probe() {
    for (key, value) in [
        ("width", json!(1280)),
        ("fps", json!(60)),
        ("vcodec", json!("hev1")),
        ("dynamic_range", json!("HDR10")),
        ("tbr", json!(1000)),
        ("format_id", json!("new-id")),
    ] {
        let (s, state, _) = service();
        let now = Instant::now();
        let p = s.analyze(URL, intent(), now).unwrap();
        let snapshot = s.prepare_download(&p.analysis_id, intent(), now).unwrap();
        let before = state.probe_calls.load(Ordering::SeqCst);
        state.metadata.lock().unwrap()["formats"][0][key] = value;
        assert!(
            matches!(
                s.refresh(&snapshot, CancellationToken::new()),
                Err(DownloadErrorCode::ContentChanged)
            ),
            "{key}"
        );
        assert_eq!(state.probe_calls.load(Ordering::SeqCst), before, "{key}");
    }
}

#[test]
fn bilibili_reanalysis_and_ui_expiry_do_not_mutate_submitted_plan_or_network() {
    let (s, state, _) = service();
    let now = Instant::now();
    let p = s.analyze(URL, intent(), now).unwrap();
    let snap = s.prepare_download(&p.analysis_id, intent(), now).unwrap();
    let newer = s.analyze(URL, intent(), now).unwrap();
    assert_ne!(p.analysis_id, newer.analysis_id);
    let v = s.refresh(&snap, CancellationToken::new()).unwrap();
    assert!(Arc::ptr_eq(
        &v.network(),
        &state.profiles.lock().unwrap()[0]
    ));
    let mut changed = snap.plan().clone();
    changed.compatibility_mode = true;
    assert_eq!(
        v.validate_request(&changed, None, None),
        Err(DownloadErrorCode::InvalidRequest)
    );
    let data = serde_json::to_string(&newer).unwrap();
    for sensitive in [
        "279787",
        "bilivideo",
        "SYNTHETIC",
        "C:\\",
        "http_headers",
        "proxy",
    ] {
        assert!(!data.contains(sensitive), "{sensitive}");
    }
}

struct Racing {
    inner: Analyzer,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
    calls: AtomicUsize,
}
impl BilibiliAnalyzer for Racing {
    fn analyze(
        &self,
        request: &BilibiliVideoRequest,
        scope: BilibiliAnalysisScope,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliAnalyzedSource, AnalysisErrorCode> {
        let result = self.inner.analyze(request, scope, context);
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.wait();
            self.release.wait();
        }
        result // deliberately ignore cancellation after the producer completed
    }
}
#[test]
fn bilibili_late_generation_cannot_publish() {
    let state = Arc::new(FixtureState::default());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let s = Arc::new(BilibiliAnalysisService::new(
        Racing {
            inner: Analyzer(state.clone()),
            entered: entered.clone(),
            release: release.clone(),
            calls: AtomicUsize::new(0),
        },
        Arc::new(Covers::default()),
    ));
    let first = s.clone();
    let worker = thread::spawn(move || first.analyze(URL, intent(), Instant::now()));
    entered.wait();
    state.metadata.lock().unwrap()["title"] = "Newest title".into();
    let latest = s.analyze(URL, intent(), Instant::now()).unwrap();
    release.wait();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(
        s.preview(&latest.analysis_id, intent(), Instant::now())
            .unwrap()
            .media
            .title,
        "Newest title"
    );
    s.shutdown();
    assert!(s.analyze(URL, intent(), Instant::now()).is_err());
    assert!(s.cover(&latest.analysis_id, Instant::now()).is_err());
}

#[test]
fn bilibili_expiry_and_invalid_selection_do_not_create_a_snapshot() {
    let (s, _, _) = service();
    let now = Instant::now();
    let p = s.analyze(URL, intent(), now).unwrap();
    let mut invalid = intent();
    invalid.video_format_id = Some("not-selected".into());
    assert!(s.prepare_download(&p.analysis_id, invalid, now).is_err());
    assert!(s
        .prepare_download(&p.analysis_id, intent(), now + Duration::from_secs(901))
        .is_err());
    assert!(s.resolve_source(&p.analysis_id, now).is_err());
}

#[test]
fn bilibili_cid_change_during_extraction_stops_before_any_media_probe() {
    let (s, state, _) = service();
    let now = Instant::now();
    let p = s.analyze(URL, intent(), now).unwrap();
    let snapshot = s.prepare_download(&p.analysis_id, intent(), now).unwrap();
    let mut changed = fixtures::page();
    changed["videoData"]["pages"][1]["cid"] = 999999.into();
    *state.change_page_after_metadata.lock().unwrap() = Some(changed);
    let before = state.probe_calls.load(Ordering::SeqCst);
    assert!(matches!(
        s.refresh(&snapshot, CancellationToken::new()),
        Err(DownloadErrorCode::ContentChanged)
    ));
    assert_eq!(state.probe_calls.load(Ordering::SeqCst), before);
}
