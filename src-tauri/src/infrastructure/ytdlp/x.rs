use crate::infrastructure::x::hls_local::{HlsSegmentTransport, HttpHlsSegmentTransport};
use crate::infrastructure::x::manifest::{
    build_hls_snapshot, HlsManifestTransport, HttpXManifestTransport,
};
use crate::{
    application::{
        process::{
            OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest, ProcessStream,
            ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
        x_source::{XAttachmentIdentity, XInventory, XRequestContext},
        x_video::{match_x_video, resolve_x_video, ResolvedXVideo, XVideoAnalyzer},
    },
    domain::{managed_tools::ToolName, media::AnalysisErrorCode},
};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    time::{Duration, Instant},
};
pub struct YtDlpXVideoAnalyzer<R, P, T = HttpXManifestTransport, S = HttpHlsSegmentTransport> {
    resolver: R,
    process: P,
    manifest: T,
    segments: S,
}
impl<R, P> YtDlpXVideoAnalyzer<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self::with_manifest_transport(resolver, process, HttpXManifestTransport)
    }
}
impl<R, P, T> YtDlpXVideoAnalyzer<R, P, T> {
    pub fn with_manifest_transport(resolver: R, process: P, manifest: T) -> Self {
        Self {
            resolver,
            process,
            manifest,
            segments: HttpHlsSegmentTransport,
        }
    }
}
impl<R, P, T, S> YtDlpXVideoAnalyzer<R, P, T, S> {
    pub fn with_transports(resolver: R, process: P, manifest: T, segments: S) -> Self {
        Self {
            resolver,
            process,
            manifest,
            segments,
        }
    }
}
struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _: ProcessStream, _: &[u8]) {}
}
impl<
        R: ManagedToolResolver + Send + Sync,
        P: ProcessSupervisor + Send + Sync,
        T: HlsManifestTransport,
        S: HlsSegmentTransport,
    > XVideoAnalyzer for YtDlpXVideoAnalyzer<R, P, T, S>
{
    fn analyze_item(
        &self,
        inventory: &XInventory,
        identity: &XAttachmentIdentity,
        compatibility: bool,
        context: &XRequestContext,
    ) -> Result<ResolvedXVideo, AnalysisErrorCode> {
        if context.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        if let Ok(tool) = self.resolver.resolve_verified(ToolName::Ffmpeg) {
            crate::infrastructure::x::http_session::register_x_http_ffmpeg(&context.network, tool);
        }
        let item = inventory
            .items
            .iter()
            .find(|i| i.identity.as_ref() == Some(identity))
            .ok_or(AnalysisErrorCode::ContentChanged)?;
        let working = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let tool = self
            .resolver
            .resolve_verified(ToolName::YtDlp)
            .map_err(super::errors::map_tool_error)?;
        let args: Vec<OsString> = [
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-cache-dir",
            "--no-update",
            "--no-remote-components",
            "--no-cookies",
            "--no-cookies-from-browser",
            "--no-check-formats",
            "--simulate",
            "--dump-single-json",
            "--yes-playlist",
            "--socket-timeout",
            "10",
            "--retries",
            "0",
            "--extractor-retries",
            "0",
            "--use-extractors",
            "Twitter",
            "--",
            inventory.source.url(),
        ]
        .map(Into::into)
        .to_vec();
        let extraction_deadline = context
            .deadline
            .min(Instant::now() + Duration::from_secs(45));
        let mut retried = false;
        let extraction = loop {
            if context.cancellation.is_canceled() {
                return Err(AnalysisErrorCode::ProcessFailed);
            }
            let timeout = extraction_deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or(AnalysisErrorCode::AnalysisTimeout)?;
            let result = self.process.run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_resolved_tool(
                        crate::application::tool_resolver::ResolvedTool {
                            name: tool.name,
                            version: tool.version.clone(),
                            path: tool.path.clone(),
                        },
                    )
                    .with_network_profile(context.network.clone())
                    .with_explicit_https_proxy_for_host("x.com"),
                    args: args.clone(),
                    working_dir: working.path().to_owned(),
                    timeout: Some(timeout),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: 16 * 1024 * 1024,
                        stderr_limit: 65536,
                    },
                },
                context.cancellation.clone(),
                &Discard,
            );
            if context.cancellation.is_canceled() {
                return Err(AnalysisErrorCode::ProcessFailed);
            }
            // A transient TLS/connection failure can be retried once without
            // changing sources, guards, or the original extraction deadline.
            if !retried
                && matches!(&result, Ok(exit) if exit.code != 0
                    && super::errors::retryable_x_metadata_failure(&exit.stderr))
            {
                retried = true;
                continue;
            }
            break result;
        };
        let mut matched = match extraction {
            Ok(exit) if exit.code == 0 => {
                let value: Value = serde_json::from_slice(&exit.stdout)
                    .map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
                match_x_video(inventory, item, &value)?
            }
            outcome => {
                let error = match outcome {
                    Ok(exit) => super::errors::classify_nonzero_exit(&exit.stderr),
                    Err(error) => super::errors::map_process_error(error),
                };
                if context.cancellation.is_canceled()
                    || matches!(
                        error,
                        AnalysisErrorCode::AnalysisTimeout
                            | AnalysisErrorCode::ProcessFailed
                            | AnalysisErrorCode::InvalidMetadata
                    )
                {
                    return Err(error);
                }
                // The anonymous inventory itself can prove a complete direct-source set.
                if item.video_candidates.is_empty()
                    || item
                        .video_candidates
                        .iter()
                        .any(|c| c.content_type != "video/mp4")
                {
                    return Err(error);
                }
                let formats=item.video_candidates.iter().enumerate().map(|(n,c)|json!({"format_id":format!("x{}",n+1),"url":c.url.as_str(),"protocol":"https","ext":"mp4"})).collect::<Vec<_>>();
                match_x_video(
                    inventory,
                    item,
                    &json!({"id":identity.media_id,"display_id":identity.post_id,"formats":formats}),
                )?
            }
        };
        let rows = matched
            .raw
            .get("formats")
            .and_then(Value::as_array)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        if rows
            .iter()
            .filter(|row| {
                matches!(
                    row.get("protocol").and_then(Value::as_str),
                    Some("m3u8" | "m3u8_native")
                )
            })
            .count()
            > 16
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        for row in rows {
            match row
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("https")
            {
                "https" => {}
                "m3u8" | "m3u8_native" => {
                    let id = row
                        .get("format_id")
                        .and_then(Value::as_str)
                        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
                    let url = row
                        .get("url")
                        .and_then(Value::as_str)
                        .and_then(|s| url::Url::parse(s).ok())
                        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
                    let snapshot = build_hls_snapshot(&url, &self.manifest, context)?;
                    // A format must name a concrete rendition. Passing a
                    // master to FFmpeg would let it choose a different stream.
                    if !snapshot.is_media_playlist() {
                        return Err(AnalysisErrorCode::FormatUnavailable);
                    }
                    if matched
                        .hls
                        .insert(id.to_owned(), std::sync::Arc::new(snapshot))
                        .is_some()
                    {
                        return Err(AnalysisErrorCode::InvalidMetadata);
                    }
                }
                _ => return Err(AnalysisErrorCode::UnsupportedMedia),
            }
        }
        super::x_probe::hydrate_formats(
            &mut matched.raw,
            &matched.hls,
            &self.resolver,
            &self.process,
            &self.segments,
            working.path(),
            context,
        )?;
        if context.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        resolve_x_video(matched, context.network.clone(), compatibility)
    }
}

#[cfg(test)]
#[path = "x_metadata_retry_tests.rs"]
mod metadata_retry_tests;
