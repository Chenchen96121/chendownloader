use super::{
    url_policy::{validate_x_asset_url, XAssetKind},
    GalleryDlXAdapter,
};
use crate::{
    application::{
        media_url::ValidatedMediaUrl,
        process::{
            OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram, ProcessRequest,
            ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
        x_source::{
            XAttachmentIdentity, XImageCandidate, XInventory, XInventoryExtractor, XRequestContext,
            XSourceItem, XVideoCandidate,
        },
    },
    domain::{
        managed_tools::{ManagedToolErrorCode, ToolName},
        media::{AnalysisErrorCode, Platform},
        x::{XItemAvailability, XMediaKind},
    },
};
use serde_json::Value;
use std::{
    collections::HashSet,
    ffi::OsString,
    time::{Duration, Instant},
};

pub fn parse_x_inventory(
    bytes: &[u8],
    source: &ValidatedMediaUrl,
) -> Result<XInventory, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if source.platform() != Platform::X {
        return Err(AnalysisErrorCode::UnsupportedPlatform);
    }
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(invalid);
    }
    let messages: Vec<Value> = serde_json::from_slice(bytes).map_err(|_| invalid)?;
    if messages.len() > 128 {
        return Err(invalid);
    }
    let mut directory = None;
    for message in &messages {
        let row = message.as_array().ok_or(invalid)?;
        match row.first().and_then(Value::as_i64) {
            Some(-1) => {
                return Err(classify_error(
                    row.get(1).unwrap_or(&Value::Null).to_string().as_bytes(),
                ))
            }
            Some(2) if row.len() == 2 => {
                let data = &row[1];
                let legacy = data.get("legacy").unwrap_or(data);
                let id = legacy
                    .get("id_str")
                    .and_then(Value::as_str)
                    .ok_or(invalid)?;
                if id != source.media_id()
                    || data.get("rest_id").is_some_and(|v| v.as_str() != Some(id))
                {
                    return Err(AnalysisErrorCode::ContentChanged);
                }
                if directory.is_some_and(|old: &Value| old != data) {
                    return Err(invalid);
                }
                directory = Some(data);
            }
            Some(3) if row.len() == 3 => {}
            _ => return Err(invalid),
        }
    }
    let data = directory.ok_or(invalid)?;
    let legacy = data.get("legacy").unwrap_or(data);
    let entities = legacy
        .pointer("/extended_entities/media")
        .and_then(Value::as_array)
        .ok_or(AnalysisErrorCode::UnsupportedMedia)?;
    if entities.is_empty() {
        return Err(AnalysisErrorCode::FormatUnavailable);
    }
    if entities.len() > 32 {
        return Err(invalid);
    }
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    let mut items = Vec::new();
    let mut format_count = 0usize;
    let mut url_bytes = 0usize;
    for (index, entity) in entities.iter().enumerate() {
        let kind = match entity.get("type").and_then(Value::as_str) {
            Some("photo") => XMediaKind::Image,
            Some("video") => XMediaKind::Video,
            Some("animated_gif") => XMediaKind::AnimatedVideo,
            _ => return Err(AnalysisErrorCode::UnsupportedMedia),
        };
        let media_id = entity
            .get("id_str")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()));
        if let Some(id) = media_id {
            if !ids.insert(id) {
                return Err(invalid);
            }
        }
        let key = entity.get("media_key").and_then(Value::as_str);
        if let Some(key) = key {
            if key.len() > 64
                || key.is_empty()
                || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || !keys.insert(key)
            {
                return Err(invalid);
            }
        }
        let identity = media_id.map(|id| XAttachmentIdentity {
            post_id: source.media_id().into(),
            media_id: id.into(),
            media_key: key.map(str::to_owned),
        });
        let width = dimension(entity.pointer("/original_info/width"));
        let height = dimension(entity.pointer("/original_info/height"));
        let preview = entity
            .get("media_url_https")
            .and_then(Value::as_str)
            .and_then(|s| validate_x_asset_url(s, XAssetKind::Image).ok());
        let mut images = Vec::new();
        let mut videos = Vec::new();
        let mut reason = if identity.is_none() {
            Some(invalid)
        } else {
            None
        };
        if entity
            .pointer("/ext_media_availability/status")
            .and_then(Value::as_str)
            == Some("Unavailable")
        {
            reason = Some(AnalysisErrorCode::ContentUnavailable)
        }
        if reason.is_none() {
            if kind == XMediaKind::Image {
                if let Some(url) = preview.as_ref().and_then(original_image) {
                    images.push(XImageCandidate { url, width, height })
                } else {
                    reason = Some(invalid)
                }
            } else if let Some(variants) = entity
                .pointer("/video_info/variants")
                .and_then(Value::as_array)
            {
                format_count += variants.len();
                for variant in variants {
                    let candidate = (|| {
                        let raw = variant.get("url").and_then(Value::as_str).ok_or(invalid)?;
                        let url = validate_x_asset_url(raw, XAssetKind::Video)?;
                        let content_type = variant
                            .get("content_type")
                            .and_then(Value::as_str)
                            .filter(|s| {
                                matches!(
                                    *s,
                                    "video/mp4"
                                        | "application/x-mpegURL"
                                        | "application/vnd.apple.mpegurl"
                                )
                            })
                            .ok_or(invalid)?
                            .to_owned();
                        let bitrate = variant
                            .get("bitrate")
                            .map(|v| {
                                v.as_f64()
                                    .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1_000_000_000.0)
                                    .ok_or(invalid)
                            })
                            .transpose()?;
                        Ok::<_, AnalysisErrorCode>(XVideoCandidate {
                            url,
                            content_type,
                            bitrate,
                        })
                    })();
                    match candidate {
                        Ok(c) => videos.push(c),
                        Err(e) => {
                            reason = Some(e);
                            break;
                        }
                    }
                }
                if videos.is_empty() {
                    reason = Some(AnalysisErrorCode::FormatUnavailable)
                }
            } else {
                reason = Some(AnalysisErrorCode::FormatUnavailable)
            }
        }
        if reason.is_some() {
            images.clear();
            videos.clear();
        }
        url_bytes += preview.as_ref().map_or(0, |u| u.as_str().len())
            + images.iter().map(|c| c.url.as_str().len()).sum::<usize>()
            + videos.iter().map(|c| c.url.as_str().len()).sum::<usize>();
        if format_count > 512 || url_bytes > 1024 * 1024 {
            return Err(invalid);
        }
        items.push(XSourceItem {
            identity,
            ordinal: (index + 1) as u16,
            kind,
            image_candidates: images,
            video_candidates: videos,
            preview,
            width,
            height,
            duration_seconds: entity
                .pointer("/video_info/duration_millis")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n > 0.0 && *n <= 31_536_000_000.0)
                .map(|n| n / 1000.0),
            availability: if reason.is_some() {
                XItemAvailability::Unavailable
            } else {
                XItemAvailability::Ready
            },
            unavailable_reason: reason,
        });
    }
    let title = clean_text(
        legacy
            .get("full_text")
            .or_else(|| legacy.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("X post"),
        512,
    );
    let creator = data
        .pointer("/core/user_results/result/legacy/screen_name")
        .or_else(|| data.pointer("/core/user_results/result/core/screen_name"))
        .and_then(Value::as_str)
        .map(|s| clean_text(s, 256))
        .filter(|s| !s.is_empty());
    Ok(XInventory {
        source: source.clone(),
        title: if title.is_empty() {
            "X post".into()
        } else {
            title
        },
        creator,
        items,
    })
}
fn dimension(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .filter(|n| (1..=16384).contains(n))
        .map(|n| n as u32)
}
fn clean_text(value: &str, limit: usize) -> String {
    value.chars().filter(|c|!c.is_control()&&!matches!(*c,'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).take(limit).collect::<String>().trim().to_owned()
}
fn original_image(url: &url::Url) -> Option<url::Url> {
    if !url.path().starts_with("/media/") {
        return None;
    }
    let ext = url
        .query_pairs()
        .find(|(k, _)| k == "format")
        .map(|(_, v)| v.into_owned())
        .or_else(|| url.path().rsplit_once('.').map(|(_, v)| v.to_owned()))?;
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "avif") {
        return None;
    }
    let mut result = url.clone();
    let path = result.path().to_owned();
    if let Some((base, suffix)) = path.rsplit_once('.') {
        if suffix == ext {
            result.set_path(base)
        }
    }
    result.set_query(None);
    result
        .query_pairs_mut()
        .append_pair("format", &ext)
        .append_pair("name", "orig");
    Some(result)
}
pub(crate) fn classify_error(bytes: &[u8]) -> AnalysisErrorCode {
    let s = String::from_utf8_lossy(bytes).to_lowercase();
    if s.contains("authentication") || s.contains("login") || s.contains("sign in") {
        AnalysisErrorCode::AuthenticationRequired
    } else if s.contains("429") || s.contains("rate limit") {
        AnalysisErrorCode::RateLimited
    } else if s.contains("403") || s.contains("forbidden") {
        AnalysisErrorCode::AccessDenied
    } else if s.contains("not found") || s.contains("404") || s.contains("deleted") {
        AnalysisErrorCode::ContentUnavailable
    } else if s.contains("timeout") || s.contains("timed out") {
        AnalysisErrorCode::AnalysisTimeout
    } else if s.contains("connection") || s.contains("resolve") || s.contains("network") {
        AnalysisErrorCode::NetworkFailed
    } else {
        AnalysisErrorCode::AnalysisUnavailable
    }
}
struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _: ProcessStream, _: &[u8]) {}
}
impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync> XInventoryExtractor
    for GalleryDlXAdapter<R, P>
{
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        context: &XRequestContext,
    ) -> Result<XInventory, AnalysisErrorCode> {
        if source.platform() != Platform::X {
            return Err(AnalysisErrorCode::UnsupportedPlatform);
        }
        if context.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let timeout = context
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(AnalysisErrorCode::AnalysisTimeout)?
            .min(Duration::from_secs(120));
        let tool = self
            .resolver
            .resolve_verified(ToolName::GalleryDl)
            .map_err(|e| match e.code {
                ManagedToolErrorCode::ToolMissing => AnalysisErrorCode::ToolMissing,
                _ => AnalysisErrorCode::ToolIntegrityFailed,
            })?;
        let working = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let mut args: Vec<OsString> = ["--config-ignore", "--dump-json", "--no-input"]
            .map(Into::into)
            .to_vec();
        for option in [
            "extractor.twitter.transform=false",
            "extractor.twitter.text-tweets=true",
            "extractor.twitter.quoted=false",
            "extractor.twitter.quoted-expand=false",
            "extractor.twitter.conversations=false",
            "extractor.twitter.expand=false",
            "extractor.twitter.cards=false",
            "extractor.twitter.articles=false",
            "extractor.twitter.twitpic=false",
            "extractor.twitter.previews=false",
            "extractor.twitter.videos=true",
            "extractor.twitter.size=orig",
            "output.jsonl=false",
            "output.private=false",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
        args.extend(["--".into(), source.url().into()]);
        let exit = self
            .process
            .run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_resolved_tool(tool)
                        .with_network_profile(context.network.clone()),
                    args,
                    working_dir: working.path().to_owned(),
                    timeout: Some(timeout),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: 16 * 1024 * 1024,
                        stderr_limit: 65536,
                    },
                },
                context.cancellation.clone(),
                &Discard,
            )
            .map_err(|e| match e {
                ProcessFailureKind::TimedOut | ProcessFailureKind::InactivityTimedOut => {
                    AnalysisErrorCode::AnalysisTimeout
                }
                ProcessFailureKind::StdoutLimitExceeded
                | ProcessFailureKind::StderrLimitExceeded
                | ProcessFailureKind::LineLimitExceeded => AnalysisErrorCode::InvalidMetadata,
                _ => AnalysisErrorCode::ProcessFailed,
            })?;
        if context.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        if exit.code != 0 {
            return Err(classify_error(&exit.stderr));
        }
        parse_x_inventory(&exit.stdout, source)
    }
}
