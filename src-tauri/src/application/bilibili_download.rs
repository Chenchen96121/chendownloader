//! Immutable queued selection. None of these types can be supplied through IPC.
use super::{
    bilibili_source::{BilibiliAnalyzedSource, BilibiliPageIdentity, BilibiliStreamSource},
    media_url::ValidatedMediaUrl,
    network_profile::NetworkProfile,
    process::CancellationToken,
};
use crate::domain::{
    bilibili::{validate_bilibili_url, BilibiliVideoRequest},
    download::DownloadErrorCode,
    format_selection::SelectionPlan,
    media::{MediaFormat, MediaInfo},
};
use crate::infrastructure::{
    bilibili::url_policy::{validate_bilibili_asset_url, BilibiliAssetKind},
    ffprobe::stream_properties::{AudioStreamProperties, VideoStreamProperties},
};
use std::{collections::HashSet, sync::Arc};

#[derive(Clone)]
pub struct BilibiliDownloadSnapshot {
    analysis_id: String,
    source: ValidatedMediaUrl,
    request: BilibiliVideoRequest,
    identity: BilibiliPageIdentity,
    media: MediaInfo,
    plan: SelectionPlan,
    streams: Vec<BilibiliStreamSource>,
    network: Arc<NetworkProfile>,
}
impl BilibiliDownloadSnapshot {
    pub(crate) fn capture(
        analysis_id: String,
        analyzed: &BilibiliAnalyzedSource,
        plan: SelectionPlan,
        network: Arc<NetworkProfile>,
    ) -> Result<Self, DownloadErrorCode> {
        let mut selected = Vec::new();
        let mut ids = HashSet::new();
        for id in &plan.source_format_ids {
            if !ids.insert(id) {
                return Err(DownloadErrorCode::InvalidRequest);
            }
            selected.push(
                analyzed
                    .stream(id)
                    .ok_or(DownloadErrorCode::FormatUnavailable)?
                    .clone(),
            );
        }
        if selected.is_empty() || selected.len() > 2 {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let mut media = analyzed.media().clone();
        media.formats = selected.iter().map(|s| s.format().clone()).collect();
        Ok(Self {
            analysis_id,
            source: analyzed.evidence().source().clone(),
            request: validate_bilibili_url(analyzed.evidence().source().url())
                .map_err(|_| DownloadErrorCode::InvalidRequest)?,
            identity: analyzed.evidence().identity().clone(),
            media,
            plan,
            streams: selected,
            network,
        })
    }
    pub fn source(&self) -> &ValidatedMediaUrl {
        &self.source
    }
    pub fn media(&self) -> &MediaInfo {
        &self.media
    }
    pub fn plan(&self) -> &SelectionPlan {
        &self.plan
    }
    pub fn duration_seconds(&self) -> f64 {
        self.identity.duration_seconds
    }
    pub(crate) fn request(&self) -> &BilibiliVideoRequest {
        &self.request
    }
    pub(crate) fn identity(&self) -> &BilibiliPageIdentity {
        &self.identity
    }
    pub(crate) fn network(&self) -> Arc<NetworkProfile> {
        self.network.clone()
    }
    pub(crate) fn expected_declarations(&self) -> Vec<MediaFormat> {
        self.streams
            .iter()
            .map(|stream| {
                let mut format = stream.format().clone();
                format.fps = stream.declared_fps;
                format
            })
            .collect()
    }
    pub(crate) fn bind(
        &self,
        current: BilibiliAnalyzedSource,
    ) -> Result<VerifiedBilibiliDownloadSource, DownloadErrorCode> {
        let changed = DownloadErrorCode::ContentChanged;
        if current.evidence().identity() != &self.identity
            || current.media().duration_seconds != self.media.duration_seconds
        {
            return Err(changed);
        }
        let mut streams = Vec::new();
        for previous in &self.streams {
            let fresh = current.stream(&previous.format().id).ok_or(changed)?;
            if !known_format_matches(previous.format(), fresh.format())
                || previous
                    .video()
                    .is_some_and(|old| fresh.video().is_none_or(|new| !new.unchanged_from(old)))
                || previous
                    .audio()
                    .is_some_and(|old| fresh.audio().is_none_or(|new| !new.unchanged_from(old)))
            {
                return Err(changed);
            }
            // Original UI facts/plan remain frozen. Newly acquired source facts
            // are proof, never a reason to re-rank or rewrite that plan.
            streams.push(BilibiliStreamSource {
                declared_fps: previous.declared_fps,
                format: previous.format().clone(),
                url: fresh.url().clone(),
                video: fresh.video().cloned(),
                audio: fresh.audio().cloned(),
            });
        }
        Ok(VerifiedBilibiliDownloadSource {
            analysis_id: self.analysis_id.clone(),
            source: self.source.clone(),
            identity: self.identity.clone(),
            plan: self.plan.clone(),
            streams,
            network: self.network.clone(),
        })
    }
}
fn known_format_matches(old: &MediaFormat, new: &MediaFormat) -> bool {
    macro_rules! known {($($field:ident),*)=>{$(old.$field.as_ref().is_none_or(|v|new.$field.as_ref()==Some(v)))&&*};}
    old.id == new.id
        && old.container == new.container
        && old.kind == new.kind
        && known!(
            width,
            height,
            fps,
            video_bitrate_kbps,
            audio_bitrate_kbps,
            audio_sample_rate_hz,
            audio_channels,
            video_codec,
            audio_codec,
            dynamic_range,
            file_size
        )
}
pub trait BilibiliDownloadRefresher: Send + Sync {
    fn refresh(
        &self,
        snapshot: &BilibiliDownloadSnapshot,
        cancellation: CancellationToken,
    ) -> Result<VerifiedBilibiliDownloadSource, DownloadErrorCode>;
}
#[derive(Clone)]
pub struct VerifiedBilibiliDownloadSource {
    analysis_id: String,
    source: ValidatedMediaUrl,
    identity: BilibiliPageIdentity,
    plan: SelectionPlan,
    streams: Vec<BilibiliStreamSource>,
    network: Arc<NetworkProfile>,
}
impl VerifiedBilibiliDownloadSource {
    pub fn validate_request(
        &self,
        plan: &SelectionPlan,
        video: Option<&MediaFormat>,
        audio: Option<&MediaFormat>,
    ) -> Result<(), DownloadErrorCode> {
        let expected = |id: &Option<String>| {
            id.as_ref().and_then(|id| {
                self.streams
                    .iter()
                    .find(|s| &s.format().id == id)
                    .map(|s| s.format())
            })
        };
        if self.analysis_id.is_empty()
            || plan != &self.plan
            || video != expected(&plan.video_format_id)
            || audio != expected(&plan.audio_format_id)
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        for id in &plan.source_format_ids {
            self.format_url(id)?;
        }
        Ok(())
    }
    fn selected(&self, id: &str) -> Result<&BilibiliStreamSource, DownloadErrorCode> {
        self.streams
            .iter()
            .find(|s| s.format().id == id)
            .ok_or(DownloadErrorCode::InvalidRequest)
    }
    pub fn network(&self) -> Arc<NetworkProfile> {
        self.network.clone()
    }
    pub fn referer(&self) -> &str {
        self.source.url()
    }
    pub fn duration_seconds(&self) -> f64 {
        self.identity.duration_seconds
    }
    pub fn format_url(&self, id: &str) -> Result<&url::Url, DownloadErrorCode> {
        let url = self.selected(id)?.url();
        validate_bilibili_asset_url(url.as_str(), BilibiliAssetKind::Media)
            .map_err(|_| DownloadErrorCode::InvalidRequest)?;
        Ok(url)
    }
    pub fn original_properties(
        &self,
        id: &str,
    ) -> Result<
        (
            Option<&VideoStreamProperties>,
            Option<&AudioStreamProperties>,
        ),
        DownloadErrorCode,
    > {
        let stream = self.selected(id)?;
        Ok((stream.video(), stream.audio()))
    }
    pub fn format_info_json(&self, id: &str) -> Result<Vec<u8>, DownloadErrorCode> {
        let format = self.selected(id)?.format();
        let url = self.format_url(id)?;
        serde_json::to_vec(&serde_json::json!({"id":format!("{}_p{}",self.identity.bvid,self.identity.part),"title":"Bilibili media","formats":[{"format_id":format.id,"url":url.as_str(),"ext":format.container,"protocol":"https","vcodec":format.video_codec.as_deref().unwrap_or("none"),"acodec":format.audio_codec.as_deref().unwrap_or("none"),"width":format.width,"height":format.height,"fps":format.fps,"vbr":format.video_bitrate_kbps,"abr":format.audio_bitrate_kbps,"dynamic_range":format.dynamic_range}]})).map_err(|_|DownloadErrorCode::OutputInvalid)
    }
}
