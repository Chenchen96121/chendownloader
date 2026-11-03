//! Experimental, offline-only browser-response probe. Not used by the desktop runtime.

use crate::{
    application::media_url::ValidatedMediaUrl,
    domain::{
        instagram::{InstagramMediaItem, InstagramMediaKind, InstagramUrlKind},
        media::{AnalysisErrorCode, Platform},
    },
    infrastructure::gallery_dl::{normalize_instagram_inventory, InstagramInventory},
};
use serde_json::{json, Value};
use std::collections::HashSet;
use url::Url;

pub const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_HAR_ENTRIES: usize = 2048;
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 48;
const MAX_VARIANTS: usize = 128;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCaptureReport {
    pub source_page_url: String,
    pub shortcode: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    pub items: Vec<InstagramMediaItem>,
    pub capture_only: bool,
    pub download_enabled: bool,
    pub highest_platform_quality_verified: bool,
}

pub fn parse_browser_capture(
    source: &ValidatedMediaUrl,
    capture: &[u8],
) -> Result<BrowserCaptureReport, AnalysisErrorCode> {
    if source.platform() != Platform::Instagram
        || source.instagram_kind() != Some(InstagramUrlKind::Post)
    {
        return Err(AnalysisErrorCode::UnsupportedMedia);
    }
    if capture.is_empty() || capture.len() > MAX_CAPTURE_BYTES {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let root: Value =
        serde_json::from_slice(capture).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let mut snapshots = Snapshots::default();
    if let Some(log) = root.get("log") {
        let entries = log
            .get("entries")
            .and_then(Value::as_array)
            .filter(|entries| entries.len() <= MAX_HAR_ENTRIES)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        for entry in entries {
            let Some(url) = entry.pointer("/request/url").and_then(Value::as_str) else {
                continue;
            };
            if !is_metadata_request(url) {
                continue;
            }
            if !entry
                .pointer("/response/status")
                .and_then(Value::as_u64)
                .is_some_and(|status| (200..300).contains(&status))
            {
                continue;
            }
            let Some(content) = entry.pointer("/response/content") else {
                continue;
            };
            let mime = content
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !mime.contains("json") && mime != "text/javascript" {
                continue;
            }
            let Some(text) = content.get("text").and_then(Value::as_str) else {
                continue;
            };
            if text.len() > MAX_RESPONSE_BYTES || content.get("encoding").is_some() {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            let text = text.trim_start().strip_prefix("for (;;);").unwrap_or(text);
            let body: Value =
                serde_json::from_str(text).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
            collect_posts(source, &body, &mut snapshots)?;
        }
    } else {
        collect_posts(source, &root, &mut snapshots)?;
    }
    let inventory = snapshots
        .inventory
        .ok_or(AnalysisErrorCode::FormatUnavailable)?;
    Ok(BrowserCaptureReport {
        source_page_url: source.url().to_owned(),
        shortcode: inventory.shortcode().to_owned(),
        title: inventory.title().to_owned(),
        creator: inventory.creator().map(str::to_owned),
        items: inventory
            .items()
            .iter()
            .map(|item| item.public_item().clone())
            .collect(),
        capture_only: true,
        download_enabled: false,
        highest_platform_quality_verified: false,
    })
}

#[derive(Default)]
struct Snapshots {
    inventory: Option<InstagramInventory>,
    identity: Vec<(String, InstagramMediaKind)>,
}

impl Snapshots {
    fn add(&mut self, inventory: InstagramInventory) -> Result<(), AnalysisErrorCode> {
        let identity = inventory
            .items()
            .iter()
            .map(|item| (item.media_id().to_owned(), item.public_item().kind))
            .collect::<Vec<_>>();
        if self.inventory.is_some() && self.identity != identity {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        self.identity = identity;
        self.inventory = Some(inventory);
        Ok(())
    }
}

fn collect_posts(
    source: &ValidatedMediaUrl,
    root: &Value,
    snapshots: &mut Snapshots,
) -> Result<(), AnalysisErrorCode> {
    let mut queue = vec![(root, 0usize)];
    let mut visited = 0usize;
    while let Some((value, depth)) = queue.pop() {
        visited += 1;
        if visited > MAX_NODES || depth > MAX_DEPTH {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        match value {
            Value::Object(object) => {
                let code = object
                    .get("shortcode")
                    .or_else(|| object.get("code"))
                    .and_then(Value::as_str);
                if code == Some(source.media_id())
                    && [
                        "edge_sidecar_to_children",
                        "display_url",
                        "image_versions2",
                        "carousel_media",
                    ]
                    .iter()
                    .any(|key| object.contains_key(*key))
                {
                    snapshots.add(normalize_post(source, value)?)?;
                    continue;
                }
                for child in object.values() {
                    queue.push((child, depth + 1));
                }
            }
            Value::Array(array) => {
                for child in array.iter().rev() {
                    queue.push((child, depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn normalize_post(
    source: &ValidatedMediaUrl,
    post: &Value,
) -> Result<InstagramInventory, AnalysisErrorCode> {
    let graphql = post.get("shortcode").is_some();
    let children = if graphql {
        if let Some(sidecar) = post.get("edge_sidecar_to_children") {
            let edges = sidecar
                .get("edges")
                .and_then(Value::as_array)
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            check_count(sidecar.get("count"), edges.len())?;
            edges
                .iter()
                .map(|edge| edge.get("node").ok_or(AnalysisErrorCode::InvalidMetadata))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![post]
        }
    } else if let Some(carousel) = post.get("carousel_media") {
        let children = carousel
            .as_array()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        check_count(post.get("carousel_media_count"), children.len())?;
        children.iter().collect()
    } else {
        vec![post]
    };
    if children.is_empty() || children.len() > 32 {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let title = if graphql {
        post.pointer("/edge_media_to_caption/edges/0/node/text")
    } else {
        post.pointer("/caption/text")
    }
    .and_then(Value::as_str);
    let creator = if graphql {
        post.pointer("/owner/username")
    } else {
        post.pointer("/user/username")
    }
    .and_then(Value::as_str);
    let mut ids = HashSet::new();
    let mut lines = Vec::with_capacity(children.len());
    for (index, item) in children.iter().enumerate() {
        let id = media_id(item.get(if graphql { "id" } else { "pk" }))?;
        if !ids.insert(id.clone()) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let image = if graphql {
            choose_variant(
                item.get("display_resources"),
                "src",
                "config_width",
                "config_height",
            )?
            .or(direct_asset(item, "display_url")?)
        } else {
            choose_variant(
                item.pointer("/image_versions2/candidates"),
                "url",
                "width",
                "height",
            )?
        }
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let video = if graphql {
            let is_video = item
                .get("is_video")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    item.get("__typename")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.ends_with("GraphVideo"))
                });
            if is_video {
                Some(direct_asset(item, "video_url")?.ok_or(AnalysisErrorCode::InvalidMetadata)?)
            } else {
                if item.get("video_url").is_some_and(|url| !url.is_null()) {
                    return Err(AnalysisErrorCode::InvalidMetadata);
                }
                None
            }
        } else {
            match item.get("media_type").and_then(Value::as_u64) {
                Some(2) => Some(
                    choose_variant(item.get("video_versions"), "url", "width", "height")?
                        .ok_or(AnalysisErrorCode::InvalidMetadata)?,
                ),
                Some(1) => None,
                _ => return Err(AnalysisErrorCode::InvalidMetadata),
            }
        };
        let media = video.as_ref().unwrap_or(&image);
        let extension = asset_extension(&media.url)?;
        lines.push(
            serde_json::to_string(&json!({
                "category": "instagram", "type": "post", "post_shortcode": source.media_id(),
                "post_url": source.url(), "description": title, "username": creator,
                "num": index + 1, "media_id": id, "display_url": image.url,
                "video_url": video.as_ref().map(|asset| &asset.url),
                "width": media.width, "height": media.height, "extension": extension
            }))
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?,
        );
    }
    normalize_instagram_inventory(source.media_id(), lines.join("\n").as_bytes())
}

fn check_count(count: Option<&Value>, actual: usize) -> Result<(), AnalysisErrorCode> {
    if count.is_some_and(|count| count.as_u64() != Some(actual as u64)) {
        Err(AnalysisErrorCode::InvalidMetadata)
    } else {
        Ok(())
    }
}

fn media_id(value: Option<&Value>) -> Result<String, AnalysisErrorCode> {
    let value = value.ok_or(AnalysisErrorCode::InvalidMetadata)?;
    match value {
        Value::String(id) => Ok(id.clone()),
        Value::Number(number) => number
            .as_u64()
            .map(|id| id.to_string())
            .ok_or(AnalysisErrorCode::InvalidMetadata),
        _ => Err(AnalysisErrorCode::InvalidMetadata),
    }
}

struct Asset {
    url: String,
    width: Option<u32>,
    height: Option<u32>,
}

fn dimensions(
    value: &Value,
    width: &str,
    height: &str,
) -> Result<(Option<u32>, Option<u32>), AnalysisErrorCode> {
    let w = value.get(width);
    let h = value.get(height);
    match (w, h) {
        (None, None) => Ok((None, None)),
        (Some(w), Some(h)) => {
            let w = w.as_u64().filter(|w| (1..=16_384).contains(w));
            let h = h.as_u64().filter(|h| (1..=16_384).contains(h));
            match (w, h) {
                (Some(w), Some(h)) => Ok((Some(w as u32), Some(h as u32))),
                _ => Err(AnalysisErrorCode::InvalidMetadata),
            }
        }
        _ => Err(AnalysisErrorCode::InvalidMetadata),
    }
}

fn direct_asset(item: &Value, key: &str) -> Result<Option<Asset>, AnalysisErrorCode> {
    let Some(value) = item.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let url = value.as_str().ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let (width, height) = match item.get("dimensions") {
        Some(dimensions_value) => dimensions(dimensions_value, "width", "height")?,
        None => (None, None),
    };
    Ok(Some(Asset {
        url: url.to_owned(),
        width,
        height,
    }))
}

fn choose_variant(
    value: Option<&Value>,
    url_key: &str,
    width_key: &str,
    height_key: &str,
) -> Result<Option<Asset>, AnalysisErrorCode> {
    let Some(value) = value else {
        return Ok(None);
    };
    let variants = value
        .as_array()
        .filter(|variants| variants.len() <= MAX_VARIANTS)
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let mut best: Option<Asset> = None;
    for variant in variants {
        let url = variant
            .get(url_key)
            .and_then(Value::as_str)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let (width, height) = dimensions(variant, width_key, height_key)?;
        let area = u64::from(width.unwrap_or(0)) * u64::from(height.unwrap_or(0));
        if best.as_ref().is_none_or(|best| {
            area > u64::from(best.width.unwrap_or(0)) * u64::from(best.height.unwrap_or(0))
        }) {
            best = Some(Asset {
                url: url.to_owned(),
                width,
                height,
            });
        }
    }
    Ok(best)
}

fn asset_extension(value: &str) -> Result<String, AnalysisErrorCode> {
    let url = Url::parse(value).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    url.path()
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| !extension.is_empty() && extension.len() <= 8)
        .ok_or(AnalysisErrorCode::InvalidMetadata)
}

fn is_metadata_request(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && matches!(url.host_str(), Some("instagram.com" | "www.instagram.com"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none()
        && (matches!(
            url.path().trim_end_matches('/'),
            "/graphql/query" | "/api/graphql"
        ) || (url.path().starts_with("/api/v1/media/") && url.path().ends_with("/info/")))
}
