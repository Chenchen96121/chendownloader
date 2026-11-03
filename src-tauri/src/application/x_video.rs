use crate::{
    application::{
        network_profile::NetworkProfile,
        x_source::{XAttachmentIdentity, XInventory, XRequestContext, XSourceItem},
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::{select_formats, SelectionIntent, SelectionMode, SelectionPlan},
        media::{AnalysisErrorCode, MediaFormat, MediaInfo, Platform, StreamKind},
    },
    infrastructure::{
        x::{
            manifest::VerifiedHlsSnapshot,
            url_policy::{validate_x_asset_url, XAssetKind},
        },
        ytdlp::normalize_metadata,
    },
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub struct VerifiedXVideoMetadata {
    pub(crate) identity: XAttachmentIdentity,
    pub(crate) raw: Value,
    pub(crate) hls: HashMap<String, Arc<VerifiedHlsSnapshot>>,
}
#[derive(Clone)]
pub struct VerifiedXDownloadSource {
    identity: XAttachmentIdentity,
    formats: Vec<(MediaFormat, url::Url, String)>,
    network: Arc<NetworkProfile>,
    hls: HashMap<String, Arc<VerifiedHlsSnapshot>>,
    plan: SelectionPlan,
}
impl VerifiedXDownloadSource {
    pub(crate) fn validate_request(
        &self,
        plan: &SelectionPlan,
        video: Option<&MediaFormat>,
        audio: Option<&MediaFormat>,
    ) -> Result<(), DownloadErrorCode> {
        if plan != &self.plan {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let selected = |id: &Option<String>| {
            id.as_ref().and_then(|id| {
                self.formats
                    .iter()
                    .find(|(f, _, _)| &f.id == id)
                    .map(|(f, _, _)| f)
            })
        };
        if video != selected(&plan.video_format_id) || audio != selected(&plan.audio_format_id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        self.validate_transfer(plan)
    }
    pub fn hls_input(
        &self,
        format_id: &str,
    ) -> Result<Option<Arc<VerifiedHlsSnapshot>>, DownloadErrorCode> {
        let (_, _, protocol) = self
            .formats
            .iter()
            .find(|(f, _, _)| f.id == format_id)
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        if protocol == "https" {
            return Ok(None);
        }
        if !matches!(protocol.as_str(), "m3u8" | "m3u8_native") {
            return Err(DownloadErrorCode::FormatUnavailable);
        }
        let snapshot = self
            .hls
            .get(format_id)
            .ok_or(DownloadErrorCode::FormatUnavailable)?;
        if !snapshot.is_media_playlist() {
            return Err(DownloadErrorCode::FormatUnavailable);
        }
        snapshot
            .revalidate()
            .map_err(|_| DownloadErrorCode::OutputInvalid)?;
        Ok(Some(Arc::clone(snapshot)))
    }
    fn validate_transfer(&self, plan: &SelectionPlan) -> Result<(), DownloadErrorCode> {
        let mut seen = HashSet::new();
        if plan.clip.is_some() || plan.source_format_ids.is_empty() {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        for id in &plan.source_format_ids {
            if !seen.insert(id) {
                return Err(DownloadErrorCode::InvalidRequest);
            }
            let (_, url, _) = self
                .formats
                .iter()
                .find(|(f, _, _)| &f.id == id)
                .ok_or(DownloadErrorCode::InvalidRequest)?;
            validate_x_asset_url(url.as_str(), XAssetKind::Video)
                .map_err(|_| DownloadErrorCode::InvalidRequest)?;
            self.hls_input(id)?;
        }
        Ok(())
    }
    pub fn identity(&self) -> &XAttachmentIdentity {
        &self.identity
    }
    pub fn network(&self) -> Arc<NetworkProfile> {
        Arc::clone(&self.network)
    }
    pub fn info_json(&self, plan: &SelectionPlan) -> Result<Vec<u8>, DownloadErrorCode> {
        self.validate_transfer(plan)?;
        self.serialize_formats(&plan.source_format_ids)
    }
    pub(crate) fn format(&self, id: &str) -> Result<&MediaFormat, DownloadErrorCode> {
        self.formats
            .iter()
            .find(|(f, _, _)| f.id == id)
            .map(|(f, _, _)| f)
            .ok_or(DownloadErrorCode::InvalidRequest)
    }
    pub(crate) fn format_info_json(
        &self,
        plan: &SelectionPlan,
        id: &str,
    ) -> Result<Vec<u8>, DownloadErrorCode> {
        self.validate_transfer(plan)?;
        if !plan.source_format_ids.iter().any(|selected| selected == id) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        self.serialize_formats(&[id.to_owned()])
    }
    fn serialize_formats(&self, ids: &[String]) -> Result<Vec<u8>, DownloadErrorCode> {
        let mut formats = Vec::new();
        for id in ids {
            let (f, url, protocol) = self
                .formats
                .iter()
                .find(|(f, _, _)| &f.id == id)
                .ok_or(DownloadErrorCode::InvalidRequest)?;
            // A manifest must first have an immutable, validated transfer plan.
            if protocol != "https" {
                return Err(DownloadErrorCode::FormatUnavailable);
            }
            validate_x_asset_url(url.as_str(), XAssetKind::Video)
                .map_err(|_| DownloadErrorCode::InvalidRequest)?;
            formats.push(json!({"format_id":f.id,"url":url.as_str(),"ext":f.container,"protocol":"https","vcodec":f.video_codec.as_deref().unwrap_or("none"),"acodec":f.audio_codec.as_deref().unwrap_or("none"),"width":f.width,"height":f.height,"fps":f.fps,"vbr":f.video_bitrate_kbps,"abr":f.audio_bitrate_kbps,"dynamic_range":f.dynamic_range}));
        }
        serde_json::to_vec(
            &json!({"id":self.identity.media_id,"title":"X media","formats":formats}),
        )
        .map_err(|_| DownloadErrorCode::OutputInvalid)
    }
}
pub struct ResolvedXVideo {
    media: MediaInfo,
    plan: SelectionPlan,
    source: VerifiedXDownloadSource,
}
impl ResolvedXVideo {
    pub fn media(&self) -> &MediaInfo {
        &self.media
    }
    pub fn plan(&self) -> &SelectionPlan {
        &self.plan
    }
    pub fn source(&self) -> &VerifiedXDownloadSource {
        &self.source
    }
    pub fn into_parts(self) -> (MediaInfo, SelectionPlan, VerifiedXDownloadSource) {
        (self.media, self.plan, self.source)
    }
}
pub trait XVideoAnalyzer: Send + Sync {
    fn analyze_item(
        &self,
        inventory: &XInventory,
        identity: &XAttachmentIdentity,
        compatibility_mode: bool,
        context: &XRequestContext,
    ) -> Result<ResolvedXVideo, AnalysisErrorCode>;
}

pub fn match_x_video(
    inventory: &XInventory,
    item: &XSourceItem,
    metadata: &Value,
) -> Result<VerifiedXVideoMetadata, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let changed = AnalysisErrorCode::ContentChanged;
    let identity = item.identity.as_ref().ok_or(changed)?;
    if inventory.source.platform() != Platform::X
        || identity.post_id != inventory.source.media_id()
        || inventory
            .items
            .iter()
            .filter(|i| i.identity.as_ref() == Some(identity))
            .count()
            != 1
    {
        return Err(changed);
    }
    let entries = if let Some(entries) = metadata.get("entries") {
        if metadata.get("id").and_then(Value::as_str) != Some(identity.post_id.as_str()) {
            return Err(changed);
        }
        entries
            .as_array()
            .filter(|a| a.len() <= 32)
            .ok_or(invalid)?
            .iter()
            .collect::<Vec<_>>()
    } else {
        vec![metadata]
    };
    let mut matched = None;
    for entry in entries {
        let Some(formats) = entry.get("formats").and_then(Value::as_array) else {
            continue;
        };
        if formats.is_empty() || formats.len() > 512 {
            return Err(invalid);
        }
        let mut matches = HashSet::new();
        for candidate in &inventory.items {
            let Some(id) = candidate.identity.as_ref() else {
                continue;
            };
            let id_match = entry.get("id").and_then(Value::as_str) == Some(id.media_id.as_str());
            let url_match = formats.iter().any(|f| {
                candidate
                    .video_candidates
                    .iter()
                    .any(|c| f.get("url").and_then(Value::as_str) == Some(c.url.as_str()))
            });
            if id_match || url_match {
                matches.insert(id.media_id.as_str());
            }
        }
        if !matches.contains(identity.media_id.as_str()) {
            continue;
        }
        if matches.len() != 1
            || matched.is_some()
            || entry.get("display_id").and_then(Value::as_str) != Some(identity.post_id.as_str())
        {
            return Err(changed);
        }
        for format in formats {
            let raw = format.get("url").and_then(Value::as_str).ok_or(invalid)?;
            validate_x_asset_url(raw, XAssetKind::Video)?;
            let manifest = format.get("manifest_url").and_then(Value::as_str);
            if !item
                .video_candidates
                .iter()
                .any(|c| c.url.as_str() == raw || manifest == Some(c.url.as_str()))
            {
                return Err(changed);
            }
            if format.get("has_drm").and_then(Value::as_bool) == Some(true) {
                return Err(AnalysisErrorCode::UnsupportedMedia);
            }
        }
        if item.video_candidates.iter().any(|candidate| {
            !formats.iter().any(|f| {
                f.get("url").and_then(Value::as_str) == Some(candidate.url.as_str())
                    || f.get("manifest_url").and_then(Value::as_str) == Some(candidate.url.as_str())
            })
        }) {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        matched = Some(entry.clone());
    }
    let mut raw = matched.ok_or(changed)?;
    let obj = raw.as_object_mut().ok_or(invalid)?;
    obj.remove("entries");
    obj.remove("_type");
    obj.insert("id".into(), identity.media_id.clone().into());
    obj.insert("title".into(), inventory.title.clone().into());
    if let Some(duration) = item.duration_seconds {
        obj.insert("duration".into(), duration.into());
    }
    Ok(VerifiedXVideoMetadata {
        identity: identity.clone(),
        raw,
        hls: HashMap::new(),
    })
}

pub fn resolve_x_video(
    verified: VerifiedXVideoMetadata,
    network: Arc<NetworkProfile>,
    compatibility: bool,
) -> Result<ResolvedXVideo, AnalysisErrorCode> {
    let unavailable = AnalysisErrorCode::FormatUnavailable;
    let rows = verified
        .raw
        .get("formats")
        .and_then(Value::as_array)
        .ok_or(unavailable)?;
    // Do not let the generic normalizer discard an unknown highest candidate.
    for row in rows {
        for key in ["vcodec", "acodec"] {
            if !row
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|s| !matches!(s, "" | "unknown"))
            {
                return Err(unavailable);
            }
        }
        if row.get("vcodec").and_then(Value::as_str) != Some("none")
            && ["width", "height", "fps"].iter().any(|key| {
                !row.get(*key)
                    .and_then(Value::as_f64)
                    .is_some_and(|v| v > 0.0)
            })
        {
            return Err(unavailable);
        }
    }
    let media = normalize_metadata(
        Platform::X,
        &serde_json::to_string(&verified.raw).map_err(|_| unavailable)?,
    )?;
    if media.formats.len() != rows.len() {
        return Err(unavailable);
    }
    if !crate::domain::format_selection::unproven_quality_format_ids(&media).is_empty() {
        return Err(unavailable);
    }
    let mode = if media
        .formats
        .iter()
        .all(|f| f.kind == StreamKind::VideoOnly)
    {
        SelectionMode::VideoOnly
    } else {
        SelectionMode::BestAvailable
    };
    let plan = select_formats(
        &media,
        &SelectionIntent {
            compatibility_mode: compatibility,
            mode,
            video_format_id: None,
            output_container: None,
            clip: None,
        },
    )
    .map_err(|_| unavailable)?;
    let mut formats = Vec::new();
    for format in &media.formats {
        let row = rows
            .iter()
            .find(|row| row.get("format_id").and_then(Value::as_str) == Some(format.id.as_str()))
            .ok_or(unavailable)?;
        let url = validate_x_asset_url(
            row.get("url").and_then(Value::as_str).ok_or(unavailable)?,
            XAssetKind::Video,
        )?;
        let protocol = row
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or("https")
            .to_owned();
        formats.push((format.clone(), url, protocol));
    }
    let source = VerifiedXDownloadSource {
        identity: verified.identity,
        formats,
        network,
        hls: verified.hls,
        plan: plan.clone(),
    };
    // A ready entry must already have a safe transfer path for its best plan.
    source.validate_transfer(&plan).map_err(|_| unavailable)?;
    Ok(ResolvedXVideo {
        media,
        plan,
        source,
    })
}
