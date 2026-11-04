use crate::domain::{
    download::DownloadErrorCode,
    output_policy::{normalized_audio_codec, normalized_video_codec, TargetAudioCodec},
};
use serde::Deserialize;

pub const PROBE_ENTRIES: &str = "format=format_name,duration,size:stream=codec_type,codec_name,profile,width,height,avg_frame_rate,pix_fmt,color_range,color_space,color_transfer,color_primaries,channels,sample_rate,channel_layout:stream_side_data";

#[derive(Clone, Debug, PartialEq)]
pub struct VideoStreamProperties {
    pub profile: Option<String>,
    pub hdr_side_data: Vec<serde_json::Value>,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: String,
    pub pixel_format: Option<String>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioStreamProperties {
    pub codec: TargetAudioCodec,
    pub channels: u32,
    pub sample_rate_hz: u32,
    pub channel_layout: Option<String>,
}
impl AudioStreamProperties {
    pub fn unchanged_from(&self, source: &Self) -> bool {
        self == source
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeStream {
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub profile: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub avg_frame_rate: Option<String>,
    pub pix_fmt: Option<String>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub channels: Option<u32>,
    pub sample_rate: Option<String>,
    pub channel_layout: Option<String>,
    #[serde(default)]
    pub side_data_list: Vec<serde_json::Value>,
}

pub fn frame_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<u32>().ok()?;
    let denominator = denominator.parse::<u32>().ok()?;
    let rate = f64::from(numerator) / f64::from(denominator);
    (rate.is_finite() && rate > 0.0 && rate <= 240.0).then_some(rate)
}

pub fn rates_match(left: f64, right: f64) -> bool {
    left.is_finite()
        && right.is_finite()
        && left > 0.0
        && right > 0.0
        && (left - right).abs() / left <= 0.001
}

impl ProbeStream {
    pub fn audio_properties(&self) -> Result<AudioStreamProperties, DownloadErrorCode> {
        let invalid = DownloadErrorCode::OutputInvalid;
        Ok(AudioStreamProperties {
            codec: normalized_audio_codec(self.codec_name.as_deref()).ok_or(invalid)?,
            channels: self
                .channels
                .filter(|n| (1..=64).contains(n))
                .ok_or(invalid)?,
            sample_rate_hz: self
                .sample_rate
                .as_deref()
                .and_then(|n| n.parse().ok())
                .filter(|n| (1..=768000).contains(n))
                .ok_or(invalid)?,
            channel_layout: self.channel_layout.clone(),
        })
    }
    pub fn video_properties(&self) -> Result<VideoStreamProperties, DownloadErrorCode> {
        let invalid = DownloadErrorCode::OutputInvalid;
        let codec = normalized_video_codec(self.codec_name.as_deref()).ok_or(invalid)?;
        let width = self
            .width
            .filter(|n| (1..=16384).contains(n))
            .ok_or(invalid)?;
        let height = self
            .height
            .filter(|n| (1..=16384).contains(n))
            .ok_or(invalid)?;
        let frame_rate = self
            .avg_frame_rate
            .clone()
            .filter(|rate| frame_rate(rate).is_some())
            .ok_or(invalid)?;
        Ok(VideoStreamProperties {
            profile: self.profile.clone(),
            hdr_side_data: normalized_hdr_side_data(&self.side_data_list)?,
            codec: codec.to_owned(),
            width,
            height,
            frame_rate,
            pixel_format: self.pix_fmt.clone(),
            color_range: self.color_range.clone(),
            color_space: self.color_space.clone(),
            color_transfer: self.color_transfer.clone(),
            color_primaries: self.color_primaries.clone(),
        })
    }
}

impl VideoStreamProperties {
    pub fn unchanged_from(&self, source: &Self) -> bool {
        self.codec == source.codec
            && self.profile == source.profile
            && self.hdr_side_data == source.hdr_side_data
            && self.width == source.width
            && self.height == source.height
            && frame_rate(&self.frame_rate)
                .zip(frame_rate(&source.frame_rate))
                .is_some_and(|(left, right)| rates_match(left, right))
            && self.pixel_format == source.pixel_format
            && self.color_range == source.color_range
            && self.color_space == source.color_space
            && self.color_transfer == source.color_transfer
            && self.color_primaries == source.color_primaries
    }
}

pub fn normalized_hdr_side_data(
    values: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>, DownloadErrorCode> {
    if values.len() > 32 {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let mut result = Vec::new();
    for value in values {
        let kind = value
            .get("side_data_type")
            .and_then(|v| v.as_str())
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        match kind {
            "Mastering display metadata" | "Content light level metadata" => {
                result.push(value.clone())
            }
            "H.26[45] User Data Unregistered SEI message"
            | "CPB properties"
            | "New Extradata"
            | "MPEGTS Stream ID"
            | "Skip Samples" => {}
            _ => return Err(DownloadErrorCode::FormatUnavailable),
        }
    }
    result.sort_by_key(|v| {
        v.get("side_data_type")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned()
    });
    Ok(result)
}

pub fn refine_video_properties<
    R: crate::application::tool_resolver::ManagedToolResolver,
    P: crate::application::process::ProcessSupervisor,
>(
    resolver: &R,
    process: &P,
    source: &std::path::Path,
    staging: &std::path::Path,
    cancellation: &crate::application::process::CancellationToken,
    properties: &mut VideoStreamProperties,
    force_frame_probe: bool,
) -> Result<(), DownloadErrorCode> {
    if !force_frame_probe
        && properties.codec != "hevc"
        && properties.hdr_side_data.is_empty()
        && !matches!(
            properties.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        )
    {
        return Ok(());
    }
    use crate::{
        application::process::{OutputMode, ProcessProgram, ProcessRequest},
        domain::managed_tools::ToolName,
    };
    let tool = resolver
        .resolve_verified(ToolName::Ffprobe)
        .map_err(|error| match error.code {
            crate::domain::managed_tools::ManagedToolErrorCode::ToolMissing => {
                DownloadErrorCode::ToolMissing
            }
            crate::domain::managed_tools::ManagedToolErrorCode::ToolIntegrityFailed
            | crate::domain::managed_tools::ManagedToolErrorCode::InvalidToolManifest
            | crate::domain::managed_tools::ManagedToolErrorCode::ToolVersionMismatch => {
                DownloadErrorCode::ToolIntegrityFailed
            }
            _ => DownloadErrorCode::ProcessFailed,
        })?;
    let exit = process.run(ProcessRequest {
        stdin: None,
        program:ProcessProgram::from_resolved_tool(tool),
        args:vec!["-v".into(),"error".into(),"-select_streams".into(),"v:0".into(),"-read_intervals".into(),"%+#32".into(),"-show_frames".into(),"-show_entries".into(),"frame=pix_fmt,color_range,color_space,color_transfer,color_primaries,side_data_list".into(),"-of".into(),"json".into(),"--".into(),source.as_os_str().to_owned()],
        working_dir:staging.to_path_buf(),timeout:Some(std::time::Duration::from_secs(30)),inactivity_timeout:None,
        output_mode:OutputMode::Capture {stdout_limit:262144,stderr_limit:32768},
    },cancellation.clone(),&DiscardFrameEvents).map_err(|error| match error {
        crate::application::process::ProcessFailureKind::StdoutLimitExceeded | crate::application::process::ProcessFailureKind::StderrLimitExceeded => DownloadErrorCode::OutputInvalid,
        _ => DownloadErrorCode::ProcessFailed,
    })?;
    if cancellation.is_canceled() {
        return Err(DownloadErrorCode::ProcessFailed);
    }
    if exit.code != 0 {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let probe: FrameProbe =
        serde_json::from_slice(&exit.stdout).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if probe.frames.is_empty() || probe.frames.len() > 32 {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    for frame in &probe.frames {
        normalized_hdr_side_data(&frame.side_data_list)?;
    }
    let frame = &probe.frames[0];
    refine_property(&mut properties.pixel_format, &frame.pix_fmt)?;
    refine_property(&mut properties.color_range, &frame.color_range)?;
    refine_property(&mut properties.color_space, &frame.color_space)?;
    refine_property(&mut properties.color_transfer, &frame.color_transfer)?;
    refine_property(&mut properties.color_primaries, &frame.color_primaries)?;
    let frame_hdr = normalized_hdr_side_data(&frame.side_data_list)?;
    if properties.hdr_side_data.is_empty() {
        properties.hdr_side_data = frame_hdr;
    } else if properties.hdr_side_data != frame_hdr {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    // Every sampled frame must agree; static HDR evidence must not reappear later.
    for frame in &probe.frames[1..] {
        refine_property(&mut properties.pixel_format, &frame.pix_fmt)?;
        refine_property(&mut properties.color_range, &frame.color_range)?;
        refine_property(&mut properties.color_space, &frame.color_space)?;
        refine_property(&mut properties.color_transfer, &frame.color_transfer)?;
        refine_property(&mut properties.color_primaries, &frame.color_primaries)?;
        if normalized_hdr_side_data(&frame.side_data_list)? != properties.hdr_side_data {
            return Err(DownloadErrorCode::OutputInvalid);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameProbe {
    frames: Vec<ColorFrame>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColorFrame {
    pix_fmt: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
}
fn refine_property(
    summary: &mut Option<String>,
    frame: &Option<String>,
) -> Result<(), DownloadErrorCode> {
    if let Some(value) = frame {
        if summary.as_ref().is_some_and(|known| known != value) {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        if summary.is_none() {
            *summary = Some(value.clone());
        }
    }
    Ok(())
}
struct DiscardFrameEvents;
impl crate::application::process::ProcessEventSink for DiscardFrameEvents {
    fn on_line(&self, _: crate::application::process::ProcessStream, _: &[u8]) {}
}
