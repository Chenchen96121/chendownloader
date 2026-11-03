#[path = "support/bilibili.rs"]
mod fixtures;
use local_media_downloader_lib::{
    application::{
        bilibili_analysis::BilibiliCoverLoader,
        bilibili_source::BilibiliRequestContext,
        network_profile::NetworkProfile,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
    },
    domain::{bilibili::validate_bilibili_url, media::AnalysisErrorCode},
    infrastructure::bilibili::{
        cover::{BilibiliCoverTransport, HttpBilibiliCoverLoader},
        page::parse_bilibili_page,
    },
};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3, 2, 0,
    239, 163, 47, 91, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
#[derive(Clone)]
struct Transport {
    body: Arc<Vec<u8>>,
    calls: Arc<AtomicUsize>,
}
impl BilibiliCoverTransport for Transport {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        context: &BilibiliRequestContext,
    ) -> Result<(), AnalysisErrorCode> {
        assert_eq!(url.as_str(), "https://i0.hdslb.com/bfs/archive/fixture.jpg");
        assert!(
            context.deadline().saturating_duration_since(Instant::now()) <= Duration::from_secs(15)
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(destination, self.body.as_ref()).unwrap();
        Ok(())
    }
}
struct Probe {
    calls: Arc<AtomicUsize>,
    width: u32,
}
impl ProcessSupervisor for Probe {
    fn run(
        &self,
        request: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(
            request.timeout.unwrap() <= Duration::from_secs(10),
            "cover probe must inherit the remaining total budget"
        );
        assert!(request
            .args
            .windows(2)
            .any(|args| args == ["-protocol_whitelist", "file"]));
        assert!(matches!(
            request.output_mode,
            OutputMode::Capture {
                stdout_limit: 65536,
                stderr_limit: 65536
            }
        ));
        Ok(ProcessExit{code:0,stdout:serde_json::to_vec(&json!({"streams":[{"codec_type":"video","codec_name":"png","width":self.width,"height":1}],"format":{"format_name":"png_pipe","size":PNG.len().to_string()}})).unwrap(),stderr:vec![]})
    }
}
fn context() -> BilibiliRequestContext {
    BilibiliRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(10),
        Arc::new(NetworkProfile::current()),
    )
}
fn evidence() -> local_media_downloader_lib::application::bilibili_source::BilibiliPageEvidence {
    parse_bilibili_page(
        &validate_bilibili_url(fixtures::URL).unwrap(),
        include_bytes!("../../tests/fixtures/bilibili/ordinary-page.html"),
    )
    .unwrap()
}
#[test]
fn bilibili_cover_validates_actual_image_and_caps_shared_probe_deadline() {
    let calls = Arc::new(AtomicUsize::new(0));
    let probe_calls = Arc::new(AtomicUsize::new(0));
    let loader = HttpBilibiliCoverLoader::with_transport(
        fixtures::Resolver,
        Probe {
            calls: probe_calls.clone(),
            width: 1,
        },
        Transport {
            body: Arc::new(PNG.into()),
            calls: calls.clone(),
        },
    );
    let preview = loader.load(&evidence(), &context()).unwrap();
    assert_eq!(preview.mime_type, "image/png");
    assert_eq!(preview.data_base64, fixtures::PNG);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(probe_calls.load(Ordering::SeqCst), 1);
}
#[test]
fn bilibili_cover_rejects_html_oversize_and_invalid_dimensions() {
    for body in [
        b"<html>not an image</html>".to_vec(),
        vec![0; 8 * 1024 * 1024 + 1],
        PNG.to_vec(),
    ] {
        let loader = HttpBilibiliCoverLoader::with_transport(
            fixtures::Resolver,
            Probe {
                calls: Arc::new(AtomicUsize::new(0)),
                width: 0,
            },
            Transport {
                body: Arc::new(body),
                calls: Arc::new(AtomicUsize::new(0)),
            },
        );
        assert!(loader.load(&evidence(), &context()).is_err());
    }
}
#[test]
fn bilibili_cover_cancellation_and_unknown_host_prevent_fetch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let loader = HttpBilibiliCoverLoader::with_transport(
        fixtures::Resolver,
        Probe {
            calls: Arc::new(AtomicUsize::new(0)),
            width: 1,
        },
        Transport {
            body: Arc::new(PNG.into()),
            calls: calls.clone(),
        },
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(loader
        .load(
            &evidence(),
            &BilibiliRequestContext::new(
                cancel,
                Instant::now() + Duration::from_secs(10),
                Arc::new(NetworkProfile::current())
            )
        )
        .is_err());
    let mut state = fixtures::page();
    state["videoData"]["pic"] = "https://evil.test/cover.png".into();
    let page = format!("<script>window.__INITIAL_STATE__={state};</script>");
    let evidence = parse_bilibili_page(
        &validate_bilibili_url(fixtures::URL).unwrap(),
        page.as_bytes(),
    )
    .unwrap();
    assert!(loader.load(&evidence, &context()).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
