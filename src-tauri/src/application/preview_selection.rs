use crate::domain::{
    format_selection::{
        ranked_audio_format_ids, ranked_video_format_ids, select_formats, SelectionIntent,
        SelectionPlan, SelectionUnavailableCode,
    },
    media::{AnalysisErrorCode, MediaInfo},
};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionPreview {
    pub media: MediaInfo,
    pub ranked_video_format_ids: Vec<String>,
    pub ranked_audio_format_ids: Vec<String>,
    pub outcome: SelectionOutcome,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SelectionOutcome {
    Selected { plan: SelectionPlan },
    Unavailable { code: SelectionUnavailableCode },
}

pub fn preview_selection(
    media: MediaInfo,
    intent: SelectionIntent,
) -> Result<SelectionPreview, AnalysisErrorCode> {
    media.validate()?;
    let ranked_video_format_ids = ranked_video_format_ids(&media);
    let ranked_audio_format_ids = ranked_audio_format_ids(&media);
    let outcome = match select_formats(&media, &intent) {
        Ok(plan) => SelectionOutcome::Selected { plan },
        Err(code) => SelectionOutcome::Unavailable { code },
    };

    Ok(SelectionPreview {
        media,
        ranked_video_format_ids,
        ranked_audio_format_ids,
        outcome,
    })
}
