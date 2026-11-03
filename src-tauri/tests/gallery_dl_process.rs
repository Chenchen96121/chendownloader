use std::{collections::VecDeque, ffi::OsString, fs, path::PathBuf, sync::Mutex, time::Duration};

use local_media_downloader_lib::{
    application::{
        media_url::validate_media_url,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessExit, ProcessFailureKind,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadErrorCode,
        managed_tools::{ManagedToolError, ToolName},
        media::AnalysisErrorCode,
    },
    infrastructure::gallery_dl::{
        normalize_instagram_inventory, GalleryDlInstagramAdapter, InstagramAssetAcquirer,
        InstagramInventoryExtractor,
    },
};

const INPUT: &str = "https://www.instagram.com/p/Image_01/";
const OUTPUT: &[u8] = include_bytes!("../../tests/fixtures/gallery-dl/single-image.jsonl");

struct FakeResolver;
impl ManagedToolResolver for FakeResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: "fixture".into(),
            path: PathBuf::from(r"C:\managed").join(match name {
                ToolName::GalleryDl => "gallery-dl.exe",
                ToolName::Ffprobe => "ffprobe.exe",
                ToolName::YtDlp => "yt-dlp.exe",
                ToolName::Deno => "deno.exe",
                ToolName::Ffmpeg => "ffmpeg.exe",
            }),
        })
    }
}

#[derive(Debug)]
struct Invocation {
    program: PathBuf,
    args: Vec<OsString>,
    timeout: Option<Duration>,
    stdout_limit: usize,
    stderr_limit: usize,
    canceled: bool,
}
struct FakeProcess {
    responses: Mutex<VecDeque<Result<ProcessExit, ProcessFailureKind>>>,
    calls: Mutex<Vec<Invocation>>,
}
impl FakeProcess {
    fn returning(response: Result<ProcessExit, ProcessFailureKind>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([response])),
            calls: Mutex::new(Vec::new()),
        }
    }
}
impl ProcessSupervisor for FakeProcess {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let (stdout_limit, stderr_limit) = match request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => (stdout_limit, stderr_limit),
            _ => panic!("capture required"),
        };
        self.calls.lock().unwrap().push(Invocation {
            program: request.program.path().to_owned(),
            args: request.args,
            timeout: request.timeout,
            stdout_limit,
            stderr_limit,
            canceled: cancellation.is_canceled(),
        });
        self.responses.lock().unwrap().pop_front().unwrap()
    }
}
fn exit(code: i32, stdout: &[u8], stderr: &[u8]) -> Result<ProcessExit, ProcessFailureKind> {
    Ok(ProcessExit {
        code,
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    })
}

#[test]
fn inventory_uses_verified_gallery_dl_and_only_fixed_noninteractive_arguments() {
    let process = FakeProcess::returning(exit(0, OUTPUT, b""));
    let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
    let source = validate_media_url(INPUT).unwrap();
    adapter
        .extract_post(&source, &CancellationToken::new())
        .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.program, PathBuf::from(r"C:\managed\gallery-dl.exe"));
    assert_eq!(
        call.args,
        [
            "--ignore-config",
            "--no-skip",
            "--no-download",
            "--quiet",
            "--no-input",
            "--postprocessor",
            "metadata",
            "--postprocessor-option",
            "mode=jsonl",
            "--postprocessor-option",
            "event=prepare",
            "--postprocessor-option",
            "filename=-",
            "--",
            INPUT
        ]
        .map(OsString::from)
    );
    assert_eq!(call.timeout, Some(Duration::from_secs(45)));
    assert_eq!((call.stdout_limit, call.stderr_limit), (2_097_152, 262_144));
    assert!(!call.canceled);
    let joined = call
        .args
        .iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    assert!(!joined.contains("cookie"));
    assert!(!joined.contains("proxy"));
    assert!(!joined.contains("--config"));
}

#[test]
fn maps_timeout_cancel_prompt_rate_limit_login_extractor_and_network_failures() {
    let source = validate_media_url(INPUT).unwrap();
    for (response, expected) in [
        (
            Err(ProcessFailureKind::TimedOut),
            AnalysisErrorCode::AnalysisTimeout,
        ),
        (
            Err(ProcessFailureKind::Canceled),
            AnalysisErrorCode::ProcessFailed,
        ),
        (
            exit(1, b"", b"Username: "),
            AnalysisErrorCode::AuthenticationRequired,
        ),
        (
            exit(1, b"", b"HTTP Error 429 Too Many Requests"),
            AnalysisErrorCode::RateLimited,
        ),
        (
            exit(1, b"", b"login required; use cookies"),
            AnalysisErrorCode::AuthenticationRequired,
        ),
        (
            exit(1, b"", b"KeyError: carousel_media; extractor changed"),
            AnalysisErrorCode::ExtractorOutdated,
        ),
        (
            exit(1, b"", b"temporary failure in name resolution"),
            AnalysisErrorCode::NetworkFailed,
        ),
    ] {
        let process = FakeProcess::returning(response);
        let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
        assert_eq!(
            adapter
                .extract_post(&source, &CancellationToken::new())
                .err(),
            Some(expected)
        );
    }
}

#[test]
fn instagram_login_page_redirect_is_an_authentication_error_during_analysis() {
    let source = validate_media_url(INPUT).unwrap();
    let process = FakeProcess::returning(exit(
        4,
        b"",
        b"[instagram][error] HTTP redirect to login page (https://www.instagram.com/accounts/login/)\n",
    ));
    let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
    assert_eq!(
        adapter
            .extract_post(&source, &CancellationToken::new())
            .err(),
        Some(AnalysisErrorCode::AuthenticationRequired)
    );
}

#[test]
fn instagram_login_page_redirect_is_an_authentication_error_during_image_download() {
    let inventory = normalize_instagram_inventory("Image_01", OUTPUT).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let process = FakeProcess::returning(exit(
        4,
        b"",
        b"[instagram][error] HTTP redirect to login page (https://www.instagram.com/accounts/login/)\n",
    ));
    let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
    assert_eq!(
        adapter
            .download_image(
                &inventory.items()[0],
                directory.path(),
                &CancellationToken::new(),
            )
            .err(),
        Some(DownloadErrorCode::AuthenticationRequired)
    );
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
}

#[test]
fn rejects_non_post_sources_and_pre_canceled_requests_without_launching() {
    for input in [
        "https://www.instagram.com/reel/Video_01/",
        "https://youtu.be/91YrsgWQcEo",
    ] {
        let process = FakeProcess::returning(exit(0, OUTPUT, b""));
        let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
        assert_eq!(
            adapter
                .extract_post(
                    &validate_media_url(input).unwrap(),
                    &CancellationToken::new()
                )
                .err(),
            Some(AnalysisErrorCode::UnsupportedMedia)
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
    let process = FakeProcess::returning(exit(0, OUTPUT, b""));
    let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
    let canceled = CancellationToken::new();
    canceled.cancel();
    assert_eq!(
        adapter
            .extract_post(&validate_media_url(INPUT).unwrap(), &canceled)
            .err(),
        Some(AnalysisErrorCode::ProcessFailed)
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

struct AssetProcess {
    extra_file: bool,
    metadata_stdout: Vec<u8>,
    calls: Mutex<Vec<Invocation>>,
}

impl ProcessSupervisor for AssetProcess {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let (stdout_limit, stderr_limit) = match request.output_mode {
            OutputMode::Capture {
                stdout_limit,
                stderr_limit,
            } => (stdout_limit, stderr_limit),
            _ => panic!("capture required"),
        };
        let program = request.program.path().to_owned();
        self.calls.lock().unwrap().push(Invocation {
            program: program.clone(),
            args: request.args,
            timeout: request.timeout,
            stdout_limit,
            stderr_limit,
            canceled: cancellation.is_canceled(),
        });
        if program.ends_with("gallery-dl.exe") {
            fs::write(
                request.working_dir.join("stage.jpg"),
                [0xff, 0xd8, 0xff, 0, 1],
            )
            .unwrap();
            if self.extra_file {
                fs::write(request.working_dir.join("unexpected.txt"), b"bad").unwrap();
            }
            exit(0, &self.metadata_stdout, b"")
        } else {
            exit(
                0,
                br#"{"streams":[{"codec_name":"mjpeg","codec_type":"video","width":1080,"height":1350}],"format":{"format_name":"image2","size":"5"}}"#,
                b"",
            )
        }
    }
}

#[test]
fn preview_downloads_one_closed_range_and_verifies_original_image_bytes() {
    let inventory = normalize_instagram_inventory("Image_01", OUTPUT).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let process = AssetProcess {
        extra_file: false,
        metadata_stdout: OUTPUT.to_vec(),
        calls: Mutex::new(Vec::new()),
    };
    let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
    let verified = adapter
        .fetch_preview(
            &inventory.items()[0],
            directory.path(),
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(verified.container(), "jpg");
    assert_eq!(verified.mime_type(), Some("image/jpeg"));
    assert_eq!((verified.width(), verified.height()), (1080, 1350));
    assert_eq!(verified.byte_length(), 5);
    assert_eq!(verified.sha256().len(), 64);
    assert_eq!(fs::read(verified.path()).unwrap(), [0xff, 0xd8, 0xff, 0, 1]);
    let copied = directory.path().join("published-copy.jpg");
    fs::copy(verified.path(), &copied).unwrap();
    verified.verify_byte_identical_copy(&copied).unwrap();
    fs::write(&copied, [0xff, 0xd8, 0xff, 0, 2]).unwrap();
    assert_eq!(
        verified.verify_byte_identical_copy(&copied),
        Err(DownloadErrorCode::OutputInvalid)
    );

    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let args = calls[0]
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.windows(2).any(|pair| pair == ["--range", "1"]));
    assert!(args.windows(2).any(|pair| pair == ["--directory", "."]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--filename", "stage.{extension}"]));
    assert!(args
        .windows(2)
        .any(|pair| { pair == ["--option", "extractor.instagram.videos=false"] }));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--postprocessor", "metadata"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--postprocessor-option", "mode=jsonl"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--postprocessor-option", "event=prepare"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--postprocessor-option", "filename=-"]));
    assert_eq!(args.last().unwrap(), INPUT);
    assert!(!args.join(" ").to_ascii_lowercase().contains("cookie"));
    assert_eq!(calls[1].program, PathBuf::from(r"C:\managed\ffprobe.exe"));
}

#[test]
fn rejects_multiple_downloaded_files_before_probe() {
    let inventory = normalize_instagram_inventory("Image_01", OUTPUT).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let process = AssetProcess {
        extra_file: true,
        metadata_stdout: OUTPUT.to_vec(),
        calls: Mutex::new(Vec::new()),
    };
    let adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &process);
    assert_eq!(
        adapter
            .fetch_preview(
                &inventory.items()[0],
                directory.path(),
                &CancellationToken::new(),
            )
            .err(),
        Some(AnalysisErrorCode::InvalidMetadata)
    );
    assert_eq!(process.calls.lock().unwrap().len(), 1);
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
}

#[test]
fn preview_and_download_reject_reordered_replaced_or_type_changed_items() {
    let inventory = normalize_instagram_inventory("Image_01", OUTPUT).unwrap();
    let expected = &inventory.items()[0];
    let changed = [
        r#"{"category":"instagram","post_shortcode":"Image_01","post_url":"https://www.instagram.com/p/Image_01/","type":"post","description":"Single image","username":"photographer","num":2,"media_id":"200","display_url":"https://scontent.cdninstagram.com/other.jpg","video_url":null,"width":1080,"height":1350,"extension":"jpg"}"#,
        r#"{"category":"instagram","post_shortcode":"Image_01","post_url":"https://www.instagram.com/p/Image_01/","type":"post","description":"Single image","username":"photographer","num":1,"media_id":"replacement","display_url":"https://scontent.cdninstagram.com/replacement.jpg","video_url":null,"width":1080,"height":1350,"extension":"jpg"}"#,
        r#"{"category":"instagram","post_shortcode":"Image_01","post_url":"https://www.instagram.com/p/Image_01/","type":"post","description":"Single image","username":"photographer","num":1,"media_id":"10001","display_url":"https://scontent.cdninstagram.com/cover.jpg","video_url":"https://scontent.cdninstagram.com/video.mp4","width":1080,"height":1350,"extension":"mp4"}"#,
    ];

    for metadata in changed {
        let preview_dir = tempfile::tempdir().unwrap();
        let preview_process = AssetProcess {
            extra_file: false,
            metadata_stdout: format!("{metadata}\n").into_bytes(),
            calls: Mutex::new(Vec::new()),
        };
        let preview_adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &preview_process);
        assert_eq!(
            preview_adapter
                .fetch_preview(expected, preview_dir.path(), &CancellationToken::new())
                .err(),
            Some(AnalysisErrorCode::ContentChanged),
            "{metadata}"
        );

        let download_dir = tempfile::tempdir().unwrap();
        let download_process = AssetProcess {
            extra_file: false,
            metadata_stdout: format!("{metadata}\n").into_bytes(),
            calls: Mutex::new(Vec::new()),
        };
        let download_adapter = GalleryDlInstagramAdapter::new(&FakeResolver, &download_process);
        assert_eq!(
            download_adapter
                .download_image(expected, download_dir.path(), &CancellationToken::new())
                .err(),
            Some(DownloadErrorCode::ContentChanged),
            "{metadata}"
        );
    }
}
