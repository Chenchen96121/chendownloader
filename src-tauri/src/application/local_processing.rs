use std::{path::PathBuf, sync::Arc};

use super::process::CancellationToken;
use crate::{
    domain::output_policy::{OutputContainer, StreamProcessing},
    domain::{download::DownloadErrorCode, format_selection::SelectionPlan, media::MediaFormat},
    infrastructure::ffprobe::stream_properties::{AudioStreamProperties, VideoStreamProperties},
};

#[derive(Clone, Debug, PartialEq)]
pub struct DownloadedSource {
    pub format_id: String,
    pub slot: usize,
}

#[derive(Clone, Debug)]
pub struct ProcessingEvidence {
    pub source_video: Option<VideoStreamProperties>,
    pub source_audio: Option<AudioStreamProperties>,
    pub actual_container: OutputContainer,
    pub video_processing: Option<StreamProcessing>,
    pub audio_processing: Option<StreamProcessing>,
}

pub struct LocalProcessingRequest {
    pub job_id: String,
    pub staging_dir: PathBuf,
    pub source_dir: PathBuf,
    pub plan: SelectionPlan,
    pub video_source: Option<MediaFormat>,
    pub audio_source: Option<MediaFormat>,
    pub sources: Vec<DownloadedSource>,
}

#[derive(Debug)]
pub struct ProcessedMedia {
    pub evidence: ProcessingEvidence,
}

pub trait LocalMediaProcessor: Send + Sync {
    fn process(
        &self,
        request: LocalProcessingRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode>;
}

impl<T: LocalMediaProcessor + ?Sized> LocalMediaProcessor for &T {
    fn process(
        &self,
        request: LocalProcessingRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode> {
        (**self).process(request, cancellation)
    }
}

impl<T: LocalMediaProcessor + ?Sized> LocalMediaProcessor for Arc<T> {
    fn process(
        &self,
        request: LocalProcessingRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode> {
        (**self).process(request, cancellation)
    }
}
