pub mod compatibility;
mod processing;
mod source_guard;
pub use processing::FfmpegMediaProcessor;
pub(crate) use source_guard::{canonical_job_directory, validate_sources, SourceGuards};
