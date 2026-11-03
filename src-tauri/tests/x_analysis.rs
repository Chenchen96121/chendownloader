use local_media_downloader_lib::{
    application::{
        media_url::{validate_media_url, ValidatedMediaUrl},
        x_analysis::{XAnalysisService, XPreviewLoader},
        x_source::{
            XAttachmentIdentity, XInventory, XInventoryExtractor, XRequestContext, XSourceItem,
        },
        x_video::{ResolvedXVideo, XVideoAnalyzer},
    },
    domain::{
        media::AnalysisErrorCode,
        x::{XDownloadIntent, XItemAvailability, XPreviewPayload},
    },
    infrastructure::x::inventory::parse_x_inventory,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
struct Inventory;
impl XInventoryExtractor for Inventory {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        _: &XRequestContext,
    ) -> Result<XInventory, AnalysisErrorCode> {
        parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), source)
    }
}
struct NoVideo;

#[test]
fn x_shutdown_is_terminal_even_for_analyze_handles_already_captured_by_ipc() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counted(Arc<AtomicUsize>);
    impl XInventoryExtractor for Counted {
        fn extract_post(
            &self,
            source: &ValidatedMediaUrl,
            _: &XRequestContext,
        ) -> Result<XInventory, AnalysisErrorCode> {
            self.0.fetch_add(1, Ordering::SeqCst);
            parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), source)
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let service = XAnalysisService::new(Counted(calls.clone()), NoVideo, Preview);
    let now = Instant::now();
    let preview = service
        .analyze("https://x.com/i/web/status/12345678901234567890", now)
        .unwrap();
    let snapshot = service
        .resolve_download(
            &preview.analysis_id,
            XDownloadIntent {
                item_ids: vec![preview.items[0].item_id.clone()],
                compatibility_mode: false,
            },
            now,
        )
        .unwrap();
    service.shutdown();
    service.shutdown();
    assert!(service.resolve_source(&preview.analysis_id, now).is_err());
    assert!(service
        .load_preview(&preview.analysis_id, &preview.items[0].item_id, now)
        .is_err());
    assert!(matches!(
        service.analyze("https://x.com/i/web/status/12345678901234567890", now),
        Err(AnalysisErrorCode::AnalysisUnavailable)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(snapshot.items.len(), 1); // The submitted snapshot remains independent.
}
impl XVideoAnalyzer for NoVideo {
    fn analyze_item(
        &self,
        _: &XInventory,
        _: &XAttachmentIdentity,
        _: bool,
        _: &XRequestContext,
    ) -> Result<ResolvedXVideo, AnalysisErrorCode> {
        Err(AnalysisErrorCode::FormatUnavailable)
    }
}

#[test]
fn x_queued_snapshot_retains_unavailable_slots_and_full_identity() {
    struct MissingIdentity;
    impl XInventoryExtractor for MissingIdentity {
        fn extract_post(
            &self,
            source: &ValidatedMediaUrl,
            _: &XRequestContext,
        ) -> Result<XInventory, AnalysisErrorCode> {
            let mut raw: serde_json::Value =
                serde_json::from_slice(include_bytes!("../../fixtures/x/mixed-post.json")).unwrap();
            raw[0][1]["legacy"]["extended_entities"]["media"][0]["id_str"] =
                serde_json::Value::Null;
            parse_x_inventory(&serde_json::to_vec(&raw).unwrap(), source)
        }
    }
    let service = XAnalysisService::new(MissingIdentity, NoVideo, Preview);
    let now = Instant::now();
    let preview = service
        .analyze("https://x.com/i/web/status/12345678901234567890", now)
        .unwrap();
    let download = service
        .resolve_download(
            &preview.analysis_id,
            XDownloadIntent {
                item_ids: vec![preview.items[2].item_id.clone()],
                compatibility_mode: false,
            },
            now,
        )
        .unwrap();
    assert_eq!(download.inventory_slots.len(), 4);
    assert_eq!(download.inventory_slots[0].ordinal, 1);
    assert!(download.inventory_slots[0].identity.is_none());
    assert_eq!(
        download.inventory_slots[1]
            .identity
            .as_ref()
            .unwrap()
            .media_key
            .as_deref(),
        Some("7_10000000000000000002")
    );
    assert_eq!(
        download.inventory_slots[3].kind,
        local_media_downloader_lib::domain::x::XMediaKind::AnimatedVideo
    );
    assert_eq!(download.unavailable_count, 3);
}
struct Preview;
impl XPreviewLoader for Preview {
    fn load(
        &self,
        _: &XSourceItem,
        _: &XRequestContext,
    ) -> Result<Option<XPreviewPayload>, AnalysisErrorCode> {
        Ok(Some(XPreviewPayload {
            mime_type: "image/jpeg".into(),
            data_base64: "AQID".into(),
        }))
    }
}
#[test]
fn x_analysis_keeps_unavailable_video_cover_independent_and_snapshots_image_selection() {
    let service = XAnalysisService::new(Inventory, NoVideo, Preview);
    let now = Instant::now();
    let analysis = service
        .analyze(
            "https://twitter.com/example/status/12345678901234567890?tracking=1",
            now,
        )
        .unwrap();
    assert_eq!(analysis.items.len(), 4);
    assert_eq!(
        analysis.items[1].availability,
        XItemAvailability::Unavailable
    );
    assert!(analysis.items[1].preview_available);
    assert!(service
        .load_preview(&analysis.analysis_id, &analysis.items[1].item_id, now)
        .is_ok());
    assert!(service
        .resolve_download(
            &analysis.analysis_id,
            XDownloadIntent {
                item_ids: vec![analysis.items[1].item_id.clone()],
                compatibility_mode: false
            },
            now
        )
        .is_err());
    let download = service
        .resolve_download(
            &analysis.analysis_id,
            XDownloadIntent {
                item_ids: vec![
                    analysis.items[2].item_id.clone(),
                    analysis.items[0].item_id.clone(),
                ],
                compatibility_mode: false,
            },
            now,
        )
        .unwrap();
    assert_eq!(download.items[0].public.ordinal, 1);
    assert_eq!(download.items[1].public.ordinal, 3);
    assert_eq!(download.post_item_count, 4);
    service.clear();
    assert!(service.resolve_source(&analysis.analysis_id, now).is_err());
    assert_eq!(download.items[0].identity.media_id, "10000000000000000001");
}
#[test]
fn x_analysis_expires_and_rejects_non_x_input_without_foreign_side_effects() {
    let service = XAnalysisService::new(Inventory, NoVideo, Preview);
    let now = Instant::now();
    let preview = service
        .analyze("https://x.com/i/web/status/12345678901234567890", now)
        .unwrap();
    assert!(matches!(
        service.resolve_source(&preview.analysis_id, now + Duration::from_secs(901)),
        Err(AnalysisErrorCode::AnalysisExpired)
    ));
    assert!(service
        .analyze("https://youtu.be/demoYT00001", now)
        .is_err());
    assert!(validate_media_url(&preview.source_page_url).is_ok());
}

#[test]
fn x_expiry_releases_unused_network_but_submitted_snapshot_keeps_its_original_owner() {
    use local_media_downloader_lib::application::{
        network_profile::NetworkProfile, process::CancellationToken,
    };
    use std::sync::{Mutex, Weak};
    struct Watched(Arc<Mutex<Option<Weak<NetworkProfile>>>>);
    impl XInventoryExtractor for Watched {
        fn extract_post(
            &self,
            source: &ValidatedMediaUrl,
            context: &XRequestContext,
        ) -> Result<XInventory, AnalysisErrorCode> {
            *self.0.lock().unwrap() = Some(Arc::downgrade(&context.network));
            parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), source)
        }
    }
    for submit in [false, true] {
        let watched = Arc::new(Mutex::new(None));
        let service = XAnalysisService::new(Watched(watched.clone()), NoVideo, Preview);
        let now = Instant::now();
        let preview = service
            .analyze("https://x.com/i/web/status/12345678901234567890", now)
            .unwrap();
        let retained = submit.then(|| {
            service
                .resolve_download(
                    &preview.analysis_id,
                    XDownloadIntent {
                        item_ids: vec![preview.items[0].item_id.clone()],
                        compatibility_mode: false,
                    },
                    now,
                )
                .unwrap()
        });
        let original = watched.lock().unwrap().take().unwrap();
        assert!(matches!(
            service.resolve_source(&preview.analysis_id, now + Duration::from_secs(901)),
            Err(AnalysisErrorCode::AnalysisExpired)
        ));
        if let Some(download) = retained.as_ref() {
            let next_context = XRequestContext::new(
                CancellationToken::new(),
                Instant::now() + Duration::from_secs(15),
                download.network.clone(),
            );
            assert!(Arc::ptr_eq(
                &original.upgrade().unwrap(),
                &next_context.network
            ));
            assert!(!next_context.cancellation.is_canceled());
        } else {
            assert!(
                original.upgrade().is_none(),
                "an expired unused analysis must release its native network resources"
            );
        }
        drop(retained);
        assert!(
            original.upgrade().is_none(),
            "only submitted sources may extend the expired session's network lifetime"
        );
    }
}
#[test]
fn x_stale_analysis_cannot_publish_after_clear() {
    use std::sync::{mpsc, Mutex};
    struct Blocking {
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl XInventoryExtractor for Blocking {
        fn extract_post(
            &self,
            s: &ValidatedMediaUrl,
            _: &XRequestContext,
        ) -> Result<XInventory, AnalysisErrorCode> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            parse_x_inventory(include_bytes!("../../fixtures/x/mixed-post.json"), s)
        }
    }
    let (entered, receive) = mpsc::sync_channel(1);
    let (release, unblock) = mpsc::sync_channel(1);
    let service = Arc::new(XAnalysisService::new(
        Blocking {
            entered,
            release: Mutex::new(unblock),
        },
        NoVideo,
        Preview,
    ));
    let worker = service.clone();
    let thread = std::thread::spawn(move || {
        worker.analyze(
            "https://x.com/i/web/status/12345678901234567890",
            Instant::now(),
        )
    });
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    service.clear();
    release.send(()).unwrap();
    assert!(thread.join().unwrap().is_err());
}

#[test]
fn x_failed_preview_does_not_disable_ready_downloads() {
    struct FailedPreview;
    impl XPreviewLoader for FailedPreview {
        fn load(
            &self,
            _: &XSourceItem,
            _: &XRequestContext,
        ) -> Result<Option<XPreviewPayload>, AnalysisErrorCode> {
            Err(AnalysisErrorCode::NetworkFailed)
        }
    }
    let service = XAnalysisService::new(Inventory, NoVideo, FailedPreview);
    let now = Instant::now();
    let preview = service
        .analyze("https://x.com/i/web/status/12345678901234567890", now)
        .unwrap();
    let item = &preview.items[0];
    assert!(service
        .load_preview(&preview.analysis_id, &item.item_id, now)
        .is_err());
    assert!(service
        .resolve_download(
            &preview.analysis_id,
            XDownloadIntent {
                item_ids: vec![item.item_id.clone()],
                compatibility_mode: false
            },
            now
        )
        .is_ok());
}
