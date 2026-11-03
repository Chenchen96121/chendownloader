use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use crate::{
    application::{
        analyze_media::{AnalyzeMediaService, MetadataAnalyzer},
        download_job::ResolvedDownloadInput,
        media_url::{validate_media_url, ValidatedMediaUrl},
        preview_selection::{preview_selection, SelectionOutcome},
    },
    domain::{
        download::DownloadErrorCode,
        format_selection::{
            select_formats, SelectionIntent, SelectionMode, SelectionUnavailableCode,
        },
        media::{AnalysisErrorCode, MediaInfo},
        output_policy::VideoContainer,
    },
};

const SESSION_LIFETIME: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAnalysisPreview {
    pub analysis_id: String,
    pub media: MediaInfo,
    pub ranked_video_format_ids: Vec<String>,
    pub ranked_audio_format_ids: Vec<String>,
    pub outcome: SelectionOutcome,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoResolutionPlan {
    pub video_format_id: String,
    pub output_container: VideoContainer,
    pub outcome: SelectionOutcome,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionOptions {
    pub compatibility_mode: bool,
    pub analysis_id: String,
    pub mode: SelectionMode,
    pub options: Vec<VideoResolutionPlan>,
}

struct AnalysisSession {
    analysis_id: String,
    canonical_source: ValidatedMediaUrl,
    media: MediaInfo,
    created_at: Instant,
}

pub struct LiveAnalysisService<A> {
    analyzer: A,
    next_id: AtomicU64,
    session: Mutex<Option<AnalysisSession>>,
}

impl<A: MetadataAnalyzer> LiveAnalysisService<A> {
    pub fn new(analyzer: A) -> Self {
        Self {
            analyzer,
            next_id: AtomicU64::new(1),
            session: Mutex::new(None),
        }
    }

    pub fn analyze(
        &self,
        url: &str,
        intent: SelectionIntent,
        now: Instant,
    ) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
        let source = validate_media_url(url)?;
        let media = AnalyzeMediaService::new(&self.analyzer).analyze_validated(&source)?;
        let selection = preview_selection(media.clone(), intent)?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?
            .to_string();
        *session = Some(AnalysisSession {
            analysis_id: id.clone(),
            canonical_source: source,
            media,
            created_at: now,
        });
        Ok(project(id, selection))
    }

    pub fn preview(
        &self,
        analysis_id: &str,
        intent: SelectionIntent,
        now: Instant,
    ) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
        if !valid_analysis_id(analysis_id) {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        let mut session = self
            .session
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let current = session
            .as_ref()
            .filter(|current| current.analysis_id == analysis_id)
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
        if now.saturating_duration_since(current.created_at) >= SESSION_LIFETIME {
            *session = None;
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        let media = current.media.clone();
        let id = current.analysis_id.clone();
        drop(session);
        Ok(project(id, preview_selection(media, intent)?))
    }

    pub fn resolve_download(
        &self,
        analysis_id: &str,
        intent: SelectionIntent,
        now: Instant,
    ) -> Result<ResolvedDownloadInput, DownloadErrorCode> {
        if !valid_analysis_id(analysis_id) {
            return Err(DownloadErrorCode::AnalysisUnavailable);
        }
        let mut session = self
            .session
            .lock()
            .map_err(|_| DownloadErrorCode::AnalysisUnavailable)?;
        let current = session
            .as_ref()
            .filter(|current| current.analysis_id == analysis_id)
            .ok_or(DownloadErrorCode::AnalysisUnavailable)?;
        if now.saturating_duration_since(current.created_at) >= SESSION_LIFETIME {
            *session = None;
            return Err(DownloadErrorCode::AnalysisUnavailable);
        }
        let source = current.canonical_source.clone();
        let media = current.media.clone();
        drop(session);
        let plan = select_formats(&media, &intent).map_err(|error| match error {
            SelectionUnavailableCode::FormatUnavailable
            | SelectionUnavailableCode::CompatibilityUnavailable => {
                DownloadErrorCode::FormatUnavailable
            }
            SelectionUnavailableCode::InvalidSelection
            | SelectionUnavailableCode::InvalidClip
            | SelectionUnavailableCode::ClipDurationUnknown => DownloadErrorCode::InvalidRequest,
        })?;
        Ok(ResolvedDownloadInput {
            bilibili: None,
            source,
            media,
            plan,
        })
    }

    pub fn resolve_media_page(
        &self,
        analysis_id: &str,
        now: Instant,
    ) -> Result<ValidatedMediaUrl, AnalysisErrorCode> {
        if !valid_analysis_id(analysis_id) {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        let mut session = self
            .session
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let current = session
            .as_ref()
            .filter(|current| current.analysis_id == analysis_id)
            .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
        if now.saturating_duration_since(current.created_at) >= SESSION_LIFETIME {
            *session = None;
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        Ok(current.canonical_source.clone())
    }

    pub fn resolution_options(
        &self,
        analysis_id: &str,
        mode: SelectionMode,
        compatibility_mode: bool,
        now: Instant,
    ) -> Result<ResolutionOptions, AnalysisErrorCode> {
        if !matches!(mode, SelectionMode::Manual | SelectionMode::VideoOnly) {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        let preview = self.preview(
            analysis_id,
            SelectionIntent {
                compatibility_mode,
                mode,
                video_format_id: None,
                output_container: None,
                clip: None,
            },
            now,
        )?;
        let options = preview
            .ranked_video_format_ids
            .into_iter()
            .flat_map(|id| {
                let containers: &[VideoContainer] = if compatibility_mode {
                    &[VideoContainer::Mp4]
                } else {
                    &[VideoContainer::Mp4, VideoContainer::Webm]
                };
                containers
                    .iter()
                    .copied()
                    .map(move |container| (id.clone(), container))
            })
            .map(|(id, container)| {
                let intent = SelectionIntent {
                    compatibility_mode,
                    mode,
                    video_format_id: Some(id.clone()),
                    output_container: Some(container),
                    clip: None,
                };
                let outcome = match select_formats(&preview.media, &intent) {
                    Ok(plan) => SelectionOutcome::Selected { plan },
                    Err(code) => SelectionOutcome::Unavailable { code },
                };
                VideoResolutionPlan {
                    video_format_id: id,
                    output_container: container,
                    outcome,
                }
            })
            .collect();
        Ok(ResolutionOptions {
            compatibility_mode,
            analysis_id: preview.analysis_id,
            mode,
            options,
        })
    }
}

fn project(
    analysis_id: String,
    selection: crate::application::preview_selection::SelectionPreview,
) -> LiveAnalysisPreview {
    LiveAnalysisPreview {
        analysis_id,
        media: selection.media,
        ranked_video_format_ids: selection.ranked_video_format_ids,
        ranked_audio_format_ids: selection.ranked_audio_format_ids,
        outcome: selection.outcome,
    }
}

fn valid_analysis_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|parsed| parsed > 0)
}
