use crate::domain::{
    format_selection::{
        select_formats, SelectionIntent, SelectionMode, SelectionPlan, SelectionUnavailableCode,
    },
    media::{AnalysisErrorCode, MediaInfo, Platform, StreamKind},
};

/// Native-only selector. It is intentionally not deserializable from IPC.
#[derive(Clone, Eq, PartialEq)]
pub enum InstagramVideoSelector {
    Reel,
    PostItem {
        ordinal: u16,
        expected_media_id: String,
    },
}

impl InstagramVideoSelector {
    pub const fn reel() -> Self {
        Self::Reel
    }

    pub fn post_item(
        ordinal: u16,
        expected_media_id: impl Into<String>,
    ) -> Result<Self, AnalysisErrorCode> {
        let expected_media_id = expected_media_id.into();
        if !(1..=32).contains(&ordinal) || !safe_media_id(&expected_media_id) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(Self::PostItem {
            ordinal,
            expected_media_id,
        })
    }

    pub const fn playlist_item(&self) -> Option<u16> {
        match self {
            Self::Reel => None,
            Self::PostItem { ordinal, .. } => Some(*ordinal),
        }
    }

    pub fn expected_media_id(&self) -> Option<&str> {
        match self {
            Self::Reel => None,
            Self::PostItem {
                expected_media_id, ..
            } => Some(expected_media_id),
        }
    }
}

/// Native-only analyzed video and its byte-preserving processing plan.
#[derive(Clone)]
pub struct ResolvedInstagramVideo {
    media: MediaInfo,
    selector: InstagramVideoSelector,
    plan: SelectionPlan,
}

impl ResolvedInstagramVideo {
    pub fn media(&self) -> &MediaInfo {
        &self.media
    }

    pub fn selector(&self) -> &InstagramVideoSelector {
        &self.selector
    }

    pub fn plan(&self) -> &SelectionPlan {
        &self.plan
    }

    pub fn into_parts(self) -> (MediaInfo, InstagramVideoSelector, SelectionPlan) {
        (self.media, self.selector, self.plan)
    }
}

pub fn resolve_instagram_video(
    media: MediaInfo,
    selector: InstagramVideoSelector,
    compatibility_mode: bool,
) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
    if media.platform != Platform::Instagram
        || !media
            .formats
            .iter()
            .any(|format| format.kind != StreamKind::AudioOnly)
    {
        return Err(AnalysisErrorCode::FormatUnavailable);
    }
    // A proven silent post is still a valid original video. Do not invent audio
    // or change the generic YouTube best-with-audio selection semantics.
    let mode = if media
        .formats
        .iter()
        .all(|format| format.kind == StreamKind::VideoOnly)
    {
        SelectionMode::VideoOnly
    } else {
        SelectionMode::BestAvailable
    };
    let plan = select_formats(
        &media,
        &SelectionIntent {
            compatibility_mode,
            mode,
            video_format_id: None,
            output_container: None,
            clip: None,
        },
    )
    .map_err(map_selection_error)?;
    if plan.mode != mode || plan.clip.is_some() {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(ResolvedInstagramVideo {
        media,
        selector,
        plan,
    })
}

fn safe_media_id(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    let mut parts = value.split('_');
    let primary = parts.next().unwrap_or_default();
    let owner = parts.next();
    parts.next().is_none()
        && !primary.is_empty()
        && primary.bytes().all(|byte| byte.is_ascii_digit())
        && owner
            .is_none_or(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn map_selection_error(error: SelectionUnavailableCode) -> AnalysisErrorCode {
    match error {
        SelectionUnavailableCode::FormatUnavailable
        | SelectionUnavailableCode::CompatibilityUnavailable => {
            AnalysisErrorCode::FormatUnavailable
        }
        SelectionUnavailableCode::InvalidSelection
        | SelectionUnavailableCode::InvalidClip
        | SelectionUnavailableCode::ClipDurationUnknown => AnalysisErrorCode::InvalidMetadata,
    }
}
