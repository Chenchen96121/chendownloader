#![cfg(feature = "instagram-browser-probe")]

use local_media_downloader_lib::{
    application::media_url::validate_media_url,
    domain::{instagram::InstagramMediaKind, media::AnalysisErrorCode},
    infrastructure::instagram_browser::parse_browser_capture,
};
use serde_json::{json, Value};

const GRAPHQL: &[u8] = include_bytes!("../../tests/fixtures/instagram-browser/graphql-mixed.json");
const REST: &[u8] = include_bytes!("../../tests/fixtures/instagram-browser/rest-carousel.json");

fn har_entry(url: &str, payload: &[u8]) -> Value {
    json!({
        "request": { "url": url, "headers": [{ "name": "Cookie", "value": "secret-session" }] },
        "response": { "status": 200, "headers": [{ "name": "Set-Cookie", "value": "secret-cookie" }],
            "content": { "mimeType": "application/json", "text": std::str::from_utf8(payload).unwrap() } }
    })
}

#[test]
fn graphql_capture_keeps_mixed_order_and_largest_advertised_image_without_leaking_sources() {
    let source = validate_media_url("https://www.instagram.com/p/Browser_01/?tracking=x").unwrap();
    let report = parse_browser_capture(&source, GRAPHQL).unwrap();
    assert_eq!(report.title, "Browser fixture");
    assert_eq!(report.creator.as_deref(), Some("test_author"));
    assert_eq!(
        report.source_page_url,
        "https://www.instagram.com/p/Browser_01/"
    );
    assert!(report.capture_only);
    assert!(!report.download_enabled);
    assert!(!report.highest_platform_quality_verified);
    assert_eq!(report.items.len(), 3);
    assert_eq!(report.items[0].kind, InstagramMediaKind::Image);
    assert_eq!(
        (report.items[0].width, report.items[0].height),
        (Some(1080), Some(1350))
    );
    assert_eq!(report.items[1].kind, InstagramMediaKind::Video);
    assert_eq!(
        (report.items[1].width, report.items[1].height),
        (Some(1920), Some(1080))
    );
    assert_eq!(report.items[2].container, "webp");
    assert_eq!(
        report
            .items
            .iter()
            .map(|item| item.ordinal)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    let public = serde_json::to_string(&report).unwrap();
    assert!(!public.contains("cdninstagram"));
    assert!(!public.contains("signature"));
    assert!(!public.contains("3599461615698204830"));
}

#[test]
fn rest_capture_ranks_image_and_video_variants_independently_of_input_order() {
    let source = validate_media_url("https://www.instagram.com/p/Browser_02/").unwrap();
    let report = parse_browser_capture(&source, REST).unwrap();
    assert_eq!(report.title, "REST fixture");
    assert_eq!(report.items.len(), 2);
    assert_eq!(
        (report.items[0].width, report.items[0].height),
        (Some(1440), Some(1800))
    );
    assert_eq!(
        (report.items[1].width, report.items[1].height),
        (Some(1920), Some(1080))
    );
    assert_eq!(report.items[1].container, "mp4");
    assert!(report.items.iter().all(|item| item.file_size.is_none()));
}

#[test]
fn har_uses_only_instagram_responses_for_the_requested_post_and_discards_credentials() {
    let source = validate_media_url("https://www.instagram.com/p/Browser_01/").unwrap();
    let har = json!({ "log": { "entries": [
        har_entry("https://malicious.invalid/graphql/query", GRAPHQL),
        har_entry("https://www.instagram.com/api/v1/media/1/info/", REST),
        har_entry("https://www.instagram.com/graphql/query", GRAPHQL)
    ] } });
    let report = parse_browser_capture(&source, &serde_json::to_vec(&har).unwrap()).unwrap();
    assert_eq!(report.items.len(), 3);
    let output = serde_json::to_string(&report).unwrap();
    for secret in [
        "secret-session",
        "secret-cookie",
        "Cookie",
        "malicious.invalid",
    ] {
        assert!(!output.contains(secret));
    }
    let bad_origin = json!({ "log": { "entries": [har_entry("https://instagram.com.malicious.invalid/graphql/query", GRAPHQL)] } });
    assert_eq!(
        parse_browser_capture(&source, &serde_json::to_vec(&bad_origin).unwrap()).err(),
        Some(AnalysisErrorCode::FormatUnavailable)
    );
}

#[test]
fn capture_never_substitutes_a_different_post_or_non_post_route() {
    let source = validate_media_url("https://www.instagram.com/p/Other_01/").unwrap();
    assert_eq!(
        parse_browser_capture(&source, GRAPHQL).err(),
        Some(AnalysisErrorCode::FormatUnavailable)
    );
    let reel = validate_media_url("https://www.instagram.com/reel/Browser_01/").unwrap();
    assert_eq!(
        parse_browser_capture(&reel, GRAPHQL).err(),
        Some(AnalysisErrorCode::UnsupportedMedia)
    );
}

#[test]
fn conflicting_har_snapshots_fail_instead_of_merging_reordered_items() {
    let source = validate_media_url("https://www.instagram.com/p/Browser_01/").unwrap();
    let mut changed: Value = serde_json::from_slice(GRAPHQL).unwrap();
    changed["data"]["xdt_shortcode_media"]["edge_sidecar_to_children"]["edges"]
        .as_array_mut()
        .unwrap()
        .swap(0, 2);
    let har = json!({ "log": { "entries": [
        har_entry("https://www.instagram.com/graphql/query", GRAPHQL),
        har_entry("https://www.instagram.com/graphql/query", &serde_json::to_vec(&changed).unwrap())
    ] } });
    assert_eq!(
        parse_browser_capture(&source, &serde_json::to_vec(&har).unwrap()).err(),
        Some(AnalysisErrorCode::ContentChanged)
    );
}

#[test]
fn numeric_media_ids_keep_all_u64_digits_when_detecting_replacements() {
    let source = validate_media_url("https://www.instagram.com/p/Browser_02/").unwrap();
    let changed = std::str::from_utf8(REST)
        .unwrap()
        .replace("3599461615698204831", "3599461615698204833");
    let har = json!({ "log": { "entries": [
        har_entry("https://www.instagram.com/api/v1/media/1/info/", REST),
        har_entry("https://www.instagram.com/api/v1/media/1/info/", changed.as_bytes())
    ] } });
    assert_eq!(
        parse_browser_capture(&source, &serde_json::to_vec(&har).unwrap()).err(),
        Some(AnalysisErrorCode::ContentChanged)
    );
}

#[test]
fn capture_rejects_incomplete_duplicate_oversized_or_unsafe_media() {
    let source = validate_media_url("https://www.instagram.com/p/Browser_02/").unwrap();
    let mut count: Value = serde_json::from_slice(REST).unwrap();
    count["items"][0]["carousel_media_count"] = json!(3);
    let mut duplicate: Value = serde_json::from_slice(REST).unwrap();
    duplicate["items"][0]["carousel_media"][1]["pk"] = json!(3599461615698204831u64);
    let mut unsafe_url: Value = serde_json::from_slice(REST).unwrap();
    unsafe_url["items"][0]["carousel_media"][0]["image_versions2"]["candidates"][1]["url"] =
        json!("http://127.0.0.1/private.jpg");
    for value in [count, duplicate, unsafe_url] {
        assert_eq!(
            parse_browser_capture(&source, &serde_json::to_vec(&value).unwrap()).err(),
            Some(AnalysisErrorCode::InvalidMetadata)
        );
    }
    assert_eq!(
        parse_browser_capture(&source, &vec![b' '; 16 * 1024 * 1024 + 1]).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
    assert_eq!(
        parse_browser_capture(&source, b"<html>login popup</html>").err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
}

#[test]
fn repeated_identical_har_responses_do_not_duplicate_media() {
    let source = validate_media_url("https://www.instagram.com/p/Browser_01/").unwrap();
    let har = json!({ "log": { "entries": [
        har_entry("https://www.instagram.com/graphql/query", GRAPHQL),
        har_entry("https://www.instagram.com/graphql/query", GRAPHQL)
    ] } });
    assert_eq!(
        parse_browser_capture(&source, &serde_json::to_vec(&har).unwrap())
            .unwrap()
            .items
            .len(),
        3
    );
}

#[test]
fn native_probe_reads_the_selected_file_and_returns_only_the_closed_summary() {
    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/instagram-browser/graphql-mixed.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_instagram-browser-probe"))
        .args([
            "--url",
            "https://www.instagram.com/p/Browser_01/",
            "--input",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert!(output.status.success());
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["items"].as_array().unwrap().len(), 3);
    assert_eq!(summary["items"][0]["width"], 1080);
    assert_eq!(summary["captureOnly"], true);
    assert_eq!(summary["downloadEnabled"], false);
    let public = std::str::from_utf8(&output.stdout).unwrap();
    assert!(!public.contains("signature"));
    assert!(!public.contains("graphql-mixed.json"));

    let mismatch = std::process::Command::new(env!("CARGO_BIN_EXE_instagram-browser-probe"))
        .args(["--url", "https://www.instagram.com/p/Other_01/", "--input"])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(mismatch.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&mismatch.stdout).unwrap();
    assert_eq!(error["error"], "format_unavailable");
    assert_eq!(error["downloadEnabled"], false);
}
