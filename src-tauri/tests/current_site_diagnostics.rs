//! Opt-in live regression of the user-provided links, using production adapters.
#[path = "support/real_media.rs"]
mod real_media;
use local_media_downloader_lib::{
    application::{
        download_executor::{DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink},
        instagram_analysis::{InstagramAnalysisService, ResolvedInstagramDownloadKind},
        portable_package::EMBEDDED_TOOL_MANIFEST,
        process::CancellationToken,
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        download::DownloadProgress,
        instagram::{InstagramDownloadIntent, InstagramItemAvailability, InstagramMediaKind},
        managed_tools::{ManagedToolError, ManagedToolManifest, ToolName, BUILD_TARGET},
    },
    infrastructure::{
        ffmpeg::FfmpegMediaProcessor,
        ffprobe::FfprobeOutputVerifier,
        gallery_dl::{
            FallbackInstagramAssetAcquirer, FallbackInstagramInventoryExtractor,
            GalleryDlInstagramAdapter, InstagramAssetAcquirer,
        },
        instagram_tool::{
            ToolFallbackInstagramInventoryExtractor, YtDlpInstagramInventoryExtractor,
        },
        instagram_web::{
            InstagramWebAssetAcquirer, InstagramWebInventoryExtractor, ReqwestInstagramWebTransport,
        },
        process::WindowsProcessSupervisor,
        ytdlp::{download::YtDlpDownloadExecutor, YtDlpInstagramVideoAnalyzer},
    },
};
use std::{fs, path::PathBuf, sync::Arc, time::Instant};
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

#[test]
#[ignore = "requires pinned local tools, explicit live-test consent and public regression links"]
fn original_instagram_post_and_reel_analyze_preview_and_download() {
    use local_media_downloader_lib::application::output_verifier::OutputVerifier;
    let tools = real_media::tools();
    assert_eq!(
        fs::read(tools.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let resolver = Arc::new(Resolver(tools));
    let process = Arc::new(WindowsProcessSupervisor);
    let root = tempfile::tempdir().unwrap();
    let inventory = FallbackInstagramInventoryExtractor::new(
        GalleryDlInstagramAdapter::new(resolver.clone(), process.clone()),
        ToolFallbackInstagramInventoryExtractor::new(
            InstagramWebInventoryExtractor::new(ReqwestInstagramWebTransport::default()),
            YtDlpInstagramInventoryExtractor::new(resolver.clone(), process.clone()),
        ),
    );
    let asset = FallbackInstagramAssetAcquirer::new(
        GalleryDlInstagramAdapter::new(resolver.clone(), process.clone()),
        InstagramWebAssetAcquirer::new(
            ReqwestInstagramWebTransport::default(),
            resolver.clone(),
            process.clone(),
        ),
    );
    let video = YtDlpInstagramVideoAnalyzer::new(resolver.clone(), process.clone());
    let service =
        InstagramAnalysisService::new_in(inventory, video, asset, root.path().join("cache"));
    for (slot, url, count) in [
        (0, "https://www.instagram.com/p/DdyQQ5XE1je/", 20),
        (1, "https://www.instagram.com/reel/Det5ABjSXVU/", 1),
    ] {
        let start = Instant::now();
        let preview = service
            .analyze(url, Instant::now())
            .unwrap_or_else(|e| panic!("Instagram sample {slot} analyze: {e:?}"));
        assert_eq!(preview.items.len(), count);
        for item in &preview.items {
            assert_eq!(
                item.availability,
                InstagramItemAvailability::Ready,
                "sample {slot}, item {}: {:?}",
                item.ordinal,
                item.unavailable_reason
            );
        }
        assert_eq!(
            preview
                .items
                .iter()
                .filter(|i| i.kind == InstagramMediaKind::Video)
                .count(),
            1
        );
        if slot == 0 {
            assert_eq!(preview.items[5].kind, InstagramMediaKind::Video);
        }
        eprintln!(
            "Instagram sample {slot}: {count} ready items; analysisMs={}",
            start.elapsed().as_millis()
        );
        let selected = preview
            .items
            .iter()
            .filter(|i| i.kind == InstagramMediaKind::Video || i.ordinal == 1)
            .map(|i| i.item_id.clone())
            .collect();
        let resolved = service
            .resolve_download(
                &preview.analysis_id,
                InstagramDownloadIntent {
                    item_ids: selected,
                    compatibility_mode: false,
                },
                Instant::now(),
            )
            .unwrap();
        for item in resolved.items() {
            let staging = root.path().join(format!("90{slot}{}", item.ordinal()));
            fs::create_dir(&staging).unwrap();
            let public = item.public_item();
            if public.preview_available {
                let thumbnail =
                    service.load_preview(&preview.analysis_id, &public.item_id, Instant::now());
                assert!(
                    thumbnail.is_ok(),
                    "sample {slot} preview {}: {:?}",
                    item.ordinal(),
                    thumbnail.err()
                );
            } else {
                // Reels use the existing embedded player, not the post-image API.
                assert_eq!(slot, 1);
            }
            match item.kind() {
                ResolvedInstagramDownloadKind::Image(image) => {
                    let verified = service
                        .preview_loader()
                        .download_image(image, &staging, &CancellationToken::new())
                        .unwrap_or_else(|e| panic!("image download: {e:?}"));
                    assert!(verified.byte_length() > 0);
                    eprintln!("Instagram image verified: {} bytes", verified.byte_length());
                }
                ResolvedInstagramDownloadKind::Video(selector) => {
                    let analyzer =
                        YtDlpInstagramVideoAnalyzer::new(resolver.clone(), process.clone());
                    let analyzed = if let Some(ordinal) = selector.playlist_item() {
                        analyzer.analyze_post_item(
                            resolved.source(),
                            ordinal,
                            selector.expected_media_id().unwrap(),
                            false,
                            &CancellationToken::new(),
                        )
                    } else {
                        analyzer.analyze_reel(resolved.source(), false, &CancellationToken::new())
                    }
                    .unwrap_or_else(|e| panic!("video reanalysis: {e:?}"));
                    let plan = analyzed.plan().clone();
                    let selected = |id: &Option<String>| {
                        id.as_ref()
                            .and_then(|id| analyzed.media().formats.iter().find(|f| &f.id == id))
                            .cloned()
                    };
                    let output = YtDlpDownloadExecutor::new(
                        resolver.clone(),
                        process.clone(),
                        FfmpegMediaProcessor::new(resolver.clone(), process.clone()),
                    )
                    .download(
                        DownloadExecutionRequest {
                            job_id: format!("90{slot}{}", item.ordinal()),
                            source: resolved.source().clone(),
                            playlist_item: selector.playlist_item(),
                            instagram_selector: Some(selector.clone()),
                            x_source: None,
                            bilibili_source: None,
                            plan: plan.clone(),
                            video_source: selected(&plan.video_format_id),
                            audio_source: selected(&plan.audio_format_id),
                            staging_dir: staging,
                        },
                        CancellationToken::new(),
                        &Progress,
                    )
                    .unwrap_or_else(|e| panic!("sample {slot} video download: {e:?}"));
                    let verified = FfprobeOutputVerifier::new(resolver.clone(), process.clone())
                        .verify(output, &plan, CancellationToken::new())
                        .unwrap_or_else(|e| panic!("video verification: {e:?}"));
                    eprintln!(
                        "Instagram sample {slot} video verified: {} bytes {}",
                        verified.byte_length, verified.container
                    );
                }
            }
        }
    }
}
