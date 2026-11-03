#[path = "support/real_media.rs"]
mod real_media;

use local_media_downloader_lib::{
    application::{
        instagram_download_job::{
            InstagramOutputPublisher, InstagramPublicationRequest, InstagramVerifiedAsset,
        },
        media_url::validate_media_url,
        portable_package::EMBEDDED_TOOL_MANIFEST,
        process::CancellationToken,
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        instagram::InstagramSourceKind,
        managed_tools::{ManagedToolError, ManagedToolManifest, ToolName, BUILD_TARGET},
        media::AnalysisErrorCode,
    },
    infrastructure::{
        gallery_dl::InstagramAssetAcquirer,
        instagram_web::{
            parse_web_response, InstagramGraphQlRequest, InstagramWebAssetAcquirer,
            InstagramWebTransport,
        },
        output::instagram::FilesystemInstagramOutputPublisher,
        process::WindowsProcessSupervisor,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct PinnedResolver {
    root: PathBuf,
    manifest: ManagedToolManifest,
}
impl ManagedToolResolver for PinnedResolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        assert_eq!(name, ToolName::Ffprobe);
        Ok(ResolvedTool {
            name,
            version: self
                .manifest
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap()
                .version
                .clone(),
            path: self.root.join(name.artifact_name(BUILD_TARGET)),
        })
    }
}

struct ImageBytes(Vec<u8>);
impl InstagramWebTransport for ImageBytes {
    fn fetch_post(
        &self,
        _: &InstagramGraphQlRequest,
        _: &CancellationToken,
    ) -> Result<Vec<u8>, AnalysisErrorCode> {
        unreachable!()
    }
    fn fetch_asset(
        &self,
        _: &url::Url,
        destination: &Path,
        maximum: u64,
        _: &CancellationToken,
    ) -> Result<(), AnalysisErrorCode> {
        assert!(self.0.len() as u64 <= maximum);
        fs::write(destination, &self.0).unwrap();
        Ok(())
    }
}

#[test]
#[ignore = "requires exact portable tools; generates local JPEG/PNG/WebP and uses real FFprobe, no platform requests"]
fn real_images_preview_download_and_publish_preserve_actual_format_and_bytes() {
    let package = real_media::tools();
    assert_eq!(
        fs::read(package.join("manifest.json")).unwrap(),
        EMBEDDED_TOOL_MANIFEST
    );
    let resolver = PinnedResolver {
        root: package.clone(),
        manifest: serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).unwrap(),
    };
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("下载 图片");
    fs::create_dir(&destination).unwrap();
    let source = validate_media_url("https://www.instagram.com/p/Image_01/").unwrap();
    let metadata = serde_json::to_vec(&serde_json::json!({"data": {"xdt_shortcode_media": {
        "shortcode": "Image_01", "id": "100", "is_video": false,
        "dimensions": {"width": 128, "height": 96},
        "display_url": "https://scontent.cdninstagram.com/image.jpg"
    }}}))
    .unwrap();
    let inventory = parse_web_response(&source, &metadata).unwrap();
    let item = &inventory.items()[0];
    assert_eq!(item.public_item().container, "jpg");
    for (index, (extension, codec, mime)) in [
        ("jpg", "mjpeg", "image/jpeg"),
        ("png", "png", "image/png"),
        ("webp", "libwebp", "image/webp"),
        ("avif", "libaom-av1", "image/avif"),
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = root.path().join(format!("fixture.{extension}"));
        real_media::checked(
            Command::new(package.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET)))
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-n",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=red:s=128x96",
                    "-frames:v",
                    "1",
                    "-threads",
                    "1",
                    "-c:v",
                    codec,
                ])
                .arg(&fixture)
                .output()
                .unwrap(),
        );
        let original = fs::read(&fixture).unwrap();
        let acquirer = InstagramWebAssetAcquirer::new(
            ImageBytes(original.clone()),
            &resolver,
            WindowsProcessSupervisor,
        );
        let preview_dir = tempfile::tempdir_in(root.path()).unwrap();
        let preview = acquirer
            .fetch_preview(item, preview_dir.path(), &CancellationToken::new())
            .unwrap_or_else(|error| {
                let diagnostic = tempfile::tempdir_in(root.path()).unwrap();
                let download_error = acquirer.download_image(item, diagnostic.path(), &CancellationToken::new()).err();
                panic!("{extension} preview failed: {error:?}; download diagnostic: {download_error:?}")
            })
            .unwrap();
        assert_eq!(preview.mime_type(), Some(mime));
        assert_eq!((preview.width(), preview.height()), (128, 96));
        assert_eq!(fs::read(preview.path()).unwrap(), original);

        let job = (42 + index).to_string();
        let stage = root.path().join("jobs").join(&job).join("01");
        fs::create_dir_all(&stage).unwrap();
        let image = acquirer
            .download_image(item, &stage, &CancellationToken::new())
            .unwrap();
        assert_eq!(image.container(), extension);
        assert_eq!(fs::read(image.path()).unwrap(), original);
        let publisher = FilesystemInstagramOutputPublisher::new();
        let publication = publisher
            .prepare(
                &destination,
                &InstagramPublicationRequest::new(
                    &job,
                    InstagramSourceKind::Post,
                    "Image_01",
                    "Local image fixture",
                    None,
                    1,
                )
                .unwrap(),
            )
            .unwrap();
        let name = publisher
            .publish(
                InstagramVerifiedAsset::new(
                    image.path().to_owned(),
                    image.container(),
                    image.byte_length(),
                )
                .unwrap(),
                &publication,
                item.public_item(),
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(name, format!("01.{extension}"));
        assert_eq!(fs::read(destination.join(name)).unwrap(), original);
    }
}
