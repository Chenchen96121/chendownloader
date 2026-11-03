use super::{
    bilibili_download::{
        BilibiliDownloadRefresher, BilibiliDownloadSnapshot, VerifiedBilibiliDownloadSource,
    },
    bilibili_source::{
        BilibiliAnalysisScope, BilibiliAnalyzedSource, BilibiliAnalyzer, BilibiliPageEvidence,
        BilibiliRequestContext,
    },
    live_analysis::{ResolutionOptions, VideoResolutionPlan},
    network_profile::NetworkProfile,
    preview_selection::{preview_selection, SelectionOutcome},
    process::CancellationToken,
};
use crate::domain::{
    bilibili::{validate_bilibili_url, BilibiliAnalysisPreview, BilibiliCoverPayload},
    download::{valid_opaque_id, DownloadErrorCode},
    format_selection::{select_formats, SelectionIntent, SelectionMode},
    media::AnalysisErrorCode,
    output_policy::VideoContainer,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub trait BilibiliCoverLoader: Send + Sync {
    fn load(
        &self,
        evidence: &BilibiliPageEvidence,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliCoverPayload, AnalysisErrorCode>;
}
impl<T: BilibiliCoverLoader + ?Sized> BilibiliCoverLoader for Arc<T> {
    fn load(
        &self,
        evidence: &BilibiliPageEvidence,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        (**self).load(evidence, context)
    }
}
struct Session {
    id: String,
    source: Arc<BilibiliAnalyzedSource>,
    network: Arc<NetworkProfile>,
    created_at: Instant,
    cancellation: CancellationToken,
    cover: Option<BilibiliCoverPayload>,
    cover_loading: bool,
}
#[derive(Default)]
struct State {
    generation: u64,
    closed: bool,
    active: Option<CancellationToken>,
    session: Option<Session>,
}
pub struct BilibiliAnalysisService<A, C> {
    analyzer: A,
    covers: C,
    state: Mutex<State>,
}
impl<A: BilibiliAnalyzer, C: BilibiliCoverLoader> BilibiliAnalysisService<A, C> {
    pub fn new(analyzer: A, covers: C) -> Self {
        Self {
            analyzer,
            covers,
            state: Mutex::new(State::default()),
        }
    }
    pub fn analyze(
        &self,
        url: &str,
        intent: SelectionIntent,
        now: Instant,
    ) -> Result<BilibiliAnalysisPreview, AnalysisErrorCode> {
        let cancellation = CancellationToken::new();
        let generation = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
            if state.closed {
                return Err(AnalysisErrorCode::AnalysisUnavailable);
            }
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
            if let Some(old) = state.active.take() {
                old.cancel();
            }
            if let Some(old) = state.session.take() {
                old.cancellation.cancel();
            }
            state.active = Some(cancellation.clone());
            state.generation
        };
        let request = validate_bilibili_url(url)?;
        let network = Arc::new(NetworkProfile::current());
        let started = Instant::now();
        let context = BilibiliRequestContext::new(
            cancellation.clone(),
            started + Duration::from_secs(120),
            network.clone(),
        );
        let result = self
            .analyzer
            .analyze(&request, BilibiliAnalysisScope::Discover, &context);
        let mut state = self
            .state
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        if state.closed || state.generation != generation || cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        state.active = None;
        let source = Arc::new(result?);
        let id = generation.to_string();
        let preview = project(
            id.clone(),
            source.evidence().identity().part,
            source.media().clone(),
            intent,
        )?;
        state.session = Some(Session {
            id,
            source,
            network,
            created_at: now + started.elapsed(),
            cancellation,
            cover: None,
            cover_loading: false,
        });
        Ok(preview)
    }
    pub fn preview(
        &self,
        id: &str,
        intent: SelectionIntent,
        now: Instant,
    ) -> Result<BilibiliAnalysisPreview, AnalysisErrorCode> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let session = current(&mut state, id, now)?;
        project(
            session.id.clone(),
            session.source.evidence().identity().part,
            session.source.media().clone(),
            intent,
        )
    }
    pub fn resolution_options(
        &self,
        id: &str,
        mode: SelectionMode,
        compatibility: bool,
        now: Instant,
    ) -> Result<ResolutionOptions, AnalysisErrorCode> {
        if !matches!(mode, SelectionMode::Manual | SelectionMode::VideoOnly) {
            return Err(AnalysisErrorCode::AnalysisUnavailable);
        }
        let preview = self.preview(
            id,
            SelectionIntent {
                mode,
                compatibility_mode: compatibility,
                video_format_id: None,
                output_container: None,
                clip: None,
            },
            now,
        )?;
        let containers: &[VideoContainer] = if compatibility {
            &[VideoContainer::Mp4]
        } else {
            &[VideoContainer::Mp4, VideoContainer::Webm]
        };
        let mut options = Vec::new();
        for id in preview.ranked_video_format_ids {
            for container in containers {
                let intent = SelectionIntent {
                    mode,
                    compatibility_mode: compatibility,
                    video_format_id: Some(id.clone()),
                    output_container: Some(*container),
                    clip: None,
                };
                let outcome = match select_formats(&preview.media, &intent) {
                    Ok(plan) => SelectionOutcome::Selected { plan },
                    Err(code) => SelectionOutcome::Unavailable { code },
                };
                options.push(VideoResolutionPlan {
                    video_format_id: id.clone(),
                    output_container: *container,
                    outcome,
                });
            }
        }
        Ok(ResolutionOptions {
            analysis_id: preview.analysis_id,
            mode,
            compatibility_mode: compatibility,
            options,
        })
    }
    pub fn resolve_source(
        &self,
        id: &str,
        now: Instant,
    ) -> Result<super::media_url::ValidatedMediaUrl, AnalysisErrorCode> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        Ok(current(&mut state, id, now)?
            .source
            .evidence()
            .source()
            .clone())
    }
    pub fn prepare_download(
        &self,
        id: &str,
        intent: SelectionIntent,
        now: Instant,
    ) -> Result<BilibiliDownloadSnapshot, DownloadErrorCode> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::AnalysisUnavailable)?;
        let session = current(&mut state, id, now).map_err(download_error)?;
        let plan = select_formats(session.source.media(), &intent)
            .map_err(|_| DownloadErrorCode::FormatUnavailable)?;
        BilibiliDownloadSnapshot::capture(id.into(), &session.source, plan, session.network.clone())
    }
    pub fn cover(&self, id: &str, now: Instant) -> Result<BilibiliCoverPayload, AnalysisErrorCode> {
        let (source, network, cancellation) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
            let session = current(&mut state, id, now)?;
            if let Some(cached) = &session.cover {
                return Ok(cached.clone());
            }
            if session.cover_loading {
                return Err(AnalysisErrorCode::AnalysisUnavailable);
            }
            session.cover_loading = true;
            (
                session.source.clone(),
                session.network.clone(),
                session.cancellation.clone(),
            )
        };
        let context = BilibiliRequestContext::new(
            cancellation,
            Instant::now() + Duration::from_secs(15),
            network,
        );
        let result = self
            .covers
            .load(source.evidence(), &context)
            .and_then(|cover| {
                context.remaining(Duration::from_secs(15))?;
                if cover.valid() {
                    Ok(cover)
                } else {
                    Err(AnalysisErrorCode::InvalidMetadata)
                }
            });
        let mut state = self
            .state
            .lock()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        let session = current(&mut state, id, now)?;
        session.cover_loading = false;
        let cover = result?;
        session.cover = Some(cover.clone());
        Ok(cover)
    }
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            if let Some(active) = state.active.take() {
                active.cancel();
            }
            if let Some(session) = state.session.take() {
                session.cancellation.cancel();
            }
        }
    }
}
impl<A: BilibiliAnalyzer, C: BilibiliCoverLoader> BilibiliDownloadRefresher
    for BilibiliAnalysisService<A, C>
{
    fn refresh(
        &self,
        snapshot: &BilibiliDownloadSnapshot,
        cancellation: CancellationToken,
    ) -> Result<VerifiedBilibiliDownloadSource, DownloadErrorCode> {
        if self
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::AnalysisUnavailable)?
            .closed
        {
            return Err(DownloadErrorCode::AnalysisUnavailable);
        }
        let context = BilibiliRequestContext::new(
            cancellation,
            Instant::now() + Duration::from_secs(120),
            snapshot.network(),
        )
        .bind_refresh(
            snapshot.identity().clone(),
            snapshot.expected_declarations(),
        );
        let current = self
            .analyzer
            .analyze(
                snapshot.request(),
                BilibiliAnalysisScope::RefreshSelected(snapshot.plan().source_format_ids.clone()),
                &context,
            )
            .map_err(download_error)?;
        context
            .remaining(Duration::from_secs(120))
            .map_err(download_error)?;
        if self
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::AnalysisUnavailable)?
            .closed
        {
            return Err(DownloadErrorCode::AnalysisUnavailable);
        }
        snapshot.bind(current)
    }
}
fn current<'a>(
    state: &'a mut State,
    id: &str,
    now: Instant,
) -> Result<&'a mut Session, AnalysisErrorCode> {
    if state.closed || !valid_opaque_id(id) {
        return Err(AnalysisErrorCode::AnalysisUnavailable);
    }
    let session = state
        .session
        .as_ref()
        .filter(|s| s.id == id)
        .ok_or(AnalysisErrorCode::AnalysisUnavailable)?;
    if now.saturating_duration_since(session.created_at) >= Duration::from_secs(900) {
        if let Some(old) = state.session.take() {
            old.cancellation.cancel();
        }
        return Err(AnalysisErrorCode::AnalysisExpired);
    }
    state
        .session
        .as_mut()
        .ok_or(AnalysisErrorCode::AnalysisUnavailable)
}
fn project(
    id: String,
    part: u16,
    media: crate::domain::media::MediaInfo,
    intent: SelectionIntent,
) -> Result<BilibiliAnalysisPreview, AnalysisErrorCode> {
    let p = preview_selection(media, intent)?;
    Ok(BilibiliAnalysisPreview {
        analysis_id: id,
        part,
        media: p.media,
        ranked_video_format_ids: p.ranked_video_format_ids,
        ranked_audio_format_ids: p.ranked_audio_format_ids,
        outcome: p.outcome,
    })
}
fn download_error(error: AnalysisErrorCode) -> DownloadErrorCode {
    match error {
        AnalysisErrorCode::ContentChanged => DownloadErrorCode::ContentChanged,
        AnalysisErrorCode::AnalysisTimeout => DownloadErrorCode::DownloadTimeout,
        AnalysisErrorCode::NetworkFailed => DownloadErrorCode::NetworkFailed,
        AnalysisErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        AnalysisErrorCode::ToolIntegrityFailed => DownloadErrorCode::ToolIntegrityFailed,
        AnalysisErrorCode::AuthenticationRequired => DownloadErrorCode::AuthenticationRequired,
        AnalysisErrorCode::AccessDenied => DownloadErrorCode::AccessDenied,
        AnalysisErrorCode::ContentUnavailable => DownloadErrorCode::ContentUnavailable,
        AnalysisErrorCode::RateLimited => DownloadErrorCode::RateLimited,
        AnalysisErrorCode::FormatUnavailable | AnalysisErrorCode::UnsupportedMedia => {
            DownloadErrorCode::FormatUnavailable
        }
        AnalysisErrorCode::ProcessFailed => DownloadErrorCode::ProcessFailed,
        _ => DownloadErrorCode::AnalysisUnavailable,
    }
}
