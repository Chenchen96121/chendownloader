use super::source_guard::{
    canonical_job_directory, path_too_long, reject_existing_path, validate_output,
    validate_sources, SourceGuards,
};
use crate::{
    application::{
        local_processing::{
            LocalMediaProcessor, LocalProcessingRequest, ProcessedMedia, ProcessingEvidence,
        },
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        download::DownloadErrorCode,
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
        media::MediaFormat,
        mux_capabilities::{lossless_audio_container, lossless_video_containers},
        output_policy::{
            normalized_audio_codec, normalized_video_codec, ContainerSelection, OutputContainer,
            StreamProcessing,
        },
    },
    infrastructure::ffprobe::stream_properties::{
        frame_rate, rates_match, refine_video_properties, ProbeStream, PROBE_ENTRIES,
    },
};
use serde::Deserialize;
use std::{ffi::OsString, path::Path, time::Duration};

pub struct FfmpegMediaProcessor<R, P> {
    resolver: R,
    process: P,
}
impl<R, P> FfmpegMediaProcessor<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceProbe {
    #[serde(default)]
    programs: Vec<serde_json::Value>,
    #[serde(default)]
    stream_groups: Vec<serde_json::Value>,
    pub(crate) streams: Vec<ProbeStream>,
    pub(crate) format: serde_json::Value,
}

impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync>
    FfmpegMediaProcessor<R, P>
{
    fn probe_source(
        &self,
        source: &Path,
        staging: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<ProbeStream>, DownloadErrorCode> {
        Ok(self
            .probe_complete_source(source, staging, cancellation, false)?
            .streams)
    }
    pub(crate) fn probe_complete_source(
        &self,
        source: &Path,
        staging: &Path,
        cancellation: &CancellationToken,
        local_only: bool,
    ) -> Result<SourceProbe, DownloadErrorCode> {
        let probe = self
            .resolver
            .resolve_verified(ToolName::Ffprobe)
            .map_err(map_tool_error)?;
        let mut args: Vec<OsString> = vec![
            "-v".into(),
            "error".into(),
            "-print_format".into(),
            "json".into(),
            "-show_entries".into(),
            PROBE_ENTRIES.into(),
        ];
        if local_only {
            args.extend([
                "-protocol_whitelist".into(),
                "file".into(),
                "-format_whitelist".into(),
                "mov,matroska,webm,flac,aac,mp3,ogg".into(),
            ]);
        }
        args.extend(["--".into(), source.as_os_str().to_owned()]);
        let exit = self
            .process
            .run(
                ProcessRequest {
                    program: ProcessProgram::from_resolved_tool(probe),
                    args,
                    working_dir: staging.to_path_buf(),
                    timeout: Some(Duration::from_secs(30)),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: 262_144,
                        stderr_limit: 32_768,
                    },
                },
                cancellation.clone(),
                &DiscardEvents,
            )
            .map_err(map_process_error)?;
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        if exit.code != 0 {
            return Err(map_nonzero(&exit.stderr));
        }
        let probe: SourceProbe =
            serde_json::from_slice(&exit.stdout).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if !probe.programs.is_empty()
            || !probe.stream_groups.is_empty()
            || !probe.format.is_object()
            || probe.streams.is_empty()
            || probe.streams.len() > 2
            || probe.streams.iter().any(|s| {
                !matches!(s.codec_type.as_str(), "video" | "audio") || s.side_data_list.len() > 32
            })
            || probe
                .streams
                .iter()
                .filter(|s| s.codec_type == "video")
                .count()
                > 1
            || probe
                .streams
                .iter()
                .filter(|s| s.codec_type == "audio")
                .count()
                > 1
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        Ok(probe)
    }
}

impl<R: ManagedToolResolver + Send + Sync, P: ProcessSupervisor + Send + Sync> LocalMediaProcessor
    for FfmpegMediaProcessor<R, P>
{
    fn process(
        &self,
        request: LocalProcessingRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessedMedia, DownloadErrorCode> {
        if (!request.plan.compatibility_mode
            && (request.plan.audio_processing == Some(StreamProcessing::Transcode)
                || request.plan.video_processing == Some(StreamProcessing::Transcode)
                || request.plan.container_selection == ContainerSelection::Compatibility))
            || (request.plan.compatibility_mode
                && request.plan.container_selection != ContainerSelection::Compatibility)
        {
            return Err(DownloadErrorCode::FormatUnavailable);
        }
        if cancellation.is_canceled()
            || request
                .plan
                .clip
                .is_some_and(|c| c.start_ms >= c.end_ms || c.end_ms > 31_536_000_000)
        {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        let sources = validate_sources(&request)?;
        let staging = canonical_job_directory(&request)?;
        let _guards = SourceGuards::acquire(&request, &staging, &sources)?;
        let ffmpeg = self
            .resolver
            .resolve_verified(ToolName::Ffmpeg)
            .map_err(map_tool_error)?;
        let probes: Vec<_> = sources
            .iter()
            .map(|source| self.probe_source(source, &staging, &cancellation))
            .collect::<Result<_, _>>()?;
        let role_index = |id: &str| {
            request
                .sources
                .iter()
                .position(|s| s.format_id == id)
                .ok_or(DownloadErrorCode::OutputInvalid)
        };
        let video_index = request
            .plan
            .video_format_id
            .as_deref()
            .map(role_index)
            .transpose()?;
        let audio_index = request
            .plan
            .audio_format_id
            .as_deref()
            .map(role_index)
            .transpose()?;
        let source_video = if let Some(index) = video_index {
            let stream = probes[index]
                .iter()
                .find(|s| s.codec_type == "video")
                .ok_or(DownloadErrorCode::OutputInvalid)?;
            let mut properties = stream.video_properties()?;
            if request.plan.video_processing
                != Some(if request.plan.compatibility_mode {
                    StreamProcessing::Transcode
                } else {
                    StreamProcessing::Copy
                })
                || request.plan.output_video_codec.as_deref()
                    != Some(if request.plan.compatibility_mode {
                        "h264"
                    } else {
                        properties.codec.as_str()
                    })
            {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            let missing_compatibility_colors = request.plan.compatibility_mode
                && (properties.color_transfer.is_none()
                    || properties.color_primaries.is_none()
                    || properties.color_space.is_none()
                    || properties.color_range.is_none());
            refine_video_properties(
                &self.resolver,
                &self.process,
                &sources[index],
                &staging,
                &cancellation,
                &mut properties,
                missing_compatibility_colors,
            )?;
            let selected = request
                .video_source
                .as_ref()
                .filter(|s| Some(&s.id) == request.plan.video_format_id.as_ref())
                .ok_or(DownloadErrorCode::OutputInvalid)?;
            let transfer = properties.color_transfer.as_deref();
            let hdr = matches!(transfer, Some("smpte2084" | "arib-std-b67"));
            let advertised_hdr = matches!(
                selected.dynamic_range.as_deref(),
                Some("HDR" | "HDR10" | "HLG")
            );
            if (advertised_hdr && !hdr)
                || (selected.dynamic_range.as_deref() == Some("HDR10")
                    && transfer != Some("smpte2084"))
                || (selected.dynamic_range.as_deref() == Some("HLG")
                    && transfer != Some("arib-std-b67"))
                || (selected.dynamic_range.as_deref() == Some("SDR") && hdr)
                || ((!properties.hdr_side_data.is_empty() || hdr)
                    && (!hdr
                        || properties.color_primaries.as_deref() != Some("bt2020")
                        || properties.color_space.as_deref() != Some("bt2020nc")
                        || !matches!(properties.color_range.as_deref(), Some("tv" | "pc"))
                        || properties.pixel_format.as_deref() != Some("yuv420p10le")))
            {
                return Err(DownloadErrorCode::FormatUnavailable);
            }
            if normalized_video_codec(selected.video_codec.as_deref())
                != Some(properties.codec.as_str())
                || selected.width.is_some_and(|n| n != properties.width)
                || selected.height.is_some_and(|n| n != properties.height)
                || selected.fps.is_some_and(|fps| {
                    !frame_rate(&properties.frame_rate)
                        .is_some_and(|actual| rates_match(fps, actual))
                })
                || matches!(
                    selected.dynamic_range.as_deref(),
                    Some("DV" | "HDR10+" | "HDR12")
                )
            {
                return Err(DownloadErrorCode::FormatUnavailable);
            }
            Some(properties)
        } else {
            if request.video_source.is_some()
                || request.plan.video_processing.is_some()
                || request.plan.output_video_codec.is_some()
            {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            None
        };
        let source_audio = if let Some(index) = audio_index {
            let stream = probes[index]
                .iter()
                .find(|s| s.codec_type == "audio")
                .ok_or(DownloadErrorCode::OutputInvalid)?;
            let properties = stream.audio_properties()?;
            let selected = request
                .audio_source
                .as_ref()
                .filter(|s| Some(&s.id) == request.plan.audio_format_id.as_ref())
                .ok_or(DownloadErrorCode::OutputInvalid)?;
            validate_audio_metadata(selected, &properties)?;
            let expected_codec = if request.plan.compatibility_mode {
                crate::domain::output_policy::TargetAudioCodec::Aac
            } else {
                properties.codec
            };
            if request.plan.output_audio_codec != Some(expected_codec)
                || request.plan.audio_processing
                    != Some(if properties.codec == expected_codec {
                        StreamProcessing::Copy
                    } else {
                        StreamProcessing::Transcode
                    })
            {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            Some(properties)
        } else {
            if request.audio_source.is_some()
                || request.plan.output_audio_codec.is_some()
                || request.plan.audio_processing.is_some()
            {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            None
        };
        if source_video.is_none() && source_audio.is_none() {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let decision = if request.plan.compatibility_mode {
            super::compatibility::decide_compatibility(
                source_video.as_ref(),
                source_audio.as_ref(),
            )?
        } else {
            super::compatibility::CompatibilityDecision {
                video_processing: video_index.map(|_| StreamProcessing::Copy),
                audio_processing: audio_index.map(|_| StreamProcessing::Copy),
                hdr_to_sdr: false,
            }
        };
        let candidates: Vec<_> = if request.plan.compatibility_mode {
            vec![if source_video.is_some() {
                OutputContainer::Mp4
            } else {
                OutputContainer::M4a
            }]
        } else if let Some(video) = &source_video {
            lossless_video_containers(&video.codec, source_audio.as_ref().map(|a| a.codec)).to_vec()
        } else {
            vec![lossless_audio_container(
                source_audio
                    .as_ref()
                    .ok_or(DownloadErrorCode::OutputInvalid)?
                    .codec,
            )]
        };
        let predicted = request
            .plan
            .output_container
            .as_deref()
            .and_then(OutputContainer::parse)
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        let container = if request.plan.container_selection == ContainerSelection::Automatic {
            *candidates
                .first()
                .ok_or(DownloadErrorCode::FormatUnavailable)?
        } else {
            if !candidates.contains(&predicted) {
                return Err(DownloadErrorCode::FormatUnavailable);
            }
            predicted
        };
        let output = staging.join(format!("stage.{}", container.as_str()));
        reject_existing_path(&output)?;
        if path_too_long(&output) {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let mut args: Vec<OsString> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-nostdin".into(),
        ];
        for source in &sources {
            if let Some(clip) = request.plan.clip {
                args.extend(["-ss".into(), timestamp(clip.start_ms).into()]);
            }
            args.extend(["-i".into(), source.as_os_str().to_owned()]);
        }
        if let Some(clip) = request.plan.clip {
            args.extend(["-t".into(), timestamp(clip.end_ms - clip.start_ms).into()]);
        }
        if let Some(index) = video_index {
            args.extend(["-map".into(), format!("{index}:v:0").into()]);
            if decision.video_processing == Some(StreamProcessing::Copy) {
                args.extend(["-c:v".into(), "copy".into()]);
            } else {
                if !request.plan.compatibility_mode {
                    return Err(DownloadErrorCode::OutputInvalid);
                }
                let video = source_video
                    .as_ref()
                    .ok_or(DownloadErrorCode::OutputInvalid)?;
                args.extend([
                    "-c:v".into(),
                    "libx264".into(),
                    "-crf".into(),
                    "18".into(),
                    "-preset".into(),
                    "medium".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                    "-fps_mode".into(),
                    "passthrough".into(),
                ]);
                if let Some(filter) = super::compatibility::compatibility_filter(video)? {
                    args.extend(["-vf".into(), filter.into()]);
                }
                args.extend([
                    "-color_range".into(),
                    if decision.hdr_to_sdr {
                        "tv"
                    } else {
                        video
                            .color_range
                            .as_deref()
                            .ok_or(DownloadErrorCode::FormatUnavailable)?
                    }
                    .into(),
                    "-colorspace".into(),
                    "bt709".into(),
                    "-color_trc".into(),
                    "bt709".into(),
                    "-color_primaries".into(),
                    "bt709".into(),
                ]);
            }
        } else {
            args.push("-vn".into());
        }
        if let Some(index) = audio_index {
            args.extend(["-map".into(), format!("{index}:a:0").into()]);
            if decision.audio_processing == Some(StreamProcessing::Copy) {
                args.extend(["-c:a".into(), "copy".into()]);
            } else {
                if !request.plan.compatibility_mode {
                    return Err(DownloadErrorCode::OutputInvalid);
                }
                let audio = source_audio
                    .as_ref()
                    .ok_or(DownloadErrorCode::OutputInvalid)?;
                args.extend([
                    "-c:a".into(),
                    "aac".into(),
                    "-profile:a".into(),
                    "aac_low".into(),
                    "-b:a".into(),
                    format!("{}k", (audio.channels * 96).min(512)).into(),
                    "-ar".into(),
                    audio.sample_rate_hz.to_string().into(),
                ]);
            }
        } else {
            args.push("-an".into());
        }
        args.extend([
            "-sn".into(),
            "-dn".into(),
            "-avoid_negative_ts".into(),
            "make_zero".into(),
        ]);
        if matches!(container, OutputContainer::Mp4 | OutputContainer::M4a) {
            // MOV otherwise drops container-only color declarations when the
            // bitstream does not repeat all primaries/transfer/matrix fields.
            // Only force a color atom with a proven range: nclx carries a
            // mandatory full-range bit, so an unknown range must stay unknown.
            let preserve_colors = decision.video_processing == Some(StreamProcessing::Copy)
                && source_video
                    .as_ref()
                    .is_some_and(|video| matches!(video.color_range.as_deref(), Some("tv" | "pc")));
            args.extend([
                "-movflags".into(),
                if preserve_colors {
                    "+faststart+write_colr"
                } else {
                    "+faststart"
                }
                .into(),
            ]);
        }
        args.extend(["-n".into(), output.as_os_str().to_owned()]);
        let exit = self
            .process
            .run(
                ProcessRequest {
                    program: ProcessProgram::from_resolved_tool(ffmpeg),
                    args,
                    working_dir: staging,
                    timeout: Some(Duration::from_secs(3600)),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: 32768,
                        stderr_limit: 32768,
                    },
                },
                cancellation.clone(),
                &DiscardEvents,
            )
            .map_err(map_process_error)?;
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        if exit.code != 0 {
            return Err(map_nonzero(&exit.stderr));
        }
        validate_output(&request, &sources, &output, container.as_str())?;
        Ok(ProcessedMedia {
            evidence: ProcessingEvidence {
                source_video,
                source_audio,
                actual_container: container,
                video_processing: decision.video_processing,
                audio_processing: decision.audio_processing,
            },
        })
    }
}

fn validate_audio_metadata(
    selected: &MediaFormat,
    actual: &crate::infrastructure::ffprobe::stream_properties::AudioStreamProperties,
) -> Result<(), DownloadErrorCode> {
    if normalized_audio_codec(selected.audio_codec.as_deref()) != Some(actual.codec)
        || selected
            .audio_channels
            .is_some_and(|n| u32::from(n) != actual.channels)
        || selected
            .audio_sample_rate_hz
            .is_some_and(|n| n != actual.sample_rate_hz)
    {
        Err(DownloadErrorCode::OutputInvalid)
    } else {
        Ok(())
    }
}
fn timestamp(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3600000,
        (ms / 60000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}
fn map_tool_error(error: ManagedToolError) -> DownloadErrorCode {
    match error.code {
        ManagedToolErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        ManagedToolErrorCode::ToolIntegrityFailed
        | ManagedToolErrorCode::InvalidToolManifest
        | ManagedToolErrorCode::ToolVersionMismatch => DownloadErrorCode::ToolIntegrityFailed,
        _ => DownloadErrorCode::ProcessFailed,
    }
}
fn map_process_error(error: ProcessFailureKind) -> DownloadErrorCode {
    match error {
        ProcessFailureKind::StdoutLimitExceeded
        | ProcessFailureKind::StderrLimitExceeded
        | ProcessFailureKind::LineLimitExceeded => DownloadErrorCode::OutputInvalid,
        _ => DownloadErrorCode::ProcessFailed,
    }
}
fn map_nonzero(stderr: &[u8]) -> DownloadErrorCode {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if [
        "no space left on device",
        "not enough space on the disk",
        "disk full",
    ]
    .iter()
    .any(|n| text.contains(n))
    {
        DownloadErrorCode::DiskFull
    } else {
        DownloadErrorCode::ProcessFailed
    }
}
struct DiscardEvents;
impl ProcessEventSink for DiscardEvents {
    fn on_line(&self, _: crate::application::process::ProcessStream, _: &[u8]) {}
}
