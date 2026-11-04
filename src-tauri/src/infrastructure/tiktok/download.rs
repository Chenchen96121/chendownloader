//! TikTok originals enter the same verified local processing pipeline as other platforms.
use super::{inventory::network_json, tool::TikTokToolResolver};
use crate::{
    application::{
        download_executor::{
            DownloadExecutionRequest, DownloadExecutor, DownloadProgressSink, StagedDownload,
        },
        local_processing::{DownloadedSource, LocalMediaProcessor, LocalProcessingRequest},
        process::{CancellationToken, ProcessEventSink, ProcessStream},
        tiktok_source::TikTokRequestContext,
    },
    domain::{
        download::{
            DownloadErrorCode, DownloadMessageKey, DownloadProgress, DownloadProgressMetrics,
            DownloadState,
        },
        media::Platform,
    },
};
use serde_json::json;
use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct TikTokDownloadExecutor<M> {
    pub tool: Arc<TikTokToolResolver>,
    pub processor: M,
}
pub trait TikTokSourceInspector: Send + Sync {
    fn verify_silent_source(
        &self,
        source: &std::path::Path,
        staging: &std::path::Path,
        cancellation: &CancellationToken,
    ) -> Result<(), DownloadErrorCode>;
}
impl<
        R: crate::application::tool_resolver::ManagedToolResolver + Send + Sync,
        P: crate::application::process::ProcessSupervisor + Send + Sync,
    > TikTokSourceInspector for crate::infrastructure::ffmpeg::FfmpegMediaProcessor<R, P>
{
    fn verify_silent_source(
        &self,
        source: &std::path::Path,
        staging: &std::path::Path,
        cancellation: &CancellationToken,
    ) -> Result<(), DownloadErrorCode> {
        let probe = self.probe_complete_source(source, staging, cancellation, true)?;
        if probe
            .streams
            .iter()
            .any(|stream| stream.codec_type == "audio")
            || !probe
                .streams
                .iter()
                .any(|stream| stream.codec_type == "video")
        {
            return Err(DownloadErrorCode::ContentChanged);
        }
        Ok(())
    }
}
struct Progress<'a> {
    job: &'a str,
    sink: &'a dyn DownloadProgressSink,
    slot: usize,
    count: usize,
    completed_bytes: u64,
}
impl ProcessEventSink for Progress<'_> {
    fn on_line(&self, _: ProcessStream, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            return;
        };
        let current = value
            .pointer("/data/downloadedBytes")
            .and_then(|v| v.as_u64());
        let total = value.pointer("/data/totalBytes").and_then(|v| v.as_u64());
        let downloaded = current.and_then(|n| n.checked_add(self.completed_bytes));
        if let Ok(p) = DownloadProgress::new(
            self.job,
            DownloadState::Downloading,
            DownloadProgressMetrics {
                downloaded_bytes: downloaded,
                total_bytes: if self.slot + 1 == self.count {
                    total.and_then(|n| n.checked_add(self.completed_bytes))
                } else {
                    None
                },
                percent: current.zip(total).filter(|(_, t)| *t > 0).map(|(d, t)| {
                    ((self.slot as f64 + d as f64 / t as f64) / self.count as f64 * 100.0)
                        .min(100.0)
                }),
                ..Default::default()
            },
            DownloadMessageKey::Downloading,
        ) {
            self.sink.on_progress(p);
        }
    }
}
impl<M: LocalMediaProcessor + TikTokSourceInspector> DownloadExecutor
    for TikTokDownloadExecutor<M>
{
    fn download(
        &self,
        request: DownloadExecutionRequest,
        cancellation: CancellationToken,
        progress: &dyn DownloadProgressSink,
    ) -> Result<StagedDownload, DownloadErrorCode> {
        if request.source.platform() != Platform::Tiktok
            || request.x_source.is_some()
            || request.bilibili_source.is_some()
            || request.instagram_selector.is_some()
            || request.playlist_item.is_some()
            || request.plan.clip.is_some()
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let source = request
            .tiktok_source
            .as_ref()
            .ok_or(DownloadErrorCode::InvalidRequest)?;
        source.validate_request(
            &request.plan,
            request.video_source.as_ref(),
            request.audio_source.as_ref(),
        )?;
        let staging = request
            .staging_dir
            .canonicalize()
            .map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if staging.file_name().and_then(|v| v.to_str()) != Some(request.job_id.as_str())
            || fs::symlink_metadata(&request.staging_dir)
                .map_err(|_| DownloadErrorCode::OutputInvalid)?
                .file_type()
                .is_symlink()
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let directory = staging.join("source");
        fs::create_dir(&directory).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        let context = TikTokRequestContext::new(
            cancellation.clone(),
            Instant::now() + Duration::from_secs(24 * 3600),
            source.network(),
        );
        let result = (|| {
            let mut sources = Vec::new();
            let mut completed_bytes = 0u64;
            for (slot, id) in request.plan.source_format_ids.iter().enumerate() {
                if cancellation.is_canceled() {
                    return Err(DownloadErrorCode::ProcessFailed);
                }
                let input = directory.join(format!("input-{slot}"));
                fs::create_dir(&input).map_err(|_| DownloadErrorCode::OutputInvalid)?;
                let path = input.join("source.part");
                self.tool.run_download(&json!({"protocolVersion":1,"operation":"download","url":request.source.url(),"network":network_json(&context),"context":source.context(id)?,"asset":source.asset(id)?,"stagingPath":path}),24*3600,cancellation.clone(),Some(&Progress{job:&request.job_id,sink:progress,slot,count:request.plan.source_format_ids.len(),completed_bytes}))?;
                completed_bytes = completed_bytes
                    .checked_add(
                        fs::metadata(&path)
                            .map_err(|_| DownloadErrorCode::OutputInvalid)?
                            .len(),
                    )
                    .ok_or(DownloadErrorCode::OutputInvalid)?;
                fs::rename(&path, input.join("source.mp4"))
                    .map_err(|_| DownloadErrorCode::OutputInvalid)?;
                if source
                    .formats
                    .iter()
                    .find(|format| format.id == *id)
                    .is_some_and(|format| {
                        format.kind == crate::domain::media::StreamKind::VideoOnly
                    })
                {
                    self.processor.verify_silent_source(
                        &input.join("source.mp4"),
                        &staging,
                        &cancellation,
                    )?;
                }
                sources.push(DownloadedSource {
                    format_id: id.clone(),
                    slot,
                });
            }
            progress.on_progress(DownloadProgress::new(
                &request.job_id,
                DownloadState::Processing,
                DownloadProgressMetrics::default(),
                DownloadMessageKey::Processing,
            )?);
            self.processor.process(
                LocalProcessingRequest {
                    job_id: request.job_id.clone(),
                    staging_dir: staging.clone(),
                    source_dir: directory.clone(),
                    plan: request.plan.clone(),
                    video_source: request.video_source,
                    audio_source: request.audio_source,
                    sources,
                },
                cancellation.clone(),
            )
        })();
        // The directory was created above exclusively for this job, and contains only private inputs.
        fs::remove_dir_all(&directory).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        let processed = result?;
        Ok(StagedDownload {
            job_id: request.job_id,
            staging_dir: staging,
            expected_operation: request.plan.operation,
            expected_container: Some(processed.evidence.actual_container.as_str().into()),
            requested_clip: None,
            evidence: processed.evidence,
        })
    }
}
