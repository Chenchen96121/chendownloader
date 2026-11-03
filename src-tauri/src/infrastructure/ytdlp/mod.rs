//! Project bundled or future extractor metadata into the sanitized domain contract.
pub mod bilibili;
pub mod bilibili_download;
mod bilibili_probe;
mod bilibili_witness;
pub mod download;
mod errors;
mod fixtures;
mod instagram_probe;
mod live;
mod metadata;
pub mod progress;
pub mod x;
mod x_probe;

pub(super) const INSTAGRAM_BROWSER_PROFILE: &str = "chrome-110";

pub use fixtures::preview_fixture;
pub use live::YtDlpMetadataAnalyzer;
pub use metadata::{normalize_instagram_video, normalize_metadata, YtDlpInstagramVideoAnalyzer};
