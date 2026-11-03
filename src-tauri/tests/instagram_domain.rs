use local_media_downloader_lib::domain::{
    download::DownloadErrorCode,
    instagram::{
        InstagramAnalysisPreview, InstagramCapabilities, InstagramDownloadIntent,
        InstagramDownloadProgress, InstagramDownloadResult, InstagramItemOutcome,
        InstagramJobState, InstagramMediaItem, InstagramPreviewPayload,
    },
    media::AnalysisErrorCode,
};
use serde_json::{json, Value};

fn capabilities() -> Value {
    json!({
        "postBatch": true,
        "images": true,
        "videos": true,
        "compatibility": true,
        "clip": false,
        "manualQuality": false,
        "audioOnly": false,
        "videoOnly": false
    })
}

fn image_item(ordinal: usize) -> Value {
    json!({
        "itemId": ordinal.to_string(),
        "ordinal": ordinal,
        "kind": "image",
        "width": 1080,
        "height": 1350,
        "container": "jpg",
        "fileSize": { "bytes": 1234, "isApproximate": false },
        "previewAvailable": true,
        "availability": "ready"
    })
}

fn unavailable_video_item(ordinal: usize) -> Value {
    json!({
        "itemId": ordinal.to_string(),
        "ordinal": ordinal,
        "kind": "video",
        "width": 1080,
        "height": 1920,
        "container": "mp4",
        "fps": 30.0,
        "videoBitrateKbps": 12_000.0,
        "audioBitrateKbps": 160.0,
        "videoCodec": "h264",
        "audioCodec": "aac",
        "dynamicRange": "HDR",
        "previewAvailable": false,
        "availability": "unavailable",
        "unavailableReason": "content_changed"
    })
}

fn post_preview(items: Vec<Value>) -> Value {
    json!({
        "analysisId": "1",
        "sourceKind": "post",
        "shortcode": "Abc_123-",
        "title": "公开帖子",
        "creator": "creator_name",
        "sourcePageUrl": "https://www.instagram.com/p/Abc_123-/",
        "capabilities": capabilities(),
        "items": items
    })
}

fn parses<T: serde::de::DeserializeOwned>(value: Value) -> bool {
    serde_json::from_value::<T>(value).is_ok()
}

#[test]
fn analysis_preview_accepts_one_to_thirty_two_ordered_safe_items() {
    let preview: InstagramAnalysisPreview =
        serde_json::from_value(post_preview((1..=32).map(image_item).collect())).unwrap();
    assert_eq!(preview.items.len(), 32);
    assert_eq!(
        preview.source_page_url,
        "https://www.instagram.com/p/Abc_123-/"
    );
    assert!(preview.items[0].file_size.is_some());
    assert!(parses::<InstagramAnalysisPreview>(post_preview(vec![
        unavailable_video_item(1)
    ])));

    let unknown_size = post_preview(vec![json!({
        "itemId": "1",
        "ordinal": 1,
        "kind": "image",
        "container": "webp",
        "previewAvailable": false,
        "availability": "ready"
    })]);
    let encoded = serde_json::to_value(
        serde_json::from_value::<InstagramAnalysisPreview>(unknown_size).unwrap(),
    )
    .unwrap();
    assert!(encoded["items"][0].get("fileSize").is_none());
}

#[test]
fn analysis_preview_rejects_count_order_identity_and_availability_mismatches() {
    assert!(!parses::<InstagramAnalysisPreview>(post_preview(vec![])));
    assert!(!parses::<InstagramAnalysisPreview>(post_preview(
        (1..=33).map(image_item).collect(),
    )));

    let mut gap = image_item(1);
    gap["ordinal"] = json!(2);
    assert!(!parses::<InstagramAnalysisPreview>(post_preview(vec![gap])));

    assert!(!parses::<InstagramAnalysisPreview>(post_preview(vec![
        image_item(1),
        {
            let mut row = image_item(2);
            row["itemId"] = json!("1");
            row
        },
    ])));

    let mut ready_with_reason = image_item(1);
    ready_with_reason["unavailableReason"] = json!("content_changed");
    assert!(!parses::<InstagramAnalysisPreview>(post_preview(vec![
        ready_with_reason
    ])));

    let mut unavailable_without_reason = unavailable_video_item(1);
    unavailable_without_reason
        .as_object_mut()
        .unwrap()
        .remove("unavailableReason");
    assert!(!parses::<InstagramAnalysisPreview>(post_preview(vec![
        unavailable_without_reason
    ])));

    let mut image_with_video_metrics = image_item(1);
    image_with_video_metrics["dynamicRange"] = json!("HDR");
    assert!(!parses::<InstagramAnalysisPreview>(post_preview(vec![
        image_with_video_metrics
    ])));
    assert!(!parses::<InstagramMediaItem>(json!({
        "itemId": "1", "ordinal": 1, "kind": "image", "container": "jpg",
        "videoBitrateKbps": 1.0, "previewAvailable": true, "availability": "ready"
    })));

    for (field, invalid) in [
        ("videoBitrateKbps", json!(0.0)),
        ("audioBitrateKbps", json!(100_000.1)),
        ("dynamicRange", json!("HDR99")),
    ] {
        let mut video = unavailable_video_item(1);
        video[field] = invalid;
        assert!(
            !parses::<InstagramAnalysisPreview>(post_preview(vec![video])),
            "{field}"
        );
    }
}

#[test]
fn analysis_preview_requires_exact_capabilities_canonical_page_and_closed_fields() {
    for key in [
        "postBatch",
        "images",
        "videos",
        "compatibility",
        "clip",
        "manualQuality",
        "audioOnly",
        "videoOnly",
    ] {
        let mut value = post_preview(vec![image_item(1)]);
        value["capabilities"][key] = json!(!value["capabilities"][key].as_bool().unwrap());
        assert!(!parses::<InstagramAnalysisPreview>(value), "{key}");
    }
    assert!(!parses::<InstagramCapabilities>(json!({
        "postBatch": false, "images": true, "videos": true, "compatibility": true,
        "clip": false, "manualQuality": false, "audioOnly": false, "videoOnly": false
    })));

    for source_page_url in [
        "https://www.instagram.com/p/Abc_123-/?token=secret",
        "https://cdninstagram.invalid/signed.jpg",
        "C:\\private\\preview.jpg",
    ] {
        let mut value = post_preview(vec![image_item(1)]);
        value["sourcePageUrl"] = json!(source_page_url);
        assert!(
            !parses::<InstagramAnalysisPreview>(value),
            "{source_page_url}"
        );
    }

    for (field, injected) in [
        ("rawMediaUrl", json!("https://cdn.invalid/signed")),
        ("cookie", json!("session=private")),
        ("absolutePath", json!("C:\\private\\file.jpg")),
    ] {
        let mut value = post_preview(vec![image_item(1)]);
        value
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), injected);
        assert!(!parses::<InstagramAnalysisPreview>(value), "{field}");
    }

    let reel = json!({
        "analysisId": "2",
        "sourceKind": "reel",
        "shortcode": "Reel_1",
        "title": "Reel",
        "sourcePageUrl": "https://www.instagram.com/reel/Reel_1/",
        "capabilities": capabilities(),
        "items": [{
            "itemId": "1", "ordinal": 1, "kind": "video", "container": "mp4",
            "previewAvailable": true, "availability": "ready"
        }]
    });
    assert!(parses::<InstagramAnalysisPreview>(reel));
    let invalid_reel = {
        let mut value = post_preview(vec![image_item(1)]);
        value["sourceKind"] = json!("reel");
        value["sourcePageUrl"] = json!("https://www.instagram.com/reel/Abc_123-/");
        value
    };
    assert!(!parses::<InstagramAnalysisPreview>(invalid_reel));
}

#[test]
fn download_intent_is_closed_nonempty_unique_and_bounded() {
    assert!(parses::<InstagramDownloadIntent>(json!({
        "itemIds": (1..=32).map(|value| value.to_string()).collect::<Vec<_>>(),
        "compatibilityMode": true
    })));
    for value in [
        json!({ "itemIds": [], "compatibilityMode": false }),
        json!({ "itemIds": ["1", "1"], "compatibilityMode": false }),
        json!({ "itemIds": (1..=33).map(|value| value.to_string()).collect::<Vec<_>>(), "compatibilityMode": false }),
        json!({ "itemIds": ["https://cdn.invalid/signed"], "compatibilityMode": false }),
        json!({ "itemIds": ["1"], "compatibilityMode": false, "sourceUrl": "https://cdn.invalid" }),
        json!({ "itemIds": ["1"], "compatibilityMode": false, "formatId": "best" }),
        json!({ "itemIds": ["1"], "compatibilityMode": false, "path": "C:\\private" }),
    ] {
        assert!(!parses::<InstagramDownloadIntent>(value));
    }
}

#[test]
fn preview_payload_accepts_only_bounded_inline_image_data() {
    for mime in ["image/jpeg", "image/png", "image/webp"] {
        assert!(parses::<InstagramPreviewPayload>(json!({
            "mimeType": mime,
            "dataBase64": "AQID"
        })));
    }
    for value in [
        json!({ "mimeType": "image/gif", "dataBase64": "AQID" }),
        json!({ "mimeType": "image/jpeg", "dataBase64": "https://cdn.invalid/a.jpg" }),
        json!({ "mimeType": "image/jpeg", "dataBase64": "AQID", "path": "C:\\private" }),
    ] {
        assert!(!parses::<InstagramPreviewPayload>(value));
    }
}

#[test]
fn progress_outcomes_and_results_enforce_closed_state_and_count_invariants() {
    for (state, wire) in [
        (InstagramJobState::Queued, "queued"),
        (InstagramJobState::Downloading, "downloading"),
        (InstagramJobState::Processing, "processing"),
        (InstagramJobState::Verifying, "verifying"),
        (InstagramJobState::Publishing, "publishing"),
        (InstagramJobState::Completed, "completed"),
        (InstagramJobState::Partial, "partial"),
        (InstagramJobState::Failed, "failed"),
        (InstagramJobState::Canceled, "canceled"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), json!(wire));
    }

    assert!(parses::<InstagramDownloadProgress>(json!({
        "jobId": "1", "state": "downloading", "itemIndex": 1, "itemCount": 2,
        "downloadedBytes": 5, "totalBytes": 10, "percent": 50.0
    })));
    assert!(!parses::<InstagramDownloadProgress>(json!({
        "jobId": "1", "state": "downloading", "itemIndex": 3, "itemCount": 2,
        "downloadedBytes": null, "totalBytes": null, "percent": 50.0
    })));
    assert!(!parses::<InstagramDownloadProgress>(json!({
        "jobId": "1", "state": "downloading", "itemIndex": 1, "itemCount": 2,
        "totalBytes": null, "percent": null
    })));
    assert!(!parses::<InstagramItemOutcome>(json!({
        "itemId": "1", "ordinal": 1, "kind": "image", "state": "completed"
    })));

    let partial = json!({
        "jobId": "1",
        "state": "partial",
        "completedCount": 1,
        "failedCount": 1,
        "folderDisplayName": "creator - Abc_123-",
        "items": [
            { "itemId": "1", "ordinal": 1, "kind": "image", "state": "completed", "displayName": "01.jpg" },
            { "itemId": "2", "ordinal": 2, "kind": "video", "state": "failed", "errorCode": "content_changed" }
        ]
    });
    assert!(parses::<InstagramDownloadResult>(partial.clone()));
    assert!(parses::<InstagramDownloadResult>(json!({
        "jobId": "2",
        "state": "completed",
        "completedCount": 2,
        "failedCount": 0,
        "folderDisplayName": "creator - Abc_123-",
        "items": [
            { "itemId": "1", "ordinal": 1, "kind": "image", "state": "completed", "displayName": "01.jpg" },
            { "itemId": "3", "ordinal": 3, "kind": "image", "state": "completed", "displayName": "03.jpg" }
        ]
    })));
    assert!(!parses::<InstagramDownloadResult>(json!({
        "jobId": "3",
        "state": "completed",
        "completedCount": 2,
        "failedCount": 0,
        "folderDisplayName": "creator - Abc_123-",
        "items": [
            { "itemId": "3", "ordinal": 3, "kind": "image", "state": "completed", "displayName": "03.jpg" },
            { "itemId": "1", "ordinal": 1, "kind": "image", "state": "completed", "displayName": "01.jpg" }
        ]
    })));
    let mut wrong_count = partial.clone();
    wrong_count["completedCount"] = json!(2);
    assert!(!parses::<InstagramDownloadResult>(wrong_count));
    let mut raw_path = partial;
    raw_path["items"][0]["displayName"] = json!("C:\\private\\01.jpg");
    assert!(!parses::<InstagramDownloadResult>(raw_path));
}

#[test]
fn content_changed_is_a_stable_analysis_and_download_error() {
    assert_eq!(
        serde_json::to_value(AnalysisErrorCode::ContentChanged).unwrap(),
        json!("content_changed")
    );
    assert_eq!(
        serde_json::to_value(DownloadErrorCode::ContentChanged).unwrap(),
        json!("content_changed")
    );
}
