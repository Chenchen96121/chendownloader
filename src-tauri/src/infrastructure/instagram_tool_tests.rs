use super::*;
use crate::{
    application::{
        media_url::validate_media_url, process::ProcessFailureKind, tool_resolver::ResolvedTool,
    },
    domain::{instagram::InstagramMediaKind, managed_tools::ManagedToolError},
};
use serde_json::json;
use std::sync::{atomic::AtomicUsize, Mutex};

fn source() -> ValidatedMediaUrl {
    validate_media_url("https://www.instagram.com/p/Public_01/").unwrap()
}

// Synthetic fixture: URL-only image variants and a mixed carousel, with no
// captured CDN signatures, cookies, or real account/content identifiers.
fn response() -> Vec<u8> {
    let children = (0..20)
        .map(|index| {
            let video = index == 5;
            let mut item = json!({
                "__typename": if video { "XIGPolarisVideoMedia" } else { "XIGPolarisImageMedia" },
                "pk": format!("item_{index}"), "media_type": if video {2} else {1},
                "image_versions2": { "candidates": [
                    {"url": format!("https://cdninstagram.com/full_{index}.jpg")},
                    {"url": format!("https://cdninstagram.com/small_{index}.jpg")}
                ]},
                "original_width": 2400, "original_height": 3200,
                "display_uri": format!("https://cdninstagram.com/preview_{index}.jpg")
            });
            if video {
                item["video_versions"] =
                    json!([{"type":101,"url":"https://cdninstagram.com/video.mp4"}]);
            }
            item
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({"data":{"xig_polaris_media":{
        "__typename":"XIGPolarisCarouselMedia", "code":"Public_01", "pk":"parent_1", "id":"opaque_1",
        "gating_ruling":null,
        "if_not_gated_logged_out":{
            "__typename":"XIGPolarisCarouselMedia", "code":"Public_01", "pk":"parent_1", "id":"opaque_1",
            "media_type":8, "carousel_media":children,
            "caption":{"text":"Synthetic carousel"}, "user":{"username":"fixture"}
        }
    }}})).unwrap()
}

fn put_witness(directory: &Path) {
    fs::write(directory.join("media.dump"), response()).unwrap();
    fs::write(directory.join("homepage.dump"), b"<html>anonymous</html>").unwrap();
    fs::write(directory.join("ruling.dump"), br#"{"status":"ok"}"#).unwrap();
}

#[test]
fn complete_witness_preserves_all_children_and_candidate_order() {
    let dir = tempfile::tempdir().unwrap();
    put_witness(dir.path());
    let inventory = parse_witness(&source(), dir.path()).unwrap();
    assert_eq!(inventory.items().len(), 20);
    assert_eq!(
        inventory.items()[5].public_item().kind,
        InstagramMediaKind::Video
    );
    for (index, item) in inventory.items().iter().enumerate() {
        assert_eq!(item.public_item().ordinal, (index + 1) as u16);
        assert_eq!(item.media_id(), format!("item_{index}"));
        assert_eq!(item.display_url().path(), format!("/full_{index}.jpg"));
        assert!(item.prefers_direct_asset());
        assert_eq!(item.public_item().width, None);
    }
}

#[test]
fn wrapper_requires_public_ungated_matching_identity_and_known_type() {
    let root: Value = serde_json::from_slice(&response()).unwrap();
    for (pointer, replacement, expected) in [
        (
            "/data/xig_polaris_media/code",
            json!("Different_01"),
            AnalysisErrorCode::ContentChanged,
        ),
        (
            "/data/xig_polaris_media/if_not_gated_logged_out/pk",
            json!("other"),
            AnalysisErrorCode::ContentChanged,
        ),
        (
            "/data/xig_polaris_media/if_not_gated_logged_out/id",
            json!("other"),
            AnalysisErrorCode::ContentChanged,
        ),
        (
            "/data/xig_polaris_media/gating_ruling",
            json!({"restricted":true}),
            AnalysisErrorCode::AccessDenied,
        ),
        (
            "/data/xig_polaris_media/if_not_gated_logged_out",
            Value::Null,
            AnalysisErrorCode::AuthenticationRequired,
        ),
        (
            "/data/xig_polaris_media/__typename",
            json!("UnknownMedia"),
            AnalysisErrorCode::InvalidMetadata,
        ),
        (
            "/data/xig_polaris_media/if_not_gated_logged_out/carousel_media/1/pk",
            json!("item_0"),
            AnalysisErrorCode::InvalidMetadata,
        ),
    ] {
        let mut value = root.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert_eq!(
            parse_web_response(&source(), &serde_json::to_vec(&value).unwrap()).err(),
            Some(expected),
            "{pointer}"
        );
    }
    let mut ambiguous = root;
    ambiguous["items"] = json!([]);
    assert_eq!(
        parse_web_response(&source(), &serde_json::to_vec(&ambiguous).unwrap()).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
}

#[test]
fn witness_rejects_missing_duplicate_foreign_and_oversized_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("homepage.dump"), b"<html></html>").unwrap();
    assert_eq!(
        parse_witness(&source(), dir.path()).err(),
        Some(AnalysisErrorCode::ExtractorOutdated)
    );
    put_witness(dir.path());
    fs::write(dir.path().join("duplicate.dump"), response()).unwrap();
    assert_eq!(
        parse_witness(&source(), dir.path()).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
    fs::remove_file(dir.path().join("duplicate.dump")).unwrap();
    fs::write(dir.path().join("unexpected.json"), b"{}").unwrap();
    assert_eq!(
        inventory_files(dir.path()).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
    fs::remove_file(dir.path().join("unexpected.json")).unwrap();
    fs::File::create(dir.path().join("oversized.dump"))
        .unwrap()
        .set_len(FILE_LIMIT + 1)
        .unwrap();
    assert_eq!(
        inventory_files(dir.path()).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
}

struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        assert_eq!(name, ToolName::YtDlp);
        Ok(ResolvedTool {
            name,
            version: "2026.08.19".into(),
            path: "fixture.exe".into(),
        })
    }
}

struct WitnessProcess {
    calls: AtomicUsize,
    directory: Mutex<Option<PathBuf>>,
    exit_code: i32,
    stderr: &'static [u8],
}
impl ProcessSupervisor for WitnessProcess {
    fn run(
        &self,
        request: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            request.program.explicit_https_proxy_host(),
            Some("www.instagram.com")
        );
        let args = request
            .args
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(args.contains(&"--write-pages".into()));
        assert!(args.contains(&"--simulate".into()));
        assert!(args.contains(&"--no-cookies-from-browser".into()));
        let profile = args
            .iter()
            .position(|arg| arg.as_ref() == "--impersonate")
            .unwrap();
        assert_eq!(
            args[profile + 1].as_ref(),
            super::super::ytdlp::INSTAGRAM_BROWSER_PROFILE
        );
        assert_eq!(args.last().unwrap().as_ref(), source().url());
        put_witness(&request.working_dir);
        *self.directory.lock().unwrap() = Some(request.working_dir);
        Ok(ProcessExit { code: self.exit_code, stdout: br#"{"extractor_key":"Instagram","webpage_url":"https://www.instagram.com/p/Public_01/","playlist_count":20,"entries":[null,null,null,null,null,{},null,null,null,null,null,null,null,null,null,null,null,null,null,null]}"#.to_vec(), stderr:self.stderr.to_vec() })
    }
}

fn process(code: i32, stderr: &'static [u8]) -> WitnessProcess {
    WitnessProcess {
        calls: AtomicUsize::new(0),
        directory: Mutex::new(None),
        exit_code: code,
        stderr,
    }
}

#[test]
fn tool_uses_closed_anonymous_request_and_cleans_witness_directory() {
    let process = process(0, b"");
    let extractor = YtDlpInstagramInventoryExtractor::new(Resolver, &process);
    assert_eq!(
        extractor
            .extract_post(&source(), &CancellationToken::new())
            .unwrap()
            .items()
            .len(),
        20
    );
    assert!(!process.directory.lock().unwrap().as_ref().unwrap().exists());
    for url in [
        "https://www.instagram.com/reel/Reel_01/",
        "https://www.youtube.com/watch?v=abcdefghijk",
    ] {
        assert_eq!(
            extractor
                .extract_post(&validate_media_url(url).unwrap(), &CancellationToken::new())
                .err(),
            Some(AnalysisErrorCode::UnsupportedMedia)
        );
    }
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        extractor.extract_post(&source(), &token).err(),
        Some(AnalysisErrorCode::ProcessFailed)
    );
    assert_eq!(process.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn complete_dump_does_not_override_tool_denials_or_partial_failure() {
    for (code, stderr, expected) in [
        (
            0,
            &b"ERROR: HTTP Error 429: Too Many Requests"[..],
            AnalysisErrorCode::RateLimited,
        ),
        (
            1,
            &b"ERROR: Access denied"[..],
            AnalysisErrorCode::AccessDenied,
        ),
        (
            1,
            &b"ERROR: content has been removed"[..],
            AnalysisErrorCode::ContentUnavailable,
        ),
        (
            1,
            &b"ERROR: There is no video in this post"[..],
            AnalysisErrorCode::InvalidMetadata,
        ),
        (
            1,
            &b"ERROR: network failed"[..],
            AnalysisErrorCode::ProcessFailed,
        ),
    ] {
        let p = process(code, stderr);
        assert_eq!(
            YtDlpInstagramInventoryExtractor::new(Resolver, p)
                .extract_post(&source(), &CancellationToken::new())
                .err(),
            Some(expected)
        );
    }
}

struct FailureExtractor(AnalysisErrorCode);
impl InstagramInventoryExtractor for FailureExtractor {
    fn extract_post(
        &self,
        _: &ValidatedMediaUrl,
        _: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        Err(self.0)
    }
}

#[test]
fn fallback_only_retries_transport_schema_or_anonymous_session_failures() {
    for error in [
        AnalysisErrorCode::NetworkFailed,
        AnalysisErrorCode::AnalysisTimeout,
        AnalysisErrorCode::ExtractorOutdated,
        AnalysisErrorCode::AuthenticationRequired,
    ] {
        let extractor = ToolFallbackInstagramInventoryExtractor::new(
            FailureExtractor(error),
            FailureExtractor(AnalysisErrorCode::Unknown),
        );
        assert_eq!(
            extractor
                .extract_post(&source(), &CancellationToken::new())
                .err(),
            Some(AnalysisErrorCode::Unknown)
        );
    }
    for error in [
        AnalysisErrorCode::AccessDenied,
        AnalysisErrorCode::RateLimited,
        AnalysisErrorCode::ContentUnavailable,
        AnalysisErrorCode::ContentChanged,
        AnalysisErrorCode::InvalidMetadata,
    ] {
        let extractor = ToolFallbackInstagramInventoryExtractor::new(
            FailureExtractor(error),
            FailureExtractor(AnalysisErrorCode::Unknown),
        );
        assert_eq!(
            extractor
                .extract_post(&source(), &CancellationToken::new())
                .err(),
            Some(error)
        );
    }
}

#[test]
fn budget_violation_cancels_child_without_canceling_caller() {
    let dir = tempfile::tempdir().unwrap();
    let token = CancellationToken::new();
    let result = run_with_budget(dir.path(), &token, |_| {
        fs::File::create(dir.path().join("too-large.dump"))
            .unwrap()
            .set_len(FILE_LIMIT + 1)
            .unwrap();
        Ok(ProcessExit {
            code: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    });
    assert_eq!(result.err(), Some(AnalysisErrorCode::InvalidMetadata));
    assert!(!token.is_canceled());
}

#[test]
fn image_only_no_video_exit_requires_complete_validated_inventory() {
    let mut value: Value = serde_json::from_slice(&response()).unwrap();
    let child = value
        .pointer_mut("/data/xig_polaris_media/if_not_gated_logged_out/carousel_media/5")
        .unwrap();
    child["media_type"] = json!(1);
    child["__typename"] = json!("XIGPolarisImageMedia");
    child.as_object_mut().unwrap().remove("video_versions");
    let inventory = parse_web_response(&source(), &serde_json::to_vec(&value).unwrap()).unwrap();
    let exit = ProcessExit {
        code: 1,
        stdout: Vec::new(),
        stderr: b"ERROR: [Instagram] fixture: No video formats found!".to_vec(),
    };
    assert_eq!(validate_exit(&source(), &exit, &inventory), Ok(()));
    let mixed = parse_web_response(&source(), &response()).unwrap();
    assert_eq!(
        validate_exit(&source(), &exit, &mixed),
        Err(AnalysisErrorCode::InvalidMetadata)
    );
}

#[test]
fn mixed_post_image_errors_accept_only_complete_matching_video_entries() {
    let inventory = parse_web_response(&source(), &response()).unwrap();
    let mut entries = vec![Value::Null; 20];
    entries[5] = json!({"formats":[{"format_id":"original"}]});
    let mut exit = ProcessExit {
        code: 1,
        stdout: serde_json::to_vec(&json!({"extractor_key":"Instagram","webpage_url":source().url(),"playlist_count":20,"entries":entries})).unwrap(),
        stderr: b"ERROR: [Instagram] image: No video formats found!".to_vec(),
    };
    assert_eq!(validate_exit(&source(), &exit, &inventory), Ok(()));
    entries[5] = Value::Null;
    exit.stdout = serde_json::to_vec(&json!({"extractor_key":"Instagram","webpage_url":source().url(),"playlist_count":20,"entries":entries})).unwrap();
    assert_eq!(
        validate_exit(&source(), &exit, &inventory),
        Err(AnalysisErrorCode::InvalidMetadata)
    );
}

#[test]
fn metadata_cardinality_and_source_must_agree_with_the_dump() {
    let inventory = parse_web_response(&source(), &response()).unwrap();
    for (value, error) in [
        (
            json!({"extractor_key":"Instagram","webpage_url":source().url(),"playlist_count":19,"entries":vec![Value::Null;20]}),
            AnalysisErrorCode::InvalidMetadata,
        ),
        (
            json!({"extractor_key":"Instagram","webpage_url":source().url(),"playlist_count":20,"entries":vec![Value::Null;19]}),
            AnalysisErrorCode::InvalidMetadata,
        ),
        (
            json!({"extractor_key":"Instagram","webpage_url":"https://www.instagram.com/p/Different_01/","entries":vec![Value::Null;20]}),
            AnalysisErrorCode::ContentChanged,
        ),
        (
            json!({"extractor_key":"Instagram","webpage_url":source().url()}),
            AnalysisErrorCode::InvalidMetadata,
        ),
    ] {
        let exit = ProcessExit {
            code: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: Vec::new(),
        };
        assert_eq!(validate_exit(&source(), &exit, &inventory), Err(error));
    }
}

#[test]
fn file_count_and_aggregate_budgets_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..=COUNT_LIMIT {
        fs::write(dir.path().join(format!("{index}.dump")), b"").unwrap();
    }
    assert_eq!(
        inventory_files(dir.path()).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
    let dir = tempfile::tempdir().unwrap();
    for index in 0..5 {
        fs::File::create(dir.path().join(format!("{index}.dump")))
            .unwrap()
            .set_len(FILE_LIMIT)
            .unwrap();
    }
    assert_eq!(
        inventory_files(dir.path()).err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
}

#[test]
fn caller_cancellation_reaches_child_and_stops_witness_processing() {
    let dir = tempfile::tempdir().unwrap();
    let token = CancellationToken::new();
    let result = run_with_budget(dir.path(), &token, |child| {
        token.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !child.is_canceled() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(child.is_canceled());
        Ok(ProcessExit {
            code: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    });
    assert_eq!(result.err(), Some(AnalysisErrorCode::ProcessFailed));
}
