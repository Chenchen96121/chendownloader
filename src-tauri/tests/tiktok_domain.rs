use local_media_downloader_lib::{
    application::media_url::validate_media_url,
    domain::{media::Platform, tiktok::TikTokAnalysisPreview},
    infrastructure::tiktok::{
        inventory::parse_tiktok_inventory,
        url_policy::{normalize_tiktok_input, validate_tiktok_asset_url},
    },
};
use serde_json::json;

pub fn fixture() -> serde_json::Value {
    json!({"postId":"1234567890123456789","title":"Public photos","creator":"public","duration":null,"context":{"cookies":[]},"sourcePageUrl":"https://www.tiktok.com/@public/photo/1234567890123456789","items":(0..13).map(|i|json!({"itemId":format!("asset-{i}"),"identity":format!("original-{i}"),"ordinal":i+1,"kind":"image","width":2160,"height":2880,"container":"jpg","urls":[format!("https://p16.tiktokcdn.com/original-{i}?sig=synthetic")],"previewUrls":[format!("https://p16.tiktokcdn.com/preview-{i}?sig=synthetic")],"candidates":[]})).collect::<Vec<_>>()})
}

#[test]
fn accepts_canonical_video_photo_and_share_text_without_tracking() {
    let url = normalize_tiktok_input(
        "分享 https://www.tiktok.com/@public/photo/1234567890123456789?web_id=42",
    )
    .unwrap();
    assert_eq!(
        url,
        "https://www.tiktok.com/@public/photo/1234567890123456789"
    );
    let source = validate_media_url(&url).unwrap();
    assert_eq!(source.platform(), Platform::Tiktok);
    assert_eq!(source.media_id(), "1234567890123456789");
    assert_eq!(
        normalize_tiktok_input("https://vm.tiktok.com/ABC/").unwrap(),
        "https://vm.tiktok.com/ABC/"
    );
}

#[test]
fn unsafe_source_and_cdn_destinations_never_pass() {
    for raw in [
        "https://127.0.0.1/a",
        "http://www.tiktok.com/@public/video/1",
        "https://www.tiktok.com.evil.test/@public/video/1",
        "https://user@www.tiktok.com/@public/video/1",
        "https://www.tiktok.com:444/@public/video/1",
        "https://www.tiktok.com/@public/video/%31",
        "https://www.tiktok.com/../@public/video/1",
    ] {
        assert!(normalize_tiktok_input(raw).is_err(), "{raw}");
    }
    for raw in [
        "http://p16.tiktokcdn.com/x",
        "https://tiktokcdn.com.evil.test/x",
        "https://localhost/x",
        "https://www.tiktok.com/api/item/detail/",
    ] {
        assert!(validate_tiktok_asset_url(raw).is_err(), "{raw}");
    }
    assert!(validate_tiktok_asset_url("https://v16-webapp-prime.tiktok.com/file").is_ok());
}

#[test]
fn parses_all_original_images_and_rejects_wrong_post_or_untrusted_urls() {
    let source =
        validate_media_url("https://www.tiktok.com/@public/photo/1234567890123456789").unwrap();
    let data = fixture();
    let inventory = parse_tiktok_inventory(&data, &source).unwrap();
    assert_eq!(inventory.items.len(), 13);
    assert_eq!(inventory.items[12].ordinal, 13);
    assert_eq!(inventory.items[0].width, Some(2160));
    let mut changed = data.clone();
    changed["postId"] = json!("2");
    assert!(parse_tiktok_inventory(&changed, &source).is_err());
    let mut renamed = data.clone();
    renamed["sourcePageUrl"] = json!("https://www.tiktok.com/@renamed/photo/1234567890123456789");
    assert!(parse_tiktok_inventory(&renamed, &source).is_ok());
    let mut changed = data.clone();
    changed["items"][0]["urls"] = json!(["https://127.0.0.1/x"]);
    assert!(parse_tiktok_inventory(&changed, &source).is_err());
    let mut changed = data;
    changed["items"][0]["ordinal"] = json!(2);
    assert!(parse_tiktok_inventory(&changed, &source).is_err());
}

#[test]
fn public_contract_rejects_private_fields_and_wrong_platform() {
    let value = json!({"analysisId":"1","postId":"1234567890123456789","title":"Public post","sourcePageUrl":"https://www.tiktok.com/@public/photo/1234567890123456789","capabilities":{"postBatch":true,"images":true,"videos":true,"compatibility":true,"clip":false,"manualQuality":false,"audioOnly":false,"videoOnly":false},"items":[{"itemId":"1","ordinal":1,"kind":"image","container":"jpg","previewAvailable":true,"availability":"ready","width":2160,"height":2880}]});
    assert!(serde_json::from_value::<TikTokAnalysisPreview>(value.clone()).is_ok());
    let mut bad = value.clone();
    bad["cookies"] = json!("secret");
    assert!(serde_json::from_value::<TikTokAnalysisPreview>(bad).is_err());
    let mut bad = value;
    bad["sourcePageUrl"] = json!("https://x.com/i/web/status/1234567890123456789");
    assert!(serde_json::from_value::<TikTokAnalysisPreview>(bad).is_err());
}
