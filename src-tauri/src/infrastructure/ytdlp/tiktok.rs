//! TikTok extractor candidates supplement the direct API; music-only formats are excluded.
use crate::{
    application::{
        media_url::ValidatedMediaUrl,
        process::{
            OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest, ProcessStream,
            ProcessSupervisor,
        },
        tiktok_source::{
            TikTokAttachmentIdentity, TikTokInventory, TikTokMetadataExtractor, TikTokNativeAsset,
            TikTokRequestContext,
        },
        tiktok_video::{resolve_tiktok_video, ResolvedTikTokVideo, TikTokVideoAnalyzer},
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        managed_tools::ToolName,
        media::{AnalysisErrorCode, Platform},
        output_policy::{normalized_audio_codec, normalized_video_codec},
    },
    infrastructure::tiktok::url_policy::validate_tiktok_asset_url,
};
use serde_json::{json, Value};
use std::{ffi::OsString, sync::Arc, time::Instant};

pub struct YtDlpTikTokMetadata<R, P> {
    resolver: R,
    process: P,
}
impl<R, P> YtDlpTikTokMetadata<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}
struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _: ProcessStream, _: &[u8]) {}
}
impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync>
    TikTokMetadataExtractor for YtDlpTikTokMetadata<R, P>
{
    fn extract(
        &self,
        source: &ValidatedMediaUrl,
        context: &TikTokRequestContext,
    ) -> Result<Value, AnalysisErrorCode> {
        if source.platform() != Platform::Tiktok {
            return Err(AnalysisErrorCode::UnsupportedPlatform);
        }
        let remaining = context.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AnalysisErrorCode::AnalysisTimeout);
        }
        let resolved = self
            .resolver
            .resolve_verified(ToolName::YtDlp)
            .map_err(super::errors::map_tool_error)?;
        let working = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let args = [
            "--ignore-config",
            "--no-cache-dir",
            "--no-playlist",
            "--skip-download",
            "--dump-single-json",
            "--no-progress",
            "--socket-timeout",
            "20",
            "--retries",
            "1",
            "--extractor-retries",
            "0",
            source.url(),
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let exit = self
            .process
            .run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_resolved_tool(resolved)
                        .with_network_profile(context.network.clone())
                        .with_explicit_https_proxy_for_host("www.tiktok.com"),
                    args,
                    working_dir: working.path().to_owned(),
                    timeout: Some(remaining),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: 4 * 1024 * 1024,
                        stderr_limit: 16384,
                    },
                },
                context.cancellation.clone(),
                &Discard,
            )
            .map_err(super::errors::map_process_error)?;
        if exit.code != 0 {
            return Err(super::errors::classify_nonzero_exit(&exit.stderr));
        }
        let metadata: Value =
            serde_json::from_slice(&exit.stdout).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if metadata.get("id").and_then(Value::as_str) != Some(source.media_id()) {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        Ok(metadata)
    }
}

pub fn merge_video_candidates(
    item: &Value,
    metadata: &Value,
    post_id: &str,
) -> Result<Value, AnalysisErrorCode> {
    if metadata.get("id").and_then(Value::as_str) != Some(post_id) {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    let rows = metadata
        .get("formats")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= 512)
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let mut output = item.clone();
    let mut candidates = item
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for row in rows {
        let video = row.get("vcodec").and_then(Value::as_str);
        if video == Some("none") {
            continue;
        }
        if row.get("has_drm").and_then(Value::as_bool) == Some(true) {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        let raw = row
            .get("url")
            .and_then(Value::as_str)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let url = validate_tiktok_asset_url(raw)?;
        let width = row
            .get("width")
            .and_then(Value::as_u64)
            .filter(|n| (1..=16384).contains(n))
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let height = row
            .get("height")
            .and_then(Value::as_u64)
            .filter(|n| (1..=16384).contains(n))
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let codec = normalized_video_codec(video).ok_or(AnalysisErrorCode::FormatUnavailable)?;
        let audio = normalized_audio_codec(row.get("acodec").and_then(Value::as_str));
        if let Some(existing) = candidates.iter_mut().find(|c| {
            c.get("urls").and_then(Value::as_array).is_some_and(|urls| {
                urls.iter().any(|u| {
                    u.as_str()
                        .and_then(|s| url::Url::parse(s).ok())
                        .is_some_and(|u| u.path() == url.path())
                })
            }) && c.get("width").and_then(Value::as_u64) == Some(width)
                && c.get("height").and_then(Value::as_u64) == Some(height)
        }) {
            if existing.get("audioCodec").and_then(Value::as_str).is_none() {
                if let Some(audio) = audio {
                    existing["audioCodec"] = audio.as_str().into();
                    existing["audioBitrateKbps"] = row.get("abr").cloned().unwrap_or(Value::Null);
                } else if row.get("acodec").and_then(Value::as_str) == Some("none") {
                    existing["audioAbsent"] = true.into();
                    existing
                        .as_object_mut()
                        .ok_or(AnalysisErrorCode::InvalidMetadata)?
                        .remove("audioBitrateKbps");
                }
            }
            if existing.get("fps").is_none_or(Value::is_null) {
                existing["fps"] = row.get("fps").cloned().unwrap_or(Value::Null);
            }
            if existing.get("dynamicRange").is_none_or(Value::is_null) {
                existing["dynamicRange"] = row.get("dynamic_range").cloned().unwrap_or(Value::Null);
            }
            if existing.get("audioChannels").is_none_or(Value::is_null) {
                existing["audioChannels"] =
                    row.get("audio_channels").cloned().unwrap_or(Value::Null);
            }
            if existing.get("audioSampleRateHz").is_none_or(Value::is_null) {
                existing["audioSampleRateHz"] = row.get("asr").cloned().unwrap_or(Value::Null);
            }
            continue;
        }
        let id = row
            .get("format_id")
            .and_then(Value::as_str)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let file_size = row
            .get("filesize")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n < 9_007_199_254_740_992)
            .map(|n| json!({"bytes":n,"isApproximate":false}));
        candidates.push(json!({"identity":id,"urls":[raw],"width":width,"height":height,"fps":row.get("fps"),"videoCodec":codec,"audioCodec":audio.map(|codec|codec.as_str()),"audioAbsent":row.get("acodec").and_then(Value::as_str)==Some("none"),"audioBitrateKbps":row.get("abr"),"videoBitrateKbps":row.get("vbr").or_else(||row.get("tbr")),"dynamicRange":row.get("dynamic_range"),"container":row.get("ext").and_then(Value::as_str).unwrap_or("mp4"),"fileSize":file_size}));
        let candidate = candidates
            .last_mut()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        candidate["audioChannels"] = row.get("audio_channels").cloned().unwrap_or(Value::Null);
        candidate["audioSampleRateHz"] = row.get("asr").cloned().unwrap_or(Value::Null);
        candidate["requestContext"] = extractor_request_context(metadata, row, &url)?;
    }
    if candidates.is_empty() || candidates.len() > 512 {
        return Err(AnalysisErrorCode::FormatUnavailable);
    }
    output
        .as_object_mut()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?
        .insert("candidates".into(), Value::Array(candidates));
    Ok(output)
}

fn extractor_request_context(
    metadata: &Value,
    row: &Value,
    url: &url::Url,
) -> Result<Value, AnalysisErrorCode> {
    let mut headers = serde_json::Map::new();
    for record in [metadata.get("http_headers"), row.get("http_headers")]
        .into_iter()
        .flatten()
    {
        let record = record
            .as_object()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        for (key, value) in record {
            let canonical = match key.to_ascii_lowercase().as_str() {
                "user-agent" => "User-Agent",
                "referer" => "Referer",
                "origin" => "Origin",
                "accept" => "Accept",
                "accept-language" => "Accept-Language",
                "cookie" => "Cookie",
                "authorization" => "Authorization",
                _ => {
                    let lower = key.to_ascii_lowercase();
                    if matches!(
                        lower.as_str(),
                        "host"
                            | "content-length"
                            | "transfer-encoding"
                            | "connection"
                            | "upgrade"
                            | "te"
                            | "range"
                    ) || lower.starts_with("proxy-")
                    {
                        continue;
                    }
                    if key.is_empty()
                        || key.len() > 64
                        || !key
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                    {
                        return Err(AnalysisErrorCode::InvalidMetadata);
                    }
                    key.as_str()
                }
            };
            let value = value
                .as_str()
                .filter(|s| s.len() <= 16384 && s.bytes().all(|b| (32..=126).contains(&b)))
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            headers.insert(canonical.into(), value.into());
        }
    }
    if headers.len() > 16 {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    if headers
        .iter()
        .map(|(k, v)| k.len() + v.as_str().map(str::len).unwrap_or(0))
        .sum::<usize>()
        > 16384
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let cookies = row
        .get("cookies")
        .or_else(|| metadata.get("cookies"))
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .filter(|s| s.len() <= 16384 && s.bytes().all(|b| (32..=126).contains(&b)))
                .ok_or(AnalysisErrorCode::InvalidMetadata)
        })
        .transpose()?
        .unwrap_or("");
    Ok(
        json!({"cookies":[],"headers":headers,"serializedCookies":cookies,"requestHost":url.host_str()}),
    )
}

pub fn inventory_from_ytdlp(
    metadata: &Value,
    source: &ValidatedMediaUrl,
) -> Result<Value, AnalysisErrorCode> {
    use sha2::{Digest, Sha256};
    if source.platform() != Platform::Tiktok || !source.url().contains("/video/") {
        return Err(AnalysisErrorCode::UnsupportedMedia);
    }
    let merged = merge_video_candidates(&json!({"candidates":[]}), metadata, source.media_id())?;
    let rows = merged["candidates"]
        .as_array()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let first = rows.first().ok_or(AnalysisErrorCode::FormatUnavailable)?;
    let digest = format!("{:x}", Sha256::digest(b"video"));
    let preview = metadata
        .get("thumbnail")
        .and_then(Value::as_str)
        .and_then(|s| validate_tiktok_asset_url(s).ok())
        .map(|u| vec![u.to_string()])
        .unwrap_or_default();
    Ok(
        json!({"postId":source.media_id(),"title":metadata.get("title").and_then(Value::as_str).unwrap_or("TikTok"),"creator":metadata.get("uploader"),"duration":metadata.get("duration"),"sourcePageUrl":source.url(),"context":{"cookies":[]},"items":[{"itemId":format!("{}-{}",source.media_id(),&digest[..20]),"identity":"video","kind":"video","ordinal":1,"urls":first["urls"],"previewUrls":preview,"width":first["width"],"height":first["height"],"candidates":rows}]}),
    )
}

pub struct YtDlpTikTokVideoAnalyzer {
    pub metadata: Arc<dyn TikTokMetadataExtractor>,
    pub probe: Arc<dyn crate::application::tiktok_video::TikTokCandidateProbe>,
}
impl TikTokVideoAnalyzer for YtDlpTikTokVideoAnalyzer {
    fn analyze_item(
        &self,
        inventory: &TikTokInventory,
        identity: &TikTokAttachmentIdentity,
        compatibility_mode: bool,
        context: &TikTokRequestContext,
    ) -> Result<ResolvedTikTokVideo, AnalysisErrorCode> {
        let item = inventory
            .items
            .iter()
            .find(|item| item.identity.as_ref() == Some(identity))
            .ok_or(AnalysisErrorCode::ContentChanged)?;
        let native = item
            .native
            .as_ref()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let mut augmented = inventory.clone();
        match self.metadata.extract(&inventory.source, context) {
            Ok(metadata) => {
                let merged =
                    merge_video_candidates(&native.data, &metadata, inventory.source.media_id())?;
                let selected = augmented
                    .items
                    .iter_mut()
                    .find(|item| item.identity.as_ref() == Some(identity))
                    .ok_or(AnalysisErrorCode::ContentChanged)?;
                selected.native = Some(Arc::new(TikTokNativeAsset {
                    data: merged,
                    context: native.context.clone(),
                }));
            }
            Err(_error) => {
                // An inaccessible alternate extractor cannot invalidate a complete direct result.
                if context.cancellation.is_canceled() {
                    return Err(AnalysisErrorCode::AnalysisUnavailable);
                }
            }
        }
        resolve_proven_tiktok_video(
            &augmented,
            identity,
            compatibility_mode,
            context,
            self.probe.as_ref(),
        )
    }
}

pub fn resolve_proven_tiktok_video(
    inventory: &TikTokInventory,
    identity: &TikTokAttachmentIdentity,
    compatibility: bool,
    context: &TikTokRequestContext,
    probe: &dyn crate::application::tiktok_video::TikTokCandidateProbe,
) -> Result<ResolvedTikTokVideo, AnalysisErrorCode> {
    let mut inventory = inventory.clone();
    let mut probed = std::collections::HashSet::new();
    loop {
        let item = inventory
            .items
            .iter()
            .find(|i| i.identity.as_ref() == Some(identity))
            .ok_or(AnalysisErrorCode::ContentChanged)?;
        let native = item
            .native
            .as_ref()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?
            .clone();
        let rows = native
            .data
            .get("candidates")
            .and_then(Value::as_array)
            .filter(|rows| !rows.is_empty() && rows.len() <= 512)
            .ok_or(AnalysisErrorCode::FormatUnavailable)?;
        let essential = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.get("audioCodec").and_then(Value::as_str).is_none()
                    && r.get("audioAbsent").and_then(Value::as_bool) != Some(true)
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let needed = if essential.is_empty() {
            let resolved = resolve_tiktok_video(&inventory, identity, compatibility, context)?;
            let needed =
                crate::domain::format_selection::unproven_quality_format_ids(resolved.media());
            if needed.is_empty() {
                return Ok(resolved);
            }
            resolved
                .media()
                .formats
                .iter()
                .enumerate()
                .filter(|(_, f)| needed.contains(&f.id))
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        } else {
            essential
        };
        let needed = needed
            .into_iter()
            .filter(|i| !probed.contains(i))
            .collect::<Vec<_>>();
        if needed.is_empty() || probed.len() + needed.len() > 16 {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        let mut data = native.data.clone();
        for index in needed {
            let facts = probe.probe(&native, &rows[index], context)?;
            let facts = facts
                .as_object()
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            let output = data["candidates"][index]
                .as_object_mut()
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            for (key, value) in facts {
                if !value.is_null() {
                    output.insert(key.clone(), value.clone());
                }
            }
            if output.get("audioAbsent").and_then(Value::as_bool) == Some(true) {
                output.remove("audioBitrateKbps");
                output.remove("audioCodec");
                output.remove("audioChannels");
                output.remove("audioSampleRateHz");
            }
            probed.insert(index);
        }
        inventory
            .items
            .iter_mut()
            .find(|i| i.identity.as_ref() == Some(identity))
            .ok_or(AnalysisErrorCode::ContentChanged)?
            .native = Some(Arc::new(TikTokNativeAsset {
            data,
            context: native.context.clone(),
        }));
    }
}
