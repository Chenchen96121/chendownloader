use local_media_downloader_lib::{
    application::{
        network_profile::NetworkProfile, process::CancellationToken, x_source::XRequestContext,
    },
    domain::media::AnalysisErrorCode,
    infrastructure::x::manifest::{build_hls_snapshot, HlsManifestTransport},
};
use std::{
    collections::HashMap,
    fs,
    sync::Arc,
    time::{Duration, Instant},
};
struct Transport(HashMap<String, String>);
impl HlsManifestTransport for Transport {
    fn read(&self, url: &url::Url, _: &XRequestContext) -> Result<Vec<u8>, AnalysisErrorCode> {
        self.0
            .get(url.as_str())
            .map(|s| s.as_bytes().to_vec())
            .ok_or(AnalysisErrorCode::NetworkFailed)
    }
}
fn context() -> XRequestContext {
    XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}
fn source() -> url::Url {
    url::Url::parse("https://video.twimg.com/fixture/master.m3u8").unwrap()
}
#[test]
fn x_hls_snapshot_rewrites_nested_playlists_but_not_verified_remote_segments() {
    let transport=Transport(HashMap::from([
        (source().to_string(),"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=8000000,RESOLUTION=1920x1080\nmedia/high.m3u8\n".into()),
        ("https://video.twimg.com/fixture/media/high.m3u8".into(),"#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\nseg-1.ts\n#EXTINF:6,\nseg-2.ts\n#EXT-X-ENDLIST\n".into())
    ]));
    let snapshot = build_hls_snapshot(&source(), &transport, &context()).unwrap();
    assert!(!snapshot.is_media_playlist());
    snapshot.revalidate().unwrap();
    let master = fs::read_to_string(snapshot.input()).unwrap();
    assert!(!master.contains("media/high.m3u8"));
    // FFmpeg's file protocol is not a browser URL parser on Windows.
    assert!(master.contains("file:"));
    assert!(!master.contains("file://"));
    let directory = snapshot.input().parent().unwrap();
    let combined = fs::read_dir(directory)
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(combined.contains("https://video.twimg.com/fixture/media/seg-1.ts"));
    fs::write(snapshot.input(), "#EXTM3U\nfile:///C:/private\n").unwrap();
    assert!(snapshot.revalidate().is_err());
}
#[test]
fn x_hls_refuses_foreign_segments_keys_live_content_cycles_and_deep_graphs() {
    for media in [
        "#EXTM3U\n#EXTINF:1,\nhttps://evil.test/segment.ts\n#EXT-X-ENDLIST",
        "#EXTM3U\n#EXTINF:1,\nfile:///C:/private\n#EXT-X-ENDLIST",
        "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://video.twimg.com/key\"\n#EXTINF:1,\na.ts\n#EXT-X-ENDLIST",
        "#EXTM3U\n#EXTINF:1,\na.ts\n",
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nmaster.m3u8\n",
    ]{let t=Transport(HashMap::from([(source().to_string(),media.into())]));assert!(build_hls_snapshot(&source(),&t,&context()).is_err(),"{media}");}
    let mut documents = HashMap::new();
    for n in 0..6 {
        documents.insert(
            format!("https://video.twimg.com/{n}.m3u8"),
            format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n{}.m3u8\n", n + 1),
        );
    }
    let t = Transport(documents);
    assert!(build_hls_snapshot(
        &url::Url::parse("https://video.twimg.com/0.m3u8").unwrap(),
        &t,
        &context()
    )
    .is_err());
}
#[test]
fn x_hls_honors_deadline_cancel_and_manifest_byte_limit() {
    let t = Transport(HashMap::from([(source().to_string(), "x".repeat(524289))]));
    assert!(build_hls_snapshot(&source(), &t, &context()).is_err());
    let ctx = context();
    ctx.cancellation.cancel();
    assert!(build_hls_snapshot(&source(), &t, &ctx).is_err());
}

#[test]
fn x_hls_requires_playable_references_not_only_master_tags() {
    for document in [
        "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"empty\"\n",
        "#EXTM3U\n#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=1\n",
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n#EXT-X-ENDLIST\n",
    ] {
        let transport = Transport(HashMap::from([(source().to_string(), document.into())]));
        assert!(
            build_hls_snapshot(&source(), &transport, &context()).is_err(),
            "{document}"
        );
    }
}
