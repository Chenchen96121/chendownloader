use super::{
    tool::TikTokToolResolver,
    url_policy::{normalize_tiktok_input, validate_tiktok_asset_url},
};
use crate::{
    application::{
        media_url::{validate_media_url, ValidatedMediaUrl},
        tiktok_source::*,
    },
    domain::{
        media::{AnalysisErrorCode, Platform},
        tiktok::{TikTokItemAvailability, TikTokMediaKind},
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    post_id: String,
    title: String,
    creator: Option<String>,
    duration: Option<f64>,
    items: Vec<Value>,
    context: Value,
    source_page_url: String,
}

fn text(input: &str, max: usize) -> String {
    input.chars().filter(|c|!c.is_control() && !matches!(*c,'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).take(max).collect::<String>().trim().to_owned()
}

pub fn parse_tiktok_inventory(
    raw: &Value,
    source: &ValidatedMediaUrl,
) -> Result<TikTokInventory, AnalysisErrorCode> {
    let data: Wire =
        serde_json::from_value(raw.clone()).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let canonical = validate_media_url(&data.source_page_url)?;
    if source.platform() != Platform::Tiktok
        || canonical.platform() != Platform::Tiktok
        || data.post_id != source.media_id()
        || canonical.media_id() != source.media_id()
    {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    if data.items.is_empty() || data.items.len() > 100 || !data.context.is_object() {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let mut ids = std::collections::HashSet::new();
    let mut items = Vec::new();
    for (index, item) in data.items.iter().enumerate() {
        let id = item
            .get("itemId")
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        if !ids.insert(id) || item.get("ordinal").and_then(Value::as_u64) != Some(index as u64 + 1)
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let kind: TikTokMediaKind = serde_json::from_value(
            item.get("kind")
                .cloned()
                .ok_or(AnalysisErrorCode::InvalidMetadata)?,
        )
        .map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        let addresses = item
            .get("urls")
            .and_then(Value::as_array)
            .filter(|rows| !rows.is_empty() && rows.len() <= 3)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let urls = addresses
            .iter()
            .map(|url| {
                validate_tiktok_asset_url(url.as_str().ok_or(AnalysisErrorCode::InvalidMetadata)?)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let previews = item
            .get("previewUrls")
            .and_then(Value::as_array)
            .filter(|rows| rows.len() <= 3)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let preview = previews
            .iter()
            .map(|url| {
                validate_tiktok_asset_url(url.as_str().ok_or(AnalysisErrorCode::InvalidMetadata)?)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .next();
        let dim = |key: &str| {
            item.get(key)
                .and_then(Value::as_u64)
                .filter(|v| (1..=16384).contains(v))
                .map(|v| v as u32)
        };
        let width = dim("width");
        let height = dim("height");
        let identity = item
            .get("identity")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 8192)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let mut native_item = item.clone();
        native_item
            .as_object_mut()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?
            .insert("sourcePageUrl".into(), canonical.url().into());
        items.push(TikTokSourceItem {
            native: Some(Arc::new(TikTokNativeAsset {
                data: native_item,
                context: data.context.clone(),
            })),
            identity: Some(TikTokAttachmentIdentity {
                post_id: data.post_id.clone(),
                media_id: id.to_owned(),
                media_key: Some(identity.to_owned()),
            }),
            ordinal: index as u16 + 1,
            kind,
            image_candidates: if kind == TikTokMediaKind::Image {
                urls.iter()
                    .map(|url| TikTokImageCandidate {
                        url: url.clone(),
                        width,
                        height,
                    })
                    .collect()
            } else {
                Vec::new()
            },
            video_candidates: if kind != TikTokMediaKind::Image {
                urls.iter()
                    .map(|url| TikTokVideoCandidate {
                        url: url.clone(),
                        content_type: "video/mp4".into(),
                        bitrate: item.get("videoBitrateKbps").and_then(Value::as_f64),
                    })
                    .collect()
            } else {
                Vec::new()
            },
            preview,
            width,
            height,
            duration_seconds: data.duration,
            availability: TikTokItemAvailability::Ready,
            unavailable_reason: None,
        });
    }
    let title = text(&data.title, 512);
    if title.is_empty() {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(TikTokInventory {
        source: canonical,
        title,
        creator: data
            .creator
            .map(|s| text(&s, 256))
            .filter(|s| !s.is_empty()),
        items,
    })
}

pub struct TikTokInventoryAdapter {
    pub tool: Arc<TikTokToolResolver>,
    pub metadata: Option<Arc<dyn TikTokMetadataExtractor>>,
}
pub fn network_json(context: &TikTokRequestContext) -> Value {
    json!({"proxy":context.network.proxies.https.as_ref().map(|u|u.as_str()),"bypass":context.network.proxies.bypass.as_deref().map(|s|s.split(',').collect::<Vec<_>>()).unwrap_or_default()})
}
impl TikTokInventoryAdapter {
    fn fetch(&self, url: &str, context: &TikTokRequestContext) -> Result<Value, AnalysisErrorCode> {
        let seconds = context
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .min(120);
        if seconds == 0 || context.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisTimeout);
        }
        self.tool.run(&json!({"protocolVersion":1,"operation":"analyze","url":url,"network":network_json(context)}),seconds,context.cancellation.clone(),None)
    }
}
impl TikTokInventoryExtractor for TikTokInventoryAdapter {
    fn resolve_url(
        &self,
        input: &str,
        context: &TikTokRequestContext,
    ) -> Result<ValidatedMediaUrl, AnalysisErrorCode> {
        let url = normalize_tiktok_input(input)?;
        if let Ok(source) = validate_media_url(&url) {
            return Ok(source);
        }
        let data = self.fetch(&url, context)?;
        let canonical = data
            .get("sourcePageUrl")
            .and_then(Value::as_str)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let source = validate_media_url(canonical)?;
        if source.platform() != Platform::Tiktok {
            return Err(AnalysisErrorCode::UnsupportedPlatform);
        }
        *context
            .prefetched
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)? = Some(data);
        Ok(source)
    }
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        context: &TikTokRequestContext,
    ) -> Result<TikTokInventory, AnalysisErrorCode> {
        let prefetched = context
            .prefetched
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?
            .take();
        let data = match prefetched {
            Some(data) => data,
            None => match self.fetch(source.url(), context) {
                Ok(data) => data,
                Err(error) => {
                    if !matches!(
                        error,
                        AnalysisErrorCode::NetworkFailed
                            | AnalysisErrorCode::AccessDenied
                            | AnalysisErrorCode::AuthenticationRequired
                            | AnalysisErrorCode::ContentUnavailable
                            | AnalysisErrorCode::ExtractorOutdated
                    ) || context.cancellation.is_canceled()
                    {
                        return Err(error);
                    }
                    let Some(metadata) = &self.metadata else {
                        return Err(error);
                    };
                    if !source.url().contains("/video/") {
                        return Err(error);
                    }
                    let value = metadata.extract(source, context)?;
                    crate::infrastructure::ytdlp::tiktok::inventory_from_ytdlp(&value, source)?
                }
            },
        };
        parse_tiktok_inventory(&data, source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_bypass_keeps_the_raw_proxy_for_other_cdn_hosts() {
        let mut network = crate::application::network_profile::NetworkProfile::current();
        network.proxies.https = Some(url::Url::parse("http://127.0.0.1:7897").unwrap());
        network.proxies.bypass = Some("www.tiktok.com".into());
        let context = TikTokRequestContext::new(
            Default::default(),
            Instant::now() + std::time::Duration::from_secs(120),
            Arc::new(network),
        );
        assert!(network_json(&context)["proxy"].is_string());
    }
}
