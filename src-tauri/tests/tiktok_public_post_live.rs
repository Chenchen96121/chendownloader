//! Explicitly opted-in live validation; no URLs, cookies, or raw responses are logged.
#[path = "support/real_media.rs"]
mod real_media;
use local_media_downloader_lib::{
    application::{
        download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
        network_profile::NetworkProfile,
        output_verifier::OutputVerifier,
        portable_package::EMBEDDED_TOOL_MANIFEST,
        process::CancellationToken,
        tiktok_analysis::TikTokAnalysisService,
        tiktok_source::{TikTokInventoryExtractor, TikTokMetadataExtractor, TikTokRequestContext},
        tiktok_video::TikTokVideoAnalyzer,
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadProgress,
        managed_tools::{ManagedToolError, ManagedToolManifest, ToolName, BUILD_TARGET},
        tiktok::TikTokMediaKind,
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        process::WindowsProcessSupervisor,
        tiktok::{
            assets::TikTokAssetAdapter,
            download::TikTokDownloadExecutor,
            inventory::TikTokInventoryAdapter,
            tool::{TikTokToolResolver, TRUSTED_MANIFEST},
        },
        ytdlp::tiktok::{YtDlpTikTokMetadata, YtDlpTikTokVideoAnalyzer},
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
struct Resolver(PathBuf);
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        let manifest: ManagedToolManifest = serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).unwrap();
        Ok(ResolvedTool {
            name,
            version: manifest
                .tools
                .iter()
                .find(|t| t.name == name)
                .unwrap()
                .version
                .clone(),
            path: self.0.join(name.artifact_name(BUILD_TARGET)),
        })
    }
}
struct Progress;
impl DownloadProgressSink for Progress {
    fn on_progress(&self, _: DownloadProgress) {}
}
struct CaptureProcessor<M> {
    inner: M,
    root: PathBuf,
}
impl<M: local_media_downloader_lib::infrastructure::tiktok::download::TikTokSourceInspector>
    local_media_downloader_lib::infrastructure::tiktok::download::TikTokSourceInspector
    for CaptureProcessor<M>
{
    fn verify_silent_source(
        &self,
        source: &std::path::Path,
        staging: &std::path::Path,
        cancel: &CancellationToken,
    ) -> Result<(), local_media_downloader_lib::domain::download::DownloadErrorCode> {
        self.inner.verify_silent_source(source, staging, cancel)
    }
}
impl<M: local_media_downloader_lib::application::local_processing::LocalMediaProcessor>
    local_media_downloader_lib::application::local_processing::LocalMediaProcessor
    for CaptureProcessor<M>
{
    fn process(
        &self,
        request: local_media_downloader_lib::application::local_processing::LocalProcessingRequest,
        cancel: CancellationToken,
    ) -> Result<
        local_media_downloader_lib::application::local_processing::ProcessedMedia,
        local_media_downloader_lib::domain::download::DownloadErrorCode,
    > {
        for source in &request.sources {
            let input = request
                .source_dir
                .join(format!("input-{}/source.mp4", source.slot));
            fs::copy(input, self.root.join(format!("{}.mp4", source.format_id))).unwrap();
        }
        self.inner.process(request, cancel)
    }
}

#[test]
#[ignore = "requires user-supplied public TikTok URLs, pinned tools, helper and network"]
fn tiktok_public_post_live() {
    let tools = real_media::tools();
    let helper =
        PathBuf::from(std::env::var_os("LMD_TIKTOK_CORE_ROOT").expect("helper root required"));
    let source = std::env::var("LMD_TIKTOK_LIVE_URL").expect("test URL required");
    let resolver = Arc::new(Resolver(tools.clone()));
    let process = Arc::new(WindowsProcessSupervisor);
    let tool = Arc::new(TikTokToolResolver::new(helper, TRUSTED_MANIFEST).unwrap());
    tool.resolve_verified().unwrap();
    let metadata: Arc<dyn TikTokMetadataExtractor> =
        Arc::new(YtDlpTikTokMetadata::new(resolver.clone(), process.clone()));
    let inventory = TikTokInventoryAdapter {
        tool: tool.clone(),
        metadata: Some(metadata.clone()),
    };
    let video = YtDlpTikTokVideoAnalyzer {
        metadata,
        probe: Arc::new(
            local_media_downloader_lib::infrastructure::tiktok::probe::NativeTikTokCandidateProbe {
                resolver: resolver.clone(),
                process: process.clone(),
                tool: tool.clone(),
            },
        ),
    };
    let assets = TikTokAssetAdapter::new(resolver.clone(), process.clone(), tool.clone());
    let context = TikTokRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    eprintln!(
        "TikTok captured page proxy route={}",
        context
            .network
            .https_proxy_for_host("www.tiktok.com")
            .is_some()
    );
    let canonical = inventory.resolve_url(&source, &context).unwrap();
    let post = inventory.extract_post(&canonical, &context).unwrap();
    let expected: usize = std::env::var("LMD_TIKTOK_EXPECT_ITEMS")
        .unwrap_or("1".into())
        .parse()
        .unwrap();
    assert_eq!(post.items.len(), expected);
    let temporary = tempfile::tempdir().unwrap();
    let mut original_video = None;
    for item in &post.items {
        let directory = temporary.path().join((100 + item.ordinal).to_string());
        fs::create_dir(&directory).unwrap();
        let context = TikTokRequestContext::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(120),
            context.network.clone(),
        );
        if item.kind == TikTokMediaKind::Image {
            use local_media_downloader_lib::application::tiktok_download_job::{
                TikTokImageDownloader, TikTokVerifiedAsset,
            };
            let TikTokVerifiedAsset::Image(image) =
                assets.download(item, &directory, &context).unwrap()
            else {
                panic!("expected image")
            };
            assert_eq!(Some(image.width()), item.width);
            assert_eq!(Some(image.height()), item.height);
            real_media::checked(
                std::process::Command::new(
                    tools.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET)),
                )
                .args(["-v", "error", "-i"])
                .arg(image.path())
                .args(["-f", "null", "-"])
                .output()
                .unwrap(),
            );
        } else {
            let resolved = video
                .analyze_item(&post, item.identity.as_ref().unwrap(), false, &context)
                .unwrap();
            let (media, plan, selected) = resolved.into_parts();
            let video_source = media
                .formats
                .iter()
                .find(|f| Some(&f.id) == plan.video_format_id.as_ref())
                .cloned();
            let audio_source = media
                .formats
                .iter()
                .find(|f| Some(&f.id) == plan.audio_format_id.as_ref())
                .cloned();
            assert_eq!(video_source.as_ref().unwrap().width, Some(1080));
            assert_eq!(video_source.as_ref().unwrap().height, Some(1920));
            assert_eq!(video_source.as_ref().unwrap().fps, Some(30.0));
            assert_eq!(
                audio_source.as_ref().unwrap().audio_codec.as_deref(),
                Some("aac")
            );
            eprintln!(
                "Selected video {:?} {:?} {:?}; selected audio {:?} kbps from same stream={}",
                video_source.as_ref().and_then(|v| v.video_codec.as_deref()),
                video_source.as_ref().and_then(|v| v.fps),
                video_source.as_ref().and_then(|v| v.video_bitrate_kbps),
                audio_source.as_ref().and_then(|a| a.audio_bitrate_kbps),
                plan.video_format_id == plan.audio_format_id
            );
            let captured = temporary.path().join("captured");
            fs::create_dir(&captured).unwrap();
            let executor = TikTokDownloadExecutor {
                tool: tool.clone(),
                processor: CaptureProcessor {
                    inner: FfmpegMediaProcessor::new(resolver.clone(), process.clone()),
                    root: captured.clone(),
                },
            };
            let staged = executor
                .download(
                    DownloadExecutionRequest {
                        job_id: (100 + item.ordinal).to_string(),
                        source: canonical.clone(),
                        playlist_item: None,
                        instagram_selector: None,
                        x_source: None,
                        bilibili_source: None,
                        tiktok_source: Some(selected),
                        plan: plan.clone(),
                        video_source,
                        audio_source,
                        staging_dir: directory,
                    },
                    context.cancellation.clone(),
                    &Progress,
                )
                .unwrap();
            let verified = FfprobeOutputVerifier::new(resolver.clone(), process.clone())
                .verify(staged, &plan, context.cancellation.clone())
                .unwrap();
            assert!(verified.has_audio && verified.has_video);
            if let Some(original) = std::env::var_os("LMD_TIKTOK_ORIGINAL") {
                let ffmpeg = tools.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET));
                real_media::assert_copy_equivalent(
                    &ffmpeg,
                    &PathBuf::from(&original),
                    &verified.path,
                    real_media::StreamRole::Video,
                );
            }
            let ffmpeg = tools.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET));
            real_media::assert_copy_equivalent(
                &ffmpeg,
                &captured.join(format!("{}.mp4", plan.video_format_id.as_ref().unwrap())),
                &verified.path,
                real_media::StreamRole::Video,
            );
            real_media::assert_copy_equivalent(
                &ffmpeg,
                &captured.join(format!("{}.mp4", plan.audio_format_id.as_ref().unwrap())),
                &verified.path,
                real_media::StreamRole::Audio,
            );
            original_video = Some(verified.path);
        }
    }
    // Exercise the safe public DTO path too; no private addresses are emitted.
    let service = TikTokAnalysisService::new(inventory, video, assets);
    let preview = service.analyze(&source, Instant::now()).unwrap();
    preview.validate().unwrap();
    assert_eq!(preview.items.len(), expected);
    assert!(preview.items.iter().all(|item| item.availability
        == local_media_downloader_lib::domain::tiktok::TikTokItemAvailability::Ready));
    if let Some(video) = original_video {
        assert!(video.is_file());
    }
    eprintln!(
        "TikTok verified {expected} media items, original dimensions and full decode checks passed"
    );
}
