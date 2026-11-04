//! Disposable bounded local samples. Sample files are never sent to a publisher.
use super::{inventory::network_json, tool::TikTokToolResolver};
use crate::{
    application::{
        process::{
            OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest, ProcessStream,
            ProcessSupervisor,
        },
        tiktok_source::{TikTokNativeAsset, TikTokRequestContext},
        tiktok_video::TikTokCandidateProbe,
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        managed_tools::ToolName,
        media::AnalysisErrorCode,
        output_policy::{normalized_audio_codec, normalized_video_codec},
    },
    infrastructure::ffprobe::stream_properties::frame_rate,
};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};
pub struct NativeTikTokCandidateProbe<R, P> {
    pub resolver: R,
    pub process: P,
    pub tool: Arc<TikTokToolResolver>,
}
struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _: ProcessStream, _: &[u8]) {}
}
impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync> TikTokCandidateProbe
    for NativeTikTokCandidateProbe<R, P>
{
    fn probe(
        &self,
        native: &TikTokNativeAsset,
        candidate: &Value,
        context: &TikTokRequestContext,
    ) -> Result<Value, AnalysisErrorCode> {
        let seconds = context
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .min(15);
        if seconds == 0 || context.cancellation.is_canceled() {
            return Err(AnalysisErrorCode::AnalysisTimeout);
        }
        let directory = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let path = directory.path().join("sample.part");
        let mut asset = candidate.clone();
        asset
            .as_object_mut()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?
            .remove("requestContext");
        self.tool.run(&json!({"protocolVersion":1,"operation":"probe","url":native.data.get("sourcePageUrl"),"network":network_json(context),"context":candidate.get("requestContext").unwrap_or(&native.context),"asset":asset,"stagingPath":path}),seconds,context.cancellation.clone(),None)?;
        let length = std::fs::metadata(&path)
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?
            .len();
        if length == 0 || length > 1024 * 1024 {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let probe = self
            .resolver
            .resolve_verified(ToolName::Ffprobe)
            .map_err(|_| AnalysisErrorCode::ToolMissing)?;
        let remaining = context
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(15));
        if remaining.is_zero() {
            return Err(AnalysisErrorCode::AnalysisTimeout);
        }
        let mut args:Vec<OsString>=["-v","error","-protocol_whitelist","file","-show_entries","stream=codec_type,codec_name,width,height,avg_frame_rate,color_transfer,channels,sample_rate,bit_rate","-of","json","-i"].into_iter().map(Into::into).collect();
        args.push(path.into_os_string());
        let exit = self
            .process
            .run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_resolved_tool(probe),
                    args,
                    working_dir: directory.path().to_owned(),
                    timeout: Some(remaining),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: 65536,
                        stderr_limit: 4096,
                    },
                },
                context.cancellation.clone(),
                &Discard,
            )
            .map_err(|_| AnalysisErrorCode::AnalysisTimeout)?;
        if exit.code != 0 {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        let raw: Value =
            serde_json::from_slice(&exit.stdout).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        let rows = raw
            .get("streams")
            .and_then(Value::as_array)
            .filter(|rows| !rows.is_empty() && rows.len() <= 2)
            .ok_or(AnalysisErrorCode::FormatUnavailable)?;
        let video = rows
            .iter()
            .find(|r| r["codec_type"] == "video")
            .ok_or(AnalysisErrorCode::FormatUnavailable)?;
        let audio = rows.iter().find(|r| r["codec_type"] == "audio");
        let number = |row: &Value, key: &str| {
            row.get(key)
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
                })
                .filter(|n| n.is_finite() && *n > 0.0)
        };
        let range = match video.get("color_transfer").and_then(Value::as_str) {
            Some("smpte2084") => Some("HDR10"),
            Some("arib-std-b67") => Some("HLG"),
            Some("bt709" | "gamma22" | "gamma28" | "iec61966-2-1" | "smpte170m") => Some("SDR"),
            _ => None,
        };
        let codec = normalized_video_codec(video.get("codec_name").and_then(Value::as_str))
            .ok_or(AnalysisErrorCode::FormatUnavailable)?;
        Ok(
            json!({"width":video.get("width"),"height":video.get("height"),"videoCodec":codec,"fps":video.get("avg_frame_rate").and_then(Value::as_str).and_then(frame_rate),"dynamicRange":range,"videoBitrateKbps":number(video,"bit_rate").map(|n|n/1000.0),"audioCodec":audio.and_then(|r|normalized_audio_codec(r.get("codec_name").and_then(Value::as_str))).map(|c|c.as_str()),"audioAbsent":audio.is_none(),"audioChannels":audio.and_then(|r|r.get("channels")),"audioSampleRateHz":audio.and_then(|r|number(r,"sample_rate")).map(|n|n as u32),"audioBitrateKbps":audio.and_then(|r|number(r,"bit_rate")).map(|n|n/1000.0)}),
        )
    }
}
