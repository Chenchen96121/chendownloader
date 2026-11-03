use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use local_media_downloader_lib::{
    application::{
        media_url::{validate_media_url, ValidatedMediaUrl},
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        instagram::InstagramMediaKind,
        managed_tools::{ManagedToolError, ToolName},
        media::AnalysisErrorCode,
    },
    infrastructure::{
        gallery_dl::{
            normalize_instagram_inventory, FallbackInstagramAssetAcquirer,
            FallbackInstagramInventoryExtractor, InstagramAssetAcquirer, InstagramInventory,
            InstagramInventoryExtractor, ResolvedInstagramItem, VerifiedInstagramImage,
        },
        instagram_web::{
            build_graphql_request, parse_web_response, InstagramWebAssetAcquirer,
            InstagramWebInventoryExtractor, InstagramWebTransport,
        },
    },
};

const SOURCE: &str = "https://www.instagram.com/p/Browser_02/";
const REST: &[u8] = include_bytes!("../../tests/fixtures/instagram-browser/rest-carousel.json");
const FALLBACK: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/mixed-carousel.jsonl");

struct FixtureExtractor {
    result: Result<&'static [u8], AnalysisErrorCode>,
    calls: Mutex<usize>,
}

struct FixtureWebTransport;

impl InstagramWebTransport for FixtureWebTransport {
    fn fetch_post(
        &self,
        _request: &local_media_downloader_lib::infrastructure::instagram_web::InstagramGraphQlRequest,
        _cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, AnalysisErrorCode> {
        Ok(REST.to_vec())
    }
}

struct FixtureAssetTransport;

impl InstagramWebTransport for FixtureAssetTransport {
    fn fetch_post(
        &self,
        _request: &local_media_downloader_lib::infrastructure::instagram_web::InstagramGraphQlRequest,
        _cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, AnalysisErrorCode> {
        Ok(REST.to_vec())
    }

    fn fetch_asset(
        &self,
        _url: &url::Url,
        destination: &Path,
        _maximum_bytes: u64,
        _cancellation: &CancellationToken,
    ) -> Result<(), AnalysisErrorCode> {
        fs::write(destination, [0xff, 0xd8, 0xff, 0, 1])
            .map_err(|_| AnalysisErrorCode::ProcessFailed)
    }
}

struct PreviewLimitTransport;

impl InstagramWebTransport for PreviewLimitTransport {
    fn fetch_post(
        &self,
        _request: &local_media_downloader_lib::infrastructure::instagram_web::InstagramGraphQlRequest,
        _cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, AnalysisErrorCode> {
        Ok(REST.to_vec())
    }

    fn fetch_asset(
        &self,
        _url: &url::Url,
        destination: &Path,
        maximum_bytes: u64,
        _cancellation: &CancellationToken,
    ) -> Result<(), AnalysisErrorCode> {
        if maximum_bytes != 8 * 1024 * 1024 {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        fs::write(destination, [0xff, 0xd8, 0xff, 0, 1])
            .map_err(|_| AnalysisErrorCode::ProcessFailed)
    }
}

struct InvalidAssetTransport;

impl InstagramWebTransport for InvalidAssetTransport {
    fn fetch_post(
        &self,
        _request: &local_media_downloader_lib::infrastructure::instagram_web::InstagramGraphQlRequest,
        _cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, AnalysisErrorCode> {
        Ok(REST.to_vec())
    }

    fn fetch_asset(
        &self,
        _url: &url::Url,
        _destination: &Path,
        _maximum_bytes: u64,
        _cancellation: &CancellationToken,
    ) -> Result<(), AnalysisErrorCode> {
        Err(AnalysisErrorCode::InvalidMetadata)
    }
}

struct FixtureResolver;

impl ManagedToolResolver for FixtureResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixture".to_owned(),
            path: PathBuf::from(r"C:\managed\ffprobe.exe"),
        })
    }
}

struct FixtureProcess;

impl ProcessSupervisor for FixtureProcess {
    fn run(
        &self,
        request: ProcessRequest,
        _cancellation: CancellationToken,
        _sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        assert_eq!(request.program.path(), Path::new(r"C:\managed\ffprobe.exe"));
        assert!(matches!(request.output_mode, OutputMode::Capture { .. }));
        assert_eq!(request.timeout, Some(Duration::from_secs(15)));
        let _args: Vec<OsString> = request.args;
        Ok(ProcessExit {
            code: 0,
            stdout: br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1440,"height":1800}],"format":{"format_name":"image2","size":"5"}}"#.to_vec(),
            stderr: Vec::new(),
        })
    }
}

struct VideoPreviewProcess;

impl ProcessSupervisor for VideoPreviewProcess {
    fn run(
        &self,
        _request: ProcessRequest,
        _cancellation: CancellationToken,
        _sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        Ok(ProcessExit {
            code: 0,
            stdout: br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1080,"height":1920}],"format":{"format_name":"image2","size":"5"}}"#.to_vec(),
            stderr: Vec::new(),
        })
    }
}

struct ErrorAssetAcquirer {
    preview: AnalysisErrorCode,
}

impl InstagramAssetAcquirer for ErrorAssetAcquirer {
    fn fetch_preview(
        &self,
        _item: &ResolvedInstagramItem,
        _protected_directory: &std::path::Path,
        _cancellation: &CancellationToken,
    ) -> Result<Option<VerifiedInstagramImage>, AnalysisErrorCode> {
        Err(self.preview)
    }

    fn download_image(
        &self,
        _item: &ResolvedInstagramItem,
        _protected_directory: &std::path::Path,
        _cancellation: &CancellationToken,
    ) -> Result<
        VerifiedInstagramImage,
        local_media_downloader_lib::domain::download::DownloadErrorCode,
    > {
        Err(local_media_downloader_lib::domain::download::DownloadErrorCode::Unknown)
    }
}

impl FixtureExtractor {
    fn failing(error: AnalysisErrorCode) -> Self {
        Self {
            result: Err(error),
            calls: Mutex::new(0),
        }
    }

    fn succeeding(bytes: &'static [u8]) -> Self {
        Self {
            result: Ok(bytes),
            calls: Mutex::new(0),
        }
    }
}

impl InstagramInventoryExtractor for FixtureExtractor {
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        _cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        *self.calls.lock().unwrap() += 1;
        let bytes = self.result?;
        normalize_instagram_inventory(source.media_id(), bytes)
    }
}

#[test]
fn web_response_preserves_carousel_order_and_selects_largest_variants() {
    let source = validate_media_url(SOURCE).unwrap();

    let inventory = parse_web_response(&source, REST).unwrap();

    assert_eq!(inventory.shortcode(), "Browser_02");
    assert_eq!(inventory.title(), "REST fixture");
    assert_eq!(inventory.creator(), Some("test_author"));
    assert_eq!(inventory.items().len(), 2);
    assert_eq!(inventory.items()[0].public_item().ordinal, 1);
    assert_eq!(
        inventory.items()[0].public_item().kind,
        InstagramMediaKind::Image
    );
    assert_eq!(
        (
            inventory.items()[0].public_item().width,
            inventory.items()[0].public_item().height,
        ),
        (Some(1440), Some(1800))
    );
    assert_eq!(inventory.items()[1].public_item().ordinal, 2);
    assert_eq!(
        inventory.items()[1].public_item().kind,
        InstagramMediaKind::Video
    );
    assert_eq!(
        (
            inventory.items()[1].public_item().width,
            inventory.items()[1].public_item().height,
        ),
        (Some(1920), Some(1080))
    );
}

#[test]
fn web_response_rejects_a_different_shortcode_and_missing_media() {
    let source = validate_media_url("https://www.instagram.com/p/Other_01/").unwrap();
    assert_eq!(
        parse_web_response(&source, REST).err(),
        Some(AnalysisErrorCode::ContentChanged)
    );

    let source = validate_media_url(SOURCE).unwrap();
    assert_eq!(
        parse_web_response(
            &source,
            br#"{"data":{"xdt_api__v1__media__shortcode__web_info":{"items":[]}},"status":"ok"}"#
        )
        .err(),
        Some(AnalysisErrorCode::ContentUnavailable)
    );
}

#[test]
fn graphql_request_is_closed_to_the_requested_post_and_current_document_contract() {
    let source = validate_media_url(SOURCE).unwrap();

    let request = build_graphql_request(&source, "csrf_fixture").unwrap();

    assert_eq!(request.url(), "https://www.instagram.com/graphql/query");
    assert_eq!(request.referer(), SOURCE);
    assert_eq!(request.csrf_token(), "csrf_fixture");
    let form = request
        .form()
        .iter()
        .cloned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        form.get("doc_id").map(String::as_str),
        Some("27128499623469141")
    );
    assert_eq!(
        form.get("server_timestamps").map(String::as_str),
        Some("true")
    );
    let variables: serde_json::Value =
        serde_json::from_str(form.get("variables").unwrap()).unwrap();
    assert_eq!(variables["shortcode"], "Browser_02");
    assert_eq!(
        variables["__relay_internal__pv__PolarisAIGMMediaWebLabelEnabledrelayprovider"],
        false
    );
}

#[test]
fn authentication_redirect_uses_web_fallback_but_other_failures_are_not_masked() {
    let source = validate_media_url("https://www.instagram.com/p/Mixed_01/").unwrap();
    let primary = FixtureExtractor::failing(AnalysisErrorCode::AuthenticationRequired);
    let fallback = FixtureExtractor::succeeding(FALLBACK);
    let adapter = FallbackInstagramInventoryExtractor::new(primary, fallback);

    let inventory = adapter
        .extract_post(&source, &CancellationToken::new())
        .unwrap();

    assert_eq!(inventory.items().len(), 3);

    let primary = FixtureExtractor::failing(AnalysisErrorCode::NetworkFailed);
    let fallback = FixtureExtractor::succeeding(FALLBACK);
    let adapter = FallbackInstagramInventoryExtractor::new(primary, fallback);
    assert_eq!(
        adapter
            .extract_post(&source, &CancellationToken::new())
            .err(),
        Some(AnalysisErrorCode::NetworkFailed)
    );
}

#[test]
fn metadata_timeout_tries_web_fallback_without_masking_access_errors() {
    let source = validate_media_url("https://www.instagram.com/p/Mixed_01/").unwrap();
    let adapter = FallbackInstagramInventoryExtractor::new(
        FixtureExtractor::failing(AnalysisErrorCode::AnalysisTimeout),
        FixtureExtractor::succeeding(FALLBACK),
    );

    let inventory = adapter
        .extract_post(&source, &CancellationToken::new())
        .unwrap();
    assert_eq!(inventory.items().len(), 3);

    let adapter = FallbackInstagramInventoryExtractor::new(
        FixtureExtractor::failing(AnalysisErrorCode::AccessDenied),
        FixtureExtractor::succeeding(FALLBACK),
    );
    assert_eq!(
        adapter
            .extract_post(&source, &CancellationToken::new())
            .err(),
        Some(AnalysisErrorCode::AccessDenied)
    );
}

#[test]
fn canceled_metadata_timeout_does_not_start_web_fallback() {
    let source = validate_media_url("https://www.instagram.com/p/Mixed_01/").unwrap();
    let adapter = FallbackInstagramInventoryExtractor::new(
        FixtureExtractor::failing(AnalysisErrorCode::AnalysisTimeout),
        FixtureExtractor::succeeding(FALLBACK),
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    assert_eq!(
        adapter.extract_post(&source, &cancellation).err(),
        Some(AnalysisErrorCode::AnalysisTimeout)
    );
}

#[test]
fn web_inventory_extractor_returns_normalized_media_and_stops_before_canceled_io() {
    let source = validate_media_url(SOURCE).unwrap();
    let extractor = InstagramWebInventoryExtractor::new(FixtureWebTransport);

    let inventory = extractor
        .extract_post(&source, &CancellationToken::new())
        .unwrap();

    assert_eq!(inventory.items().len(), 2);
    assert_eq!(
        inventory.items()[1].public_item().kind,
        InstagramMediaKind::Video
    );

    let canceled = CancellationToken::new();
    canceled.cancel();
    assert_eq!(
        extractor.extract_post(&source, &canceled).err(),
        Some(AnalysisErrorCode::ProcessFailed)
    );
}

#[test]
fn asset_fallback_routes_web_items_directly_and_does_not_mask_unrelated_primary_failures() {
    let source = validate_media_url(SOURCE).unwrap();
    let web = parse_web_response(&source, REST).unwrap();
    let gallery = normalize_instagram_inventory("Mixed_01", FALLBACK).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cancellation = CancellationToken::new();

    let acquirer = FallbackInstagramAssetAcquirer::new(
        ErrorAssetAcquirer {
            preview: AnalysisErrorCode::NetworkFailed,
        },
        ErrorAssetAcquirer {
            preview: AnalysisErrorCode::FormatUnavailable,
        },
    );
    assert_eq!(
        acquirer
            .fetch_preview(&web.items()[0], directory.path(), &cancellation)
            .err(),
        Some(AnalysisErrorCode::FormatUnavailable)
    );
    assert_eq!(
        acquirer
            .fetch_preview(&gallery.items()[0], directory.path(), &cancellation)
            .err(),
        Some(AnalysisErrorCode::NetworkFailed)
    );

    let acquirer = FallbackInstagramAssetAcquirer::new(
        ErrorAssetAcquirer {
            preview: AnalysisErrorCode::AuthenticationRequired,
        },
        ErrorAssetAcquirer {
            preview: AnalysisErrorCode::FormatUnavailable,
        },
    );
    assert_eq!(
        acquirer
            .fetch_preview(&gallery.items()[0], directory.path(), &cancellation)
            .err(),
        Some(AnalysisErrorCode::FormatUnavailable)
    );
}

#[test]
fn web_asset_acquirer_preserves_downloaded_bytes_and_reuses_native_image_verification() {
    let source = validate_media_url(SOURCE).unwrap();
    let inventory = parse_web_response(&source, REST).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let acquirer =
        InstagramWebAssetAcquirer::new(FixtureAssetTransport, FixtureResolver, FixtureProcess);

    let image = acquirer
        .fetch_preview(
            &inventory.items()[0],
            directory.path(),
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();

    assert_eq!(image.container(), "jpg");
    assert_eq!(image.mime_type(), Some("image/jpeg"));
    assert_eq!((image.width(), image.height()), (1440, 1800));
    assert_eq!(image.byte_length(), 5);
    assert_eq!(fs::read(image.path()).unwrap(), [0xff, 0xd8, 0xff, 0, 1]);
}

#[test]
fn web_preview_applies_the_preview_limit_before_transferring_the_asset() {
    let source = validate_media_url(SOURCE).unwrap();
    let inventory = parse_web_response(&source, REST).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let acquirer =
        InstagramWebAssetAcquirer::new(PreviewLimitTransport, FixtureResolver, FixtureProcess);

    let image = acquirer
        .fetch_preview(
            &inventory.items()[0],
            directory.path(),
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();

    assert_eq!(image.byte_length(), 5);
}

#[test]
fn invalid_direct_asset_is_reported_as_invalid_output_during_download() {
    let source = validate_media_url(SOURCE).unwrap();
    let inventory = parse_web_response(&source, REST).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let acquirer =
        InstagramWebAssetAcquirer::new(InvalidAssetTransport, FixtureResolver, FixtureProcess);

    assert_eq!(
        acquirer
            .download_image(
                &inventory.items()[0],
                directory.path(),
                &CancellationToken::new(),
            )
            .err(),
        Some(local_media_downloader_lib::domain::download::DownloadErrorCode::OutputInvalid)
    );
}

#[test]
fn verified_image_uses_filesystem_size_when_ffprobe_omits_its_optional_size_field() {
    struct SizeLessProbe;
    impl ProcessSupervisor for SizeLessProbe {
        fn run(
            &self,
            _: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            Ok(ProcessExit {
                code: 0,
                stdout: br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1440,"height":1800}],"format":{"format_name":"image2"}}"#.to_vec(),
                stderr: Vec::new(),
            })
        }
    }
    let inventory = parse_web_response(&validate_media_url(SOURCE).unwrap(), REST).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let acquirer =
        InstagramWebAssetAcquirer::new(FixtureAssetTransport, FixtureResolver, SizeLessProbe);
    let image = acquirer
        .download_image(
            &inventory.items()[0],
            directory.path(),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(image.byte_length(), 5);
    assert_eq!(image.container(), "jpg");
    assert_eq!(fs::read(image.path()).unwrap(), [0xff, 0xd8, 0xff, 0, 1]);
}

#[test]
fn image_verification_distinguishes_dimension_size_and_probe_failures() {
    struct ProbeFailureFixture {
        code: i32,
        stdout: &'static [u8],
    }
    impl ProcessSupervisor for ProbeFailureFixture {
        fn run(
            &self,
            request: ProcessRequest,
            _: CancellationToken,
            _: &dyn ProcessEventSink,
        ) -> Result<ProcessExit, ProcessFailureKind> {
            assert_eq!(request.program.path(), Path::new(r"C:\managed\ffprobe.exe"));
            Ok(ProcessExit {
                code: self.code,
                stdout: self.stdout.to_vec(),
                stderr: Vec::new(),
            })
        }
    }
    let inventory = parse_web_response(&validate_media_url(SOURCE).unwrap(), REST).unwrap();
    for (code, stdout, expected) in [
        (0, br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":720,"height":900}],"format":{"format_name":"image2","size":"5"}}"#.as_slice(), "image_dimensions_mismatch"),
        (0, br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1440,"height":1800}],"format":{"format_name":"image2","size":"4"}}"#.as_slice(), "image_size_mismatch"),
        (1, b"".as_slice(), "image_probe_failed"),
        (0, b"invalid JSON".as_slice(), "image_probe_failed"),
        (0, br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1440,"height":1800}],"format":{"format_name":"image2","size":null}}"#.as_slice(), "image_probe_failed"),
        (0, br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1440,"height":1800}],"format":{"format_name":"image2","size":5}}"#.as_slice(), "image_probe_failed"),
        (0, br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1440,"height":1800}],"format":{"format_name":"image2","size":""}}"#.as_slice(), "image_size_mismatch"),
        (0, br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1440,"height":1800}],"format":{"format_name":"image2","size":"broken"}}"#.as_slice(), "image_size_mismatch"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let acquirer = InstagramWebAssetAcquirer::new(
            FixtureAssetTransport, FixtureResolver, ProbeFailureFixture { code, stdout },
        );
        let error = acquirer.download_image(
            &inventory.items()[0], directory.path(), &CancellationToken::new(),
        ).err().map(|error| serde_json::to_value(error).unwrap());
        assert_eq!(error, Some(serde_json::json!(expected)));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn non_image_response_is_distinguished_from_a_valid_image_that_fails_probe() {
    struct HtmlTransport;
    impl InstagramWebTransport for HtmlTransport {
        fn fetch_post(
            &self,
            _: &local_media_downloader_lib::infrastructure::instagram_web::InstagramGraphQlRequest,
            _: &CancellationToken,
        ) -> Result<Vec<u8>, AnalysisErrorCode> {
            unreachable!()
        }
        fn fetch_asset(
            &self,
            _: &url::Url,
            destination: &Path,
            _: u64,
            _: &CancellationToken,
        ) -> Result<(), AnalysisErrorCode> {
            fs::write(destination, b"<!doctype html><html>login</html>").unwrap();
            Ok(())
        }
    }
    let inventory = parse_web_response(&validate_media_url(SOURCE).unwrap(), REST).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let acquirer = InstagramWebAssetAcquirer::new(HtmlTransport, FixtureResolver, FixtureProcess);
    let error = acquirer
        .download_image(
            &inventory.items()[0],
            directory.path(),
            &CancellationToken::new(),
        )
        .err()
        .unwrap();
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        serde_json::json!("image_content_invalid")
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn web_video_cover_preview_is_verified_as_an_image_without_requiring_video_dimensions() {
    let source = validate_media_url(SOURCE).unwrap();
    let inventory = parse_web_response(&source, REST).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let acquirer =
        InstagramWebAssetAcquirer::new(FixtureAssetTransport, FixtureResolver, VideoPreviewProcess);

    let image = acquirer
        .fetch_preview(
            &inventory.items()[1],
            directory.path(),
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();

    assert_eq!(image.mime_type(), Some("image/jpeg"));
    assert_eq!((image.width(), image.height()), (1080, 1920));
}
