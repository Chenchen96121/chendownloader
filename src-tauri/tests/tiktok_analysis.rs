use local_media_downloader_lib::{
    application::{
        media_url::{validate_media_url, ValidatedMediaUrl},
        tiktok_analysis::{TikTokAnalysisService, TikTokPreviewLoader},
        tiktok_source::{
            TikTokInventory, TikTokInventoryExtractor, TikTokRequestContext, TikTokSourceItem,
        },
        tiktok_video::{resolve_tiktok_video, NativeTikTokVideoAnalyzer},
    },
    domain::{
        media::AnalysisErrorCode,
        tiktok::{TikTokDownloadIntent, TikTokItemAvailability, TikTokPreviewPayload},
    },
    infrastructure::tiktok::inventory::parse_tiktok_inventory,
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

struct Inventory;
impl TikTokInventoryExtractor for Inventory {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        _: &TikTokRequestContext,
    ) -> Result<TikTokInventory, AnalysisErrorCode> {
        parse_tiktok_inventory(
            &json!({"postId":source.media_id(),"title":"Public post","creator":"public","duration":12,"context":{"cookies":[]},"sourcePageUrl":source.url(),"items":[{"itemId":"asset-1","identity":"native-video","ordinal":1,"kind":"video","width":1080,"height":1920,"container":"mp4","urls":["https://v16-webapp-prime.tiktok.com/original"],"previewUrls":["https://p16.tiktokcdn.com/cover"],"candidates":[{"identity":"large","urls":["https://v16-webapp-prime.tiktok.com/original"],"width":1080,"height":1920,"fps":30,"videoCodec":"hevc","audioCodec":"aac","audioBitrateKbps":64,"videoBitrateKbps":1300,"fileSize":{"bytes":2100,"isApproximate":false}},{"identity":"small","urls":["https://v16-webapp-prime.tiktok.com/small"],"width":720,"height":1280,"fps":30,"videoCodec":"h264","audioCodec":"aac","audioBitrateKbps":64,"videoBitrateKbps":2000,"fileSize":{"bytes":2000,"isApproximate":false}}]}]}),
            source,
        )
    }
}
struct Preview;
impl TikTokPreviewLoader for Preview {
    fn load(
        &self,
        _: &TikTokSourceItem,
        _: &TikTokRequestContext,
    ) -> Result<Option<TikTokPreviewPayload>, AnalysisErrorCode> {
        Err(AnalysisErrorCode::NetworkFailed)
    }
}
fn service() -> TikTokAnalysisService<Inventory, NativeTikTokVideoAnalyzer, Preview> {
    TikTokAnalysisService::new(Inventory, NativeTikTokVideoAnalyzer, Preview)
}

#[test]
fn highest_quality_is_resolution_first_and_preview_failure_does_not_disable_download() {
    let service = service();
    let now = Instant::now();
    let preview = service
        .analyze(
            "https://www.tiktok.com/@public/video/1234567890123456789",
            now,
        )
        .unwrap();
    assert_eq!(preview.items[0].availability, TikTokItemAvailability::Ready);
    assert_eq!(preview.items[0].width, Some(1080));
    assert_eq!(preview.items[0].fps, Some(30.0));
    assert_eq!(preview.items[0].video_codec.as_deref(), Some("hevc"));
    assert_eq!(
        service.load_preview(&preview.analysis_id, &preview.items[0].item_id, now),
        Err(AnalysisErrorCode::NetworkFailed)
    );
    assert!(service
        .resolve_download(
            &preview.analysis_id,
            TikTokDownloadIntent {
                item_ids: vec![preview.items[0].item_id.clone()],
                compatibility_mode: false
            },
            now
        )
        .is_ok());
}

#[test]
fn queued_identity_survives_new_analysis_and_shutdown_is_terminal() {
    let service = service();
    let now = Instant::now();
    let first = service
        .analyze(
            "https://www.tiktok.com/@public/video/1234567890123456789",
            now,
        )
        .unwrap();
    let queued = service
        .resolve_download(
            &first.analysis_id,
            TikTokDownloadIntent {
                item_ids: vec![first.items[0].item_id.clone()],
                compatibility_mode: false,
            },
            now,
        )
        .unwrap();
    let second = service
        .analyze(
            "https://www.tiktok.com/@public/video/2234567890123456789",
            now,
        )
        .unwrap();
    assert_eq!(queued.source.media_id(), "1234567890123456789");
    assert_ne!(first.analysis_id, second.analysis_id);
    assert!(service.resolve_source(&first.analysis_id, now).is_err());
    assert!(service
        .resolve_source(&second.analysis_id, now + Duration::from_secs(901))
        .is_err());
    service.shutdown();
    assert!(service
        .analyze(
            "https://www.tiktok.com/@public/video/1234567890123456789",
            now
        )
        .is_err());
}

#[test]
fn duplicate_or_foreign_selected_items_are_rejected() {
    let service = service();
    let now = Instant::now();
    let preview = service
        .analyze(
            "https://www.tiktok.com/@public/video/1234567890123456789",
            now,
        )
        .unwrap();
    let id = preview.items[0].item_id.clone();
    for ids in [vec![], vec![id.clone(), id], vec!["foreign".into()]] {
        assert!(service
            .resolve_download(
                &preview.analysis_id,
                TikTokDownloadIntent {
                    item_ids: ids,
                    compatibility_mode: false
                },
                now
            )
            .is_err());
    }
}

#[test]
fn compatibility_is_explicit_and_no_background_music_is_synthesized() {
    use local_media_downloader_lib::application::{
        network_profile::NetworkProfile, process::CancellationToken,
    };
    let source =
        validate_media_url("https://www.tiktok.com/@public/video/1234567890123456789").unwrap();
    let context = TikTokRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let mut inventory = Inventory.extract_post(&source, &context).unwrap();
    let identity = inventory.items[0].identity.clone().unwrap();
    let original = resolve_tiktok_video(&inventory, &identity, false, &context).unwrap();
    assert!(!original.plan().compatibility_mode);
    let compatible = resolve_tiktok_video(&inventory, &identity, true, &context).unwrap();
    assert!(compatible.plan().compatibility_mode);
    let mut native = inventory.items[0].native.as_ref().unwrap().data.clone();
    for row in native["candidates"].as_array_mut().unwrap() {
        row["audioCodec"] = serde_json::Value::Null;
    }
    inventory.items[0].native = Some(Arc::new(
        local_media_downloader_lib::application::tiktok_source::TikTokNativeAsset {
            data: native,
            context: json!({"cookies":[]}),
        },
    ));
    assert!(resolve_tiktok_video(&inventory, &identity, false, &context).is_err());
    // Only a positive no-audio declaration may produce a silent download.
    let mut native = inventory.items[0].native.as_ref().unwrap().data.clone();
    for row in native["candidates"].as_array_mut().unwrap() {
        row["audioAbsent"] = true.into();
        row.as_object_mut().unwrap().remove("audioBitrateKbps");
    }
    inventory.items[0].native = Some(Arc::new(
        local_media_downloader_lib::application::tiktok_source::TikTokNativeAsset {
            data: native,
            context: json!({"cookies":[]}),
        },
    ));
    let silent = resolve_tiktok_video(&inventory, &identity, false, &context).unwrap();
    assert!(silent.plan().audio_format_id.is_none());
    assert!(silent.plan().output_audio_codec.is_none());
}
