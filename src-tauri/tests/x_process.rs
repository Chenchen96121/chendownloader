use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        network_profile::NetworkProfile,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
        x_source::{XInventoryExtractor, XRequestContext},
    },
    domain::managed_tools::{ManagedToolError, ToolName},
    infrastructure::x::GalleryDlXAdapter,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        assert_eq!(name, ToolName::GalleryDl);
        Ok(ResolvedTool {
            name,
            version: "fixture".into(),
            path: PathBuf::from("C:/managed/gallery-dl.exe"),
        })
    }
}
struct Process(Mutex<Vec<ProcessRequest>>);
impl ProcessSupervisor for Process {
    fn run(
        &self,
        r: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        self.0.lock().unwrap().push(r);
        Ok(ProcessExit {
            code: 0,
            stdout: include_bytes!("../../fixtures/x/mixed-post.json").to_vec(),
            stderr: vec![],
        })
    }
}
#[test]
fn x_process_uses_raw_nonrecursive_inventory_with_bounded_fixed_arguments() {
    let process = Process(Mutex::new(vec![]));
    let adapter = GalleryDlXAdapter::new(Resolver, &process);
    let context = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let source = validate_media_url("https://x.com/i/web/status/12345678901234567890").unwrap();
    let inventory = adapter.extract_post(&source, &context).unwrap();
    assert_eq!(inventory.items.len(), 4);
    let calls = process.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let args = calls[0]
        .args
        .iter()
        .map(|s| s.to_str().unwrap())
        .collect::<Vec<_>>();
    for required in [
        "--config-ignore",
        "--dump-json",
        "extractor.twitter.transform=false",
        "extractor.twitter.text-tweets=true",
        "extractor.twitter.size=orig",
        "extractor.twitter.articles=false",
        "extractor.twitter.previews=false",
        "extractor.twitter.conversations=false",
    ] {
        assert!(args.contains(&required), "{required}");
    }
    assert_eq!(
        args.last(),
        Some(&"https://x.com/i/web/status/12345678901234567890")
    );
    assert!(!args.iter().any(|s| s.contains("cookie")));
    assert!(matches!(
        calls[0].output_mode,
        OutputMode::Capture {
            stdout_limit: 16_777_216,
            stderr_limit: 65_536
        }
    ));
    assert!(calls[0].timeout.unwrap() <= Duration::from_secs(120));
}
