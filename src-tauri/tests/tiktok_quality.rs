use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        tiktok_source::{TikTokNativeAsset, TikTokRequestContext},
        tiktok_video::TikTokCandidateProbe,
    },
    domain::media::AnalysisErrorCode,
    infrastructure::{
        tiktok::inventory::parse_tiktok_inventory, ytdlp::tiktok::resolve_proven_tiktok_video,
    },
};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
struct Probe(bool);
impl TikTokCandidateProbe for Probe {
    fn probe(
        &self,
        _: &TikTokNativeAsset,
        candidate: &Value,
        _: &TikTokRequestContext,
    ) -> Result<Value, AnalysisErrorCode> {
        if !self.0 {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        Ok(
            json!({"fps":if candidate["identity"]=="unknown"{60}else{30},"dynamicRange":"SDR","audioChannels":2,"audioSampleRateHz":44100,"audioBitrateKbps":64,"videoBitrateKbps":2000}),
        )
    }
}
#[test]
fn unknown_fps_is_probed_before_selecting_known_thirty_fps_and_failure_cannot_downgrade() {
    let source = validate_media_url("https://www.tiktok.com/@test/video/123").unwrap();
    let context = TikTokRequestContext::new(
        Default::default(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let candidate = |id: &str, fps: Value| json!({"identity":id,"urls":[format!("https://v16-webapp-prime.tiktok.com/{id}")],"width":1080,"height":1920,"fps":fps,"videoCodec":"hevc","audioCodec":"aac","audioChannels":2,"audioSampleRateHz":44100,"audioBitrateKbps":64,"videoBitrateKbps":2000,"dynamicRange":"SDR"});
    let raw = json!({"postId":"123","title":"Quality","creator":null,"duration":12,"context":{"cookies":[]},"sourcePageUrl":source.url(),"items":[{"itemId":"asset","identity":"video","ordinal":1,"kind":"video","width":1080,"height":1920,"urls":["https://v16-webapp-prime.tiktok.com/known"],"previewUrls":[],"candidates":[candidate("known",30.into()),candidate("unknown",Value::Null)]}]});
    let inventory = parse_tiktok_inventory(&raw, &source).unwrap();
    let identity = inventory.items[0].identity.as_ref().unwrap();
    assert!(
        resolve_proven_tiktok_video(&inventory, identity, false, &context, &Probe(false)).is_err()
    );
    let resolved =
        resolve_proven_tiktok_video(&inventory, identity, false, &context, &Probe(true)).unwrap();
    let selected = resolved
        .media()
        .formats
        .iter()
        .find(|f| Some(&f.id) == resolved.plan().video_format_id.as_ref())
        .unwrap();
    assert_eq!(selected.fps, Some(60.0));
}

#[test]
fn unknown_resolution_is_measured_before_ranking_and_cannot_be_silently_skipped() {
    struct Dimensions;
    impl TikTokCandidateProbe for Dimensions {
        fn probe(
            &self,
            _: &TikTokNativeAsset,
            _: &Value,
            _: &TikTokRequestContext,
        ) -> Result<Value, AnalysisErrorCode> {
            Ok(
                json!({"width":2160,"height":3840,"fps":30,"dynamicRange":"SDR","videoBitrateKbps":4000}),
            )
        }
    }
    let source = validate_media_url("https://www.tiktok.com/@test/video/123").unwrap();
    let context = TikTokRequestContext::new(
        Default::default(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let candidate = |id: &str, width: Value, height: Value| json!({"identity":id,"urls":[format!("https://v16-webapp-prime.tiktok.com/{id}")],"width":width,"height":height,"fps":30,"videoCodec":"hevc","audioCodec":"aac","audioChannels":2,"audioSampleRateHz":44100,"audioBitrateKbps":64,"videoBitrateKbps":2000,"dynamicRange":"SDR"});
    let raw = json!({"postId":"123","title":"Quality","creator":null,"duration":12,"context":{"cookies":[]},"sourcePageUrl":source.url(),"items":[{"itemId":"asset","identity":"video","ordinal":1,"kind":"video","width":1080,"height":1920,"urls":["https://v16-webapp-prime.tiktok.com/known"],"previewUrls":[],"candidates":[candidate("known",1080.into(),1920.into()),candidate("unknown",Value::Null,Value::Null)]}]});
    let inventory = parse_tiktok_inventory(&raw, &source).unwrap();
    let identity = inventory.items[0].identity.as_ref().unwrap();
    assert!(
        resolve_proven_tiktok_video(&inventory, identity, false, &context, &Probe(false)).is_err()
    );
    let resolved =
        resolve_proven_tiktok_video(&inventory, identity, false, &context, &Dimensions).unwrap();
    let selected = resolved
        .media()
        .formats
        .iter()
        .find(|f| Some(&f.id) == resolved.plan().video_format_id.as_ref())
        .unwrap();
    assert_eq!((selected.width, selected.height), (Some(2160), Some(3840)));
}
