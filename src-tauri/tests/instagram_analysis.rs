use std::{
    fs,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use local_media_downloader_lib::{
    application::{
        instagram_analysis::{
            InstagramAnalysisService, InstagramPreviewLoader, InstagramPreviewSource,
            InstagramVideoAnalyzer, ResolvedInstagramDownloadKind,
        },
        instagram_video::{
            resolve_instagram_video, InstagramVideoSelector, ResolvedInstagramVideo,
        },
        media_url::ValidatedMediaUrl,
        process::CancellationToken,
    },
    domain::{
        instagram::{
            InstagramDownloadIntent, InstagramItemAvailability, InstagramMediaKind,
            InstagramSourceKind,
        },
        media::{AnalysisErrorCode, FileSize, MediaFormat, MediaInfo, Platform, StreamKind},
    },
    infrastructure::gallery_dl::{
        normalize_instagram_inventory, InstagramInventory, InstagramInventoryExtractor,
        ResolvedInstagramItem,
    },
    ipc::commands::{
        AnalyzeInstagramRequest, GetInstagramPreviewRequest, InstagramDownloadJobRequest,
        OpenInstagramSourceRequest, StartInstagramDownloadRequest,
    },
};

const POST: &str = "https://www.instagram.com/p/Mixed_01/";
const REEL: &str = "https://www.instagram.com/reel/Reel_01/";
const MIXED: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/mixed-carousel.jsonl");

#[derive(Default)]
struct FixtureInventory {
    calls: Mutex<Vec<String>>,
}

impl InstagramInventoryExtractor for FixtureInventory {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        _cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        self.calls.lock().unwrap().push(source.url().to_owned());
        normalize_instagram_inventory(source.media_id(), MIXED)
    }
}

struct FixtureVideo {
    fail_post: bool,
    calls: Mutex<Vec<(u16, String)>>,
}

impl FixtureVideo {
    fn working() -> Self {
        Self {
            fail_post: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn failing_post() -> Self {
        Self {
            fail_post: true,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl InstagramVideoAnalyzer for FixtureVideo {
    fn analyze_reel(
        &self,
        source: &ValidatedMediaUrl,
        compatibility_mode: bool,
        _cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        resolve_instagram_video(
            video_media(source.media_id(), "Reel title"),
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
        self.calls
            .lock()
            .unwrap()
            .push((ordinal, expected_media_id.to_owned()));
        if self.fail_post {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        resolve_instagram_video(
            video_media(expected_media_id, "Carousel video"),
            InstagramVideoSelector::post_item(ordinal, expected_media_id)?,
            compatibility_mode,
        )
    }
}

#[derive(Clone, Copy)]
enum PreviewMode {
    Valid,
    Missing,
    Avif,
    Svg,
    Oversize,
}

struct FixturePreview {
    mode: PreviewMode,
    directories: Mutex<Vec<PathBuf>>,
}

impl FixturePreview {
    fn new(mode: PreviewMode) -> Self {
        Self {
            mode,
            directories: Mutex::new(Vec::new()),
        }
    }
}

impl InstagramPreviewLoader for FixturePreview {
    fn load_preview(
        &self,
        _item: &ResolvedInstagramItem,
        protected_directory: &Path,
        _cancellation: &CancellationToken,
    ) -> Result<Option<InstagramPreviewSource>, AnalysisErrorCode> {
        self.directories
            .lock()
            .unwrap()
            .push(protected_directory.to_owned());
        Ok(match self.mode {
            PreviewMode::Valid => Some(InstagramPreviewSource {
                mime_type: "image/jpeg".to_owned(),
                bytes: vec![1, 2, 3],
            }),
            PreviewMode::Missing => None,
            PreviewMode::Avif => Some(InstagramPreviewSource {
                mime_type: "image/avif".to_owned(),
                bytes: vec![1, 2, 3],
            }),
            PreviewMode::Svg => Some(InstagramPreviewSource {
                mime_type: "image/svg+xml".to_owned(),
                bytes: b"<svg><script>alert(1)</script></svg>".to_vec(),
            }),
            PreviewMode::Oversize => Some(InstagramPreviewSource {
                mime_type: "image/png".to_owned(),
                bytes: vec![1; 8 * 1024 * 1024 + 1],
            }),
        })
    }
}

fn service(
    video: FixtureVideo,
    preview: PreviewMode,
) -> InstagramAnalysisService<FixtureInventory, FixtureVideo, FixturePreview> {
    InstagramAnalysisService::new(
        FixtureInventory::default(),
        video,
        FixturePreview::new(preview),
    )
}

fn video_media(id: &str, title: &str) -> MediaInfo {
    MediaInfo {
        platform: Platform::Instagram,
        media_id: id.to_owned(),
        title: title.to_owned(),
        creator: Some("creator".to_owned()),
        duration_seconds: Some(12.0),
        formats: vec![
            MediaFormat {
                id: "av1-2160".to_owned(),
                container: "webm".to_owned(),
                kind: StreamKind::VideoOnly,
                width: Some(2160),
                height: Some(3840),
                fps: Some(60.0),
                video_bitrate_kbps: Some(12_000.0),
                audio_bitrate_kbps: None,
                audio_sample_rate_hz: None,
                audio_channels: None,
                video_codec: Some("av1".to_owned()),
                audio_codec: None,
                dynamic_range: Some("HDR".to_owned()),
                file_size: Some(FileSize {
                    bytes: 20_000_000,
                    is_approximate: false,
                }),
            },
            MediaFormat {
                id: "opus-audio".to_owned(),
                container: "webm".to_owned(),
                kind: StreamKind::AudioOnly,
                width: None,
                height: None,
                fps: None,
                video_bitrate_kbps: None,
                audio_bitrate_kbps: Some(160.0),
                audio_sample_rate_hz: Some(48_000),
                audio_channels: Some(2),
                video_codec: None,
                audio_codec: Some("opus".to_owned()),
                dynamic_range: None,
                file_size: Some(FileSize {
                    bytes: 250_000,
                    is_approximate: false,
                }),
            },
        ],
    }
}

#[test]
fn post_analysis_preserves_order_and_keeps_images_when_one_video_is_unavailable() {
    let service = service(FixtureVideo::failing_post(), PreviewMode::Valid);
    let preview = service.analyze(POST, Instant::now()).unwrap();

    assert_eq!(preview.source_kind, InstagramSourceKind::Post);
    assert_eq!(preview.shortcode, "Mixed_01");
    assert_eq!(preview.items.len(), 3);
    assert_eq!(
        preview
            .items
            .iter()
            .map(|item| (item.ordinal, item.kind, item.availability))
            .collect::<Vec<_>>(),
        vec![
            (
                1,
                InstagramMediaKind::Image,
                InstagramItemAvailability::Ready
            ),
            (
                2,
                InstagramMediaKind::Video,
                InstagramItemAvailability::Unavailable
            ),
            (
                3,
                InstagramMediaKind::Image,
                InstagramItemAvailability::Ready
            ),
        ]
    );
    assert_eq!(
        preview.items[1].unavailable_reason,
        Some(AnalysisErrorCode::ContentChanged)
    );
    assert!(!serde_json::to_string(&preview)
        .unwrap()
        .contains("cdninstagram"));
}

#[test]
fn reel_analysis_uses_one_best_available_video_and_no_post_inventory() {
    let service = service(FixtureVideo::working(), PreviewMode::Valid);
    let preview = service.analyze(REEL, Instant::now()).unwrap();

    assert_eq!(preview.source_kind, InstagramSourceKind::Reel);
    assert_eq!(preview.items.len(), 1);
    let item = &preview.items[0];
    assert_eq!(item.kind, InstagramMediaKind::Video);
    assert_eq!(
        (item.width, item.height, item.fps),
        (Some(2160), Some(3840), Some(60.0))
    );
    assert_eq!(item.container, "mp4");
    assert_eq!(item.video_codec.as_deref(), Some("av1"));
    assert_eq!(item.dynamic_range.as_deref(), Some("HDR"));
    assert_eq!(item.video_bitrate_kbps, Some(12_000.0));
    assert_eq!(item.audio_bitrate_kbps, Some(160.0));
    assert_eq!(item.audio_codec.as_deref(), Some("opus"));
    assert!(!item.preview_available);
}

#[test]
fn plural_reel_input_uses_existing_video_session_and_canonical_open_source() {
    let service = service(FixtureVideo::working(), PreviewMode::Valid);
    let now = Instant::now();
    let preview = service
        .analyze("https://www.instagram.com/reels/Det5ABjSXVU/", now)
        .unwrap();
    assert_eq!(preview.source_kind, InstagramSourceKind::Reel);
    assert_eq!(preview.shortcode, "Det5ABjSXVU");
    assert_eq!(
        preview.source_page_url,
        "https://www.instagram.com/reel/Det5ABjSXVU/"
    );
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].kind, InstagramMediaKind::Video);
    let source = service.resolve_source(&preview.analysis_id, now).unwrap();
    assert_eq!(source.media_id(), "Det5ABjSXVU");
    assert_eq!(source.url(), "https://www.instagram.com/reel/Det5ABjSXVU/");
}

#[test]
fn same_url_replaces_the_session_and_expiry_boundary_is_closed() {
    let service = service(FixtureVideo::working(), PreviewMode::Valid);
    let now = Instant::now();
    let first = service.analyze(POST, now).unwrap();
    let second = service.analyze(POST, now + Duration::from_secs(1)).unwrap();
    assert_ne!(first.analysis_id, second.analysis_id);
    assert_eq!(
        service.load_preview(&first.analysis_id, &first.items[0].item_id, now),
        Err(AnalysisErrorCode::AnalysisExpired)
    );
    assert!(service
        .load_preview(
            &second.analysis_id,
            &second.items[0].item_id,
            now + Duration::from_secs(899),
        )
        .is_ok());
    assert_eq!(
        service.load_preview(
            &second.analysis_id,
            &second.items[0].item_id,
            now + Duration::from_secs(901),
        ),
        Err(AnalysisErrorCode::AnalysisExpired)
    );
}

#[test]
fn preview_is_id_only_bounded_cached_and_session_cleanup_removes_private_files() {
    let service = service(FixtureVideo::working(), PreviewMode::Valid);
    let now = Instant::now();
    let analysis = service.analyze(POST, now).unwrap();
    let payload = service
        .load_preview(&analysis.analysis_id, &analysis.items[0].item_id, now)
        .unwrap();
    assert_eq!(payload.mime_type, "image/jpeg");
    assert_eq!(payload.data_base64, "AQID");
    assert_eq!(
        service.load_preview(&analysis.analysis_id, "999999", now),
        Err(AnalysisErrorCode::UnsupportedMedia)
    );

    let directory = service.preview_loader().directories.lock().unwrap()[0].clone();
    fs::write(directory.join("private-marker"), b"private").unwrap();
    assert_eq!(
        service
            .load_preview(&analysis.analysis_id, &analysis.items[0].item_id, now)
            .unwrap(),
        payload
    );
    assert_eq!(
        service.preview_loader().directories.lock().unwrap().len(),
        1
    );
    let session_root = directory.parent().unwrap().to_owned();
    service.clear();
    assert!(!session_root.exists());
}

#[test]
fn unsupported_or_oversize_preview_data_fails_closed() {
    for mode in [
        PreviewMode::Missing,
        PreviewMode::Svg,
        PreviewMode::Oversize,
    ] {
        let service = service(FixtureVideo::working(), mode);
        let now = Instant::now();
        let analysis = service.analyze(POST, now).unwrap();
        assert_eq!(
            service.load_preview(&analysis.analysis_id, &analysis.items[0].item_id, now),
            Err(AnalysisErrorCode::FormatUnavailable)
        );
    }
}

#[test]
fn verified_avif_video_cover_can_be_loaded_without_reencoding_the_asset() {
    let service = service(FixtureVideo::working(), PreviewMode::Avif);
    let now = Instant::now();
    let analysis = service.analyze(POST, now).unwrap();
    let video = analysis
        .items
        .iter()
        .find(|item| item.kind == InstagramMediaKind::Video)
        .unwrap();
    let payload = service
        .load_preview(&analysis.analysis_id, &video.item_id, now)
        .unwrap();
    assert_eq!(payload.mime_type, "image/avif");
    assert_eq!(payload.data_base64, "AQID");
}

#[test]
fn download_resolution_accepts_ready_ids_in_platform_order_and_keeps_native_kinds() {
    let service = service(FixtureVideo::working(), PreviewMode::Valid);
    let now = Instant::now();
    let analysis = service.analyze(POST, now).unwrap();
    let resolved = service
        .resolve_download(
            &analysis.analysis_id,
            InstagramDownloadIntent {
                item_ids: vec![
                    analysis.items[2].item_id.clone(),
                    analysis.items[0].item_id.clone(),
                ],
                compatibility_mode: true,
            },
            now,
        )
        .unwrap();
    assert!(resolved.compatibility_mode());
    assert_eq!(
        resolved
            .items()
            .iter()
            .map(|item| item.ordinal())
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(resolved
        .items()
        .iter()
        .all(|item| matches!(item.kind(), ResolvedInstagramDownloadKind::Image(_))));
}

#[test]
fn analysis_rejects_non_instagram_urls_and_clear_invalidates_the_current_id() {
    let service = Arc::new(service(FixtureVideo::working(), PreviewMode::Valid));
    assert_eq!(
        service.analyze("https://youtu.be/91YrsgWQcEo", Instant::now()),
        Err(AnalysisErrorCode::UnsupportedPlatform)
    );
    let now = Instant::now();
    let analysis = service.analyze(POST, now).unwrap();
    service.clear();
    assert!(matches!(
        service.resolve_source(&analysis.analysis_id, now),
        Err(AnalysisErrorCode::AnalysisExpired)
    ));
}

struct BlockingPreview {
    started: Mutex<Option<mpsc::Sender<PathBuf>>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl InstagramPreviewLoader for BlockingPreview {
    fn load_preview(
        &self,
        _item: &ResolvedInstagramItem,
        protected_directory: &Path,
        _cancellation: &CancellationToken,
    ) -> Result<Option<InstagramPreviewSource>, AnalysisErrorCode> {
        self.started
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(protected_directory.to_owned())
            .unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(Some(InstagramPreviewSource {
            mime_type: "image/webp".to_owned(),
            bytes: vec![9, 8, 7],
        }))
    }
}

#[test]
fn late_preview_cannot_escape_after_a_new_analysis_replaces_its_session() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let service = Arc::new(InstagramAnalysisService::new(
        FixtureInventory::default(),
        FixtureVideo::working(),
        BlockingPreview {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(release_rx),
        },
    ));
    let now = Instant::now();
    let first = service.analyze(POST, now).unwrap();
    let old_id = first.analysis_id.clone();
    let old_item = first.items[0].item_id.clone();
    let worker = {
        let service = Arc::clone(&service);
        thread::spawn(move || service.load_preview(&old_id, &old_item, now))
    };
    let old_preview_dir = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let old_session_root = old_preview_dir.parent().unwrap().to_owned();
    let second = service.analyze(POST, now + Duration::from_secs(1)).unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap(),
        Err(AnalysisErrorCode::AnalysisExpired)
    );
    assert!(!old_session_root.exists());
    assert!(service
        .resolve_source(&second.analysis_id, now + Duration::from_secs(1))
        .is_ok());
}

struct CancelAwareInventory {
    started: Mutex<Option<mpsc::Sender<()>>>,
}

impl InstagramInventoryExtractor for CancelAwareInventory {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        self.started
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        while !cancellation.is_canceled() {
            thread::yield_now();
        }
        normalize_instagram_inventory(source.media_id(), MIXED)
    }
}

#[test]
fn clear_cancels_an_inflight_analysis_and_prevents_late_session_installation() {
    let (started_tx, started_rx) = mpsc::channel();
    let service = Arc::new(InstagramAnalysisService::new(
        CancelAwareInventory {
            started: Mutex::new(Some(started_tx)),
        },
        FixtureVideo::working(),
        FixturePreview::new(PreviewMode::Valid),
    ));
    let worker = {
        let service = Arc::clone(&service);
        thread::spawn(move || service.analyze(POST, Instant::now()))
    };
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    service.clear();
    assert_eq!(
        worker.join().unwrap(),
        Err(AnalysisErrorCode::AnalysisExpired)
    );
}

#[test]
fn instagram_command_requests_reject_unknown_fields_and_never_accept_an_open_url() {
    assert!(
        serde_json::from_value::<AnalyzeInstagramRequest>(serde_json::json!({
            "url": POST
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<AnalyzeInstagramRequest>(serde_json::json!({
            "url": POST,
            "cookie": "private"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<GetInstagramPreviewRequest>(serde_json::json!({
            "analysisId": "1",
            "itemId": "2"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<GetInstagramPreviewRequest>(serde_json::json!({
            "analysisId": "1",
            "itemId": "2",
            "signedUrl": "https://cdninstagram.com/private"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<OpenInstagramSourceRequest>(serde_json::json!({
            "analysisId": "1"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<OpenInstagramSourceRequest>(serde_json::json!({
            "analysisId": "1",
            "url": "https://example.com/"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<StartInstagramDownloadRequest>(serde_json::json!({
            "analysisId": "1",
            "destinationId": "2",
            "intent": { "itemIds": ["3"], "compatibilityMode": false }
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<StartInstagramDownloadRequest>(serde_json::json!({
            "analysisId": "1",
            "destinationId": "2",
            "intent": { "itemIds": ["3"], "compatibilityMode": false },
            "url": POST
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<InstagramDownloadJobRequest>(serde_json::json!({
            "jobId": "4"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<InstagramDownloadJobRequest>(serde_json::json!({
            "jobId": "4",
            "path": "C:\\private"
        }))
        .is_err()
    );
    let capability = include_str!("../capabilities/main.json");
    assert!(!capability.contains("opener:"));
    for permission in [
        "allow-start-instagram-download",
        "allow-cancel-instagram-download",
        "allow-get-instagram-download-result",
        "allow-open-instagram-completed-location",
    ] {
        assert!(capability.contains(permission));
    }
}
