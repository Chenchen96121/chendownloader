use local_media_downloader_lib::{
    application::media_url::validate_media_url,
    domain::{
        media::AnalysisErrorCode,
        x::{XItemAvailability, XMediaKind},
    },
    infrastructure::x::{
        inventory::parse_x_inventory,
        url_policy::{validate_x_asset_url, XAssetKind},
    },
};
use serde_json::{json, Value};
const MIXED: &[u8] = include_bytes!("../../fixtures/x/mixed-post.json");
fn parse(
    bytes: &[u8],
) -> Result<local_media_downloader_lib::application::x_source::XInventory, AnalysisErrorCode> {
    parse_x_inventory(
        bytes,
        &validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap(),
    )
}
#[test]
fn x_raw_inventory_preserves_all_entity_ids_order_and_quality_candidates() {
    let inventory = parse(MIXED).unwrap();
    assert_eq!(inventory.items.len(), 4);
    assert_eq!(
        inventory.items[1].identity.as_ref().unwrap().media_id,
        "10000000000000000002"
    );
    assert_eq!(inventory.items[1].video_candidates.len(), 2);
    assert_eq!(inventory.items[1].ordinal, 2);
    assert_eq!(inventory.items[3].kind, XMediaKind::AnimatedVideo);
    assert!(inventory.items[0].image_candidates[0]
        .url
        .as_str()
        .ends_with("?format=jpg&name=orig"));
}
#[test]
fn x_inventory_retains_unavailable_and_identity_missing_entities() {
    let i = parse(include_bytes!("../../fixtures/x/all-unavailable.json")).unwrap();
    assert_eq!(i.items.len(), 1);
    assert_eq!(i.items[0].availability, XItemAvailability::Unavailable);
    let mut raw: Value = serde_json::from_slice(MIXED).unwrap();
    raw[0][1]["legacy"]["extended_entities"]["media"][0]
        .as_object_mut()
        .unwrap()
        .remove("id_str");
    let i = parse(&serde_json::to_vec(&raw).unwrap()).unwrap();
    assert!(i.items[0].identity.is_none());
    assert_eq!(i.items[0].availability, XItemAvailability::Unavailable);
    assert!(i.items[0].image_candidates.is_empty());
}
#[test]
fn x_inventory_rejects_error_records_wrong_parent_duplicates_and_truncation() {
    assert!(matches!(
        parse(include_bytes!("../../fixtures/x/embedded-error.json")),
        Err(AnalysisErrorCode::AuthenticationRequired)
    ));
    for mutate in [0, 1, 2, 3] {
        let mut raw: Value = serde_json::from_slice(MIXED).unwrap();
        match mutate {
            0 => raw[0][1]["rest_id"] = json!("9"),
            1 => {
                raw[0][1]["legacy"]["extended_entities"]["media"][1]["id_str"] =
                    json!("10000000000000000001")
            }
            2 => {
                let item = raw[0][1]["legacy"]["extended_entities"]["media"][0].clone();
                raw[0][1]["legacy"]["extended_entities"]["media"] = Value::Array(vec![item; 33]);
            }
            _ => raw[0][1]["legacy"]["extended_entities"]["media"] = json!([]),
        }
        assert!(parse(&serde_json::to_vec(&raw).unwrap()).is_err());
    }
    assert!(parse(b"[[2,").is_err());
}
#[test]
fn x_cdn_policy_rejects_nonliteral_https_hosts_credentials_and_ports() {
    assert!(validate_x_asset_url("https://pbs.twimg.com/media/a.jpg", XAssetKind::Image).is_ok());
    assert!(validate_x_asset_url(
        "https://video.twimg.com/tweet_video/a.mp4",
        XAssetKind::Video
    )
    .is_ok());
    for raw in [
        "https://video.twimg.com.evil.test/a.mp4",
        "https://video.twimg.com:443/a.mp4",
        "https://user@video.twimg.com/a.mp4",
        "file:///a.mp4",
        "https://127.0.0.1/a.mp4",
        "https://video.twimg.com/a/../b.mp4",
    ] {
        assert!(
            validate_x_asset_url(raw, XAssetKind::Video).is_err(),
            "{raw}"
        );
    }
    assert!(validate_x_asset_url("https://pbs.twimg.com/media/a.jpg", XAssetKind::Video).is_err());
}
