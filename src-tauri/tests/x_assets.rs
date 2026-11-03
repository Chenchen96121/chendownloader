use local_media_downloader_lib::{
    application::{
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadErrorCode,
        managed_tools::{ManagedToolError, ToolName},
    },
    infrastructure::image_verification::verify_image_file,
};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};
struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixture".into(),
            path: PathBuf::from("C:/managed/ffprobe.exe"),
        })
    }
}
struct Probe;
impl ProcessSupervisor for Probe {
    fn run(
        &self,
        request: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let args = request
            .args
            .iter()
            .map(|a| a.to_str().unwrap())
            .collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|a| a == ["-protocol_whitelist", "file"]));
        Ok(ProcessExit{code:0,stdout:br#"{"streams":[{"codec_type":"video","codec_name":"mjpeg","width":1600,"height":900}],"format":{"format_name":"jpeg_pipe"}}"#.to_vec(),stderr:vec![]})
    }
}
fn bytes() -> Vec<u8> {
    [vec![0xff, 0xd8, 0xff], vec![42; 64]].concat()
}
#[test]
fn x_generic_image_verification_keeps_original_bytes_and_checks_copies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.jpg");
    let original = bytes();
    fs::write(&path, &original).unwrap();
    let image = verify_image_file(
        &Resolver,
        &Probe,
        &path,
        Some((1600, 900)),
        512 * 1024 * 1024,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(fs::read(image.path()).unwrap(), original);
    assert_eq!(image.sha256(), format!("{:x}", Sha256::digest(&original)));
    assert_eq!(image.container(), "jpg");
    let copy = dir.path().join("copy.jpg");
    fs::write(&copy, &original).unwrap();
    image.verify_byte_identical_copy(&copy).unwrap();
    fs::write(&copy, vec![0; original.len()]).unwrap();
    assert!(image.verify_byte_identical_copy(&copy).is_err());
}
#[test]
fn x_generic_image_rejects_html_empty_over_budget_and_wrong_dimensions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stage.jpg");
    for content in [b"<html>login</html>".to_vec(), vec![]] {
        fs::write(&path, content).unwrap();
        assert!(verify_image_file(
            &Resolver,
            &Probe,
            &path,
            None,
            1024,
            &CancellationToken::new()
        )
        .is_err());
    }
    fs::write(&path, bytes()).unwrap();
    assert!(matches!(
        verify_image_file(&Resolver, &Probe, &path, None, 1, &CancellationToken::new()),
        Err(DownloadErrorCode::OutputInvalid)
    ));
    assert!(matches!(
        verify_image_file(
            &Resolver,
            &Probe,
            &path,
            Some((800, 450)),
            1024,
            &CancellationToken::new()
        ),
        Err(DownloadErrorCode::ImageDimensionsMismatch)
    ));
}
#[test]
fn x_generic_image_cancellation_and_directory_are_not_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.jpg");
    fs::write(&path, bytes()).unwrap();
    let canceled = CancellationToken::new();
    canceled.cancel();
    assert!(matches!(
        verify_image_file(&Resolver, &Probe, &path, None, 1024, &canceled),
        Err(DownloadErrorCode::ProcessFailed)
    ));
    assert!(verify_image_file(
        &Resolver,
        &Probe,
        dir.path(),
        None,
        1024,
        &CancellationToken::new()
    )
    .is_err());
}

use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url, network_profile::NetworkProfile, x_analysis::XPreviewLoader,
        x_download_job::XImageDownloader, x_source::XRequestContext,
    },
    domain::media::AnalysisErrorCode,
    infrastructure::x::{
        assets::{XAssetAdapter, XAssetTransport},
        inventory::parse_x_inventory,
    },
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
struct Transport {
    content: Vec<u8>,
    calls: Mutex<Vec<(String, u64)>>,
    network: Arc<NetworkProfile>,
}
impl XAssetTransport for Transport {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<(), AnalysisErrorCode> {
        assert!(Arc::ptr_eq(&context.network, &self.network));
        self.calls.lock().unwrap().push((url.to_string(), maximum));
        fs::write(destination, &self.content).unwrap();
        Ok(())
    }
}
fn context() -> XRequestContext {
    XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}
fn inventory() -> local_media_downloader_lib::application::x_source::XInventory {
    parse_x_inventory(
        include_bytes!("../../fixtures/x/mixed-post.json"),
        &validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap(),
    )
    .unwrap()
}
#[test]
fn x_asset_original_download_is_verified_and_preview_is_independent() {
    let ctx = context();
    let transport = Transport {
        content: bytes(),
        calls: Mutex::new(vec![]),
        network: ctx.network.clone(),
    };
    let adapter = XAssetAdapter::with_transport(Resolver, Probe, &transport);
    let dir = tempfile::tempdir().unwrap();
    let inv = inventory();
    let asset = adapter.download(&inv.items[0], dir.path(), &ctx).unwrap();
    assert_eq!(asset.container(), "jpg");
    assert_eq!(fs::read(asset.path()).unwrap(), bytes());
    let preview = adapter.load(&inv.items[1], &ctx).unwrap().unwrap();
    assert_eq!(preview.mime_type, "image/jpeg");
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].0.contains("name=orig"));
    assert_eq!(calls[0].1, 512 * 1024 * 1024);
    assert_eq!(calls[1].1, 8 * 1024 * 1024);
}
#[test]
fn x_asset_failed_verification_cleans_only_staging_and_never_tries_lower_quality() {
    let ctx = context();
    let transport = Transport {
        content: b"<html>login</html>".to_vec(),
        calls: Mutex::new(vec![]),
        network: ctx.network.clone(),
    };
    let adapter = XAssetAdapter::with_transport(Resolver, Probe, &transport);
    let dir = tempfile::tempdir().unwrap();
    let inv = inventory();
    assert!(matches!(
        adapter.download(&inv.items[0], dir.path(), &ctx),
        Err(DownloadErrorCode::ImageContentInvalid)
    ));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}
#[test]
fn x_asset_rejects_foreign_or_non_original_candidates_before_network() {
    let ctx = context();
    let transport = Transport {
        content: bytes(),
        calls: Mutex::new(vec![]),
        network: ctx.network.clone(),
    };
    let adapter = XAssetAdapter::with_transport(Resolver, Probe, &transport);
    let dir = tempfile::tempdir().unwrap();
    let mut inv = inventory();
    inv.items[0].image_candidates[0].url = url::Url::parse("https://evil.test/image.jpg").unwrap();
    assert!(adapter.download(&inv.items[0], dir.path(), &ctx).is_err());
    inv.items[0].image_candidates[0].url =
        url::Url::parse("https://pbs.twimg.com/media/image?format=jpg&name=small").unwrap();
    assert!(adapter.download(&inv.items[0], dir.path(), &ctx).is_err());
    assert!(transport.calls.lock().unwrap().is_empty());
}
impl XAssetTransport for &Transport {
    fn fetch(
        &self,
        url: &url::Url,
        destination: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<(), AnalysisErrorCode> {
        (**self).fetch(url, destination, maximum, context)
    }
}
