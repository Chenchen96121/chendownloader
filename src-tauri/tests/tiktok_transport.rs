use local_media_downloader_lib::{
    application::media_url::validate_media_url,
    infrastructure::ytdlp::tiktok::{inventory_from_ytdlp, merge_video_candidates},
};
use serde_json::json;

#[test]
fn merge_excludes_background_music_and_keeps_higher_resolution_extractor_candidate() {
    let item = json!({"candidates":[{"identity":"direct","urls":["https://v16-webapp-prime.tiktok.com/small"],"width":720,"height":1280,"fps":30,"videoCodec":"h264","audioCodec":"aac","audioBitrateKbps":64,"videoBitrateKbps":2000,"container":"mp4"}]});
    let metadata = json!({"id":"123","title":"Post","formats":[{"format_id":"music","url":"https://p16.tiktokcdn.com/music.mp3","vcodec":"none","acodec":"mp3"},{"format_id":"best","url":"https://v16-webapp-prime.tiktok.com/large","width":1080,"height":1920,"fps":30,"vcodec":"h265","acodec":"aac","ext":"mp4","filesize":2100,"tbr":1300}]});
    let combined = merge_video_candidates(&item, &metadata, "123").unwrap();
    let rows = combined["candidates"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row["width"] == 1080 && row["videoCodec"] == "hevc"));
    assert!(!rows.iter().any(|row| row["videoCodec"] == "none"));
}

#[test]
fn merge_rejects_different_post_or_unsafe_highest_candidate() {
    let item = json!({"candidates":[]});
    let bad = json!({"id":"other","formats":[]});
    assert!(merge_video_candidates(&item, &bad, "123").is_err());
    let bad = json!({"id":"123","formats":[{"format_id":"best","url":"https://127.0.0.1/private","width":1080,"height":1920,"vcodec":"h265","acodec":"aac","ext":"mp4"}]});
    assert!(merge_video_candidates(&item, &bad, "123").is_err());
}

#[test]
fn supplementary_format_without_dimensions_is_retained_for_bounded_probing() {
    let item = json!({"candidates":[{"identity":"direct","urls":["https://v16-webapp-prime.tiktok.com/original"],"width":1080,"height":1920,"fps":30,"videoCodec":"hevc","audioCodec":"aac"}]});
    let metadata = json!({"id":"123","formats":[{"format_id":"download","format_note":"watermarked","url":"https://v16-webapp-prime.tiktok.com/download","width":null,"height":null,"vcodec":"h264","acodec":"aac","ext":"mp4"}]});
    let merged = merge_video_candidates(&item, &metadata, "123").unwrap();
    assert_eq!(merged["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(merged["candidates"][0]["width"], 1080);
    assert!(merged["candidates"][1]["width"].is_null());
    assert_eq!(merged["candidates"][1]["identity"], "download");
}

#[test]
fn extractor_can_supply_single_video_inventory_without_a_working_web_detail_page() {
    let source = validate_media_url("https://www.tiktok.com/@public/video/123").unwrap();
    let metadata = json!({"id":"123","title":"Public video","duration":12,"formats":[{"format_id":"best","url":"https://v16-webapp-prime.tiktok.com/large","width":1080,"height":1920,"fps":30,"vcodec":"h265","acodec":"aac","ext":"mp4","filesize":2100,"tbr":1300}]});
    let result = inventory_from_ytdlp(&metadata, &source).unwrap();
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["items"][0]["kind"], "video");
    assert_eq!(result["postId"], "123");
}

#[test]
fn explicit_silent_track_metadata_enriches_the_same_direct_candidate() {
    let item = json!({"candidates":[{"identity":"original","urls":["https://v16-webapp-prime.tiktok.com/large?sig=old"],"width":1080,"height":1920,"videoCodec":"hevc","audioCodec":null}]});
    let metadata = json!({"id":"123","formats":[{"format_id":"best","url":"https://v16-webapp-prime.tiktok.com/large?sig=new","width":1080,"height":1920,"fps":30,"vcodec":"hevc","acodec":"none","ext":"mp4"}]});
    let merged = merge_video_candidates(&item, &metadata, "123").unwrap();
    assert_eq!(merged["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(merged["candidates"][0]["audioAbsent"], true);
}

#[test]
fn extractor_context_stays_with_added_candidate_and_known_hdr_enriches_direct_stream() {
    let item = json!({"candidates":[{"identity":"direct","urls":["https://v16-webapp-prime.tiktok.com/direct"],"width":1080,"height":1920,"videoCodec":"hevc","audioCodec":"aac"}]});
    let metadata = json!({"id":"123","http_headers":{"User-Agent":"extractor-UA","Referer":"https://www.tiktok.com/@test/video/123"},"formats":[{"format_id":"same","url":"https://v16-webapp-prime.tiktok.com/direct","width":1080,"height":1920,"vcodec":"hevc","acodec":"aac","dynamic_range":"HDR10","fps":30},{"format_id":"extra","url":"https://v19-webapp-prime.tiktok.com/extra","width":720,"height":1280,"vcodec":"h264","acodec":"aac","cookies":"anon=value; Domain=.tiktok.com; Path=/","http_headers":{"User-Agent":"format-UA"}}]});
    let merged = merge_video_candidates(&item, &metadata, "123").unwrap();
    assert_eq!(merged["candidates"][0]["dynamicRange"], "HDR10");
    assert_eq!(
        merged["candidates"][1]["requestContext"]["headers"]["User-Agent"],
        "format-UA"
    );
    assert_eq!(
        merged["candidates"][1]["requestContext"]["serializedCookies"],
        "anon=value; Domain=.tiktok.com; Path=/"
    );
    assert!(merged["candidates"][0].get("requestContext").is_none());
}
