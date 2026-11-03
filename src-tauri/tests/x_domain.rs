use local_media_downloader_lib::domain::x::{XAnalysisPreview, XDownloadIntent, XDownloadResult};
use serde_json::{json, Value};

fn analysis() -> Value {
    json!({"analysisId":"1","postId":"12345678901234567890","title":"X post",
        "sourcePageUrl":"https://x.com/i/web/status/12345678901234567890",
        "capabilities":{"postBatch":true,"images":true,"videos":true,"compatibility":true,"clip":false,"manualQuality":false,"audioOnly":false,"videoOnly":false},
        "items":[{"itemId":"2","ordinal":1,"kind":"animated_video","container":"mp4","previewAvailable":false,"availability":"ready"}]})
}
#[test]
fn x_analysis_accepts_ordered_silent_media_but_rejects_manual_controls() {
    assert!(serde_json::from_value::<XAnalysisPreview>(analysis()).is_ok());
    for field in ["clip", "manualQuality", "audioOnly", "videoOnly"] {
        let mut value = analysis();
        value["capabilities"][field] = json!(true);
        assert!(
            serde_json::from_value::<XAnalysisPreview>(value).is_err(),
            "{field}"
        );
    }
    let mut value = analysis();
    value["rawUrl"] = json!("https://video.twimg.com/a.mp4");
    assert!(serde_json::from_value::<XAnalysisPreview>(value).is_err());
    let mut value = analysis();
    value["items"] = json!([]);
    assert!(serde_json::from_value::<XAnalysisPreview>(value).is_err());
}
#[test]
fn x_intent_rejects_duplicate_ids_and_any_quality_or_clip_parameter() {
    assert!(serde_json::from_value::<XDownloadIntent>(
        json!({"itemIds":["1"],"compatibilityMode":false})
    )
    .is_ok());
    for extra in [
        json!({"clip":{}}),
        json!({"resolution":1080}),
        json!({"formatId":"best"}),
    ] {
        let mut value = json!({"itemIds":["1"],"compatibilityMode":false});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(serde_json::from_value::<XDownloadIntent>(value).is_err());
    }
    assert!(serde_json::from_value::<XDownloadIntent>(
        json!({"itemIds":["1","1"],"compatibilityMode":false})
    )
    .is_err());
}
#[test]
fn x_result_keeps_original_post_counts_and_content_change_flag() {
    let value = json!({"jobId":"1","state":"completed","postItemCount":3,"unavailableCount":1,"contentChanged":false,"completedCount":1,"failedCount":0,"folderDisplayName":"X post","items":[{"itemId":"2","ordinal":2,"kind":"video","state":"completed","displayName":"02.mp4"}]});
    assert!(serde_json::from_value::<XDownloadResult>(value.clone()).is_ok());
    let mut invalid = value;
    invalid["unavailableCount"] = json!(4);
    assert!(serde_json::from_value::<XDownloadResult>(invalid).is_err());
}
