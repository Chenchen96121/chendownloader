use crate::domain::media::{AnalysisErrorCode, MediaInfo, Platform};

use super::normalize_metadata;

/// Closed, compile-time sample selection: no path, tool, or platform access.
pub fn preview_fixture(platform: Platform) -> Result<MediaInfo, AnalysisErrorCode> {
    let json = match platform {
        Platform::Youtube => include_str!("../../../../tests/fixtures/ytdlp/youtube.json"),
        Platform::Bilibili => include_str!("../../../../tests/fixtures/ytdlp/bilibili.json"),
        Platform::X => include_str!("../../../../tests/fixtures/ytdlp/x.json"),
        Platform::Instagram => include_str!("../../../../tests/fixtures/ytdlp/instagram.json"),
    };
    normalize_metadata(platform, json)
}
