use std::{ffi::OsString, fs, path::PathBuf, time::Duration};

use serde::Deserialize;
pub mod stream_properties;
use stream_properties::{ProbeStream, PROBE_ENTRIES};

use crate::{
    application::{
        download_executor::StagedDownload,
        output_verifier::{OutputVerifier, VerifiedOutput},
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessFailureKind, ProcessProgram,
            ProcessRequest, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        download::{valid_opaque_id, DownloadErrorCode},
        format_selection::SelectionPlan,
        managed_tools::{ManagedToolError, ManagedToolErrorCode, ToolName},
    },
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const STDOUT_LIMIT: usize = 262_144;
const STDERR_LIMIT: usize = 32_768;
const MAX_FILE_BYTES: u64 = 9_007_199_254_740_991;
const MAX_STAGING_ENTRIES: usize = 8;
/// Source-aligned clipping may begin on the preceding keyframe.
pub const CLIP_DURATION_TOLERANCE_MS: u64 = 10_000;

pub struct FfprobeOutputVerifier<R, P> {
    resolver: R,
    process: P,
}

impl<R, P> FfprobeOutputVerifier<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}

impl<R, P> OutputVerifier for FfprobeOutputVerifier<R, P>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    fn verify(
        &self,
        staged: StagedDownload,
        plan: &SelectionPlan,
        cancellation: CancellationToken,
    ) -> Result<VerifiedOutput, DownloadErrorCode> {
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        validate_staged_contract(&staged, plan)?;
        let candidate = find_candidate(&staged)?;
        let byte_length = fs::symlink_metadata(&candidate)
            .map_err(|_| DownloadErrorCode::OutputInvalid)?
            .len();
        if byte_length == 0 || byte_length > MAX_FILE_BYTES {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        let container = candidate
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|value| recognized_extension(value))
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        if staged
            .expected_container
            .as_deref()
            .is_some_and(|expected| !expected.eq_ignore_ascii_case(&container))
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }

        let ffprobe = self
            .resolver
            .resolve_verified(ToolName::Ffprobe)
            .map_err(map_tool_error)?;
        let args = vec![
            OsString::from("-v"),
            OsString::from("error"),
            OsString::from("-print_format"),
            OsString::from("json"),
            OsString::from("-show_entries"),
            OsString::from(PROBE_ENTRIES),
            OsString::from("--"),
            candidate.as_os_str().to_owned(),
        ];
        let exit = self
            .process
            .run(
                ProcessRequest {
                    program: ProcessProgram::from_resolved_tool(ffprobe),
                    args,
                    working_dir: staged
                        .staging_dir
                        .canonicalize()
                        .map_err(|_| DownloadErrorCode::OutputInvalid)?,
                    timeout: Some(PROBE_TIMEOUT),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: STDOUT_LIMIT,
                        stderr_limit: STDERR_LIMIT,
                    },
                },
                cancellation.clone(),
                &DiscardProcessEvents,
            )
            .map_err(map_process_error)?;
        if cancellation.is_canceled() {
            return Err(DownloadErrorCode::ProcessFailed);
        }
        if exit.code != 0 {
            return Err(DownloadErrorCode::ProcessFailed);
        }

        let mut probe: ProbeDocument =
            serde_json::from_slice(&exit.stdout).map_err(|_| DownloadErrorCode::OutputInvalid)?;
        if let Some(stream) = probe.streams.iter_mut().find(|s| s.codec_type == "video") {
            if staged.evidence.video_processing
                == Some(crate::domain::output_policy::StreamProcessing::Copy)
                && stream.color_transfer.is_some()
                && staged
                    .evidence
                    .source_video
                    .as_ref()
                    .is_some_and(|source| stream.color_transfer != source.color_transfer)
            {
                return Err(DownloadErrorCode::OutputInvalid);
            }
            let mut properties = stream.video_properties()?;
            let missing_proven_copy_color = staged.evidence.video_processing
                == Some(crate::domain::output_policy::StreamProcessing::Copy)
                && staged.evidence.source_video.as_ref().is_some_and(|source| {
                    [
                        (source.pixel_format.as_ref(), stream.pix_fmt.as_ref()),
                        (source.color_range.as_ref(), stream.color_range.as_ref()),
                        (source.color_space.as_ref(), stream.color_space.as_ref()),
                        (
                            source.color_transfer.as_ref(),
                            stream.color_transfer.as_ref(),
                        ),
                        (
                            source.color_primaries.as_ref(),
                            stream.color_primaries.as_ref(),
                        ),
                    ]
                    .iter()
                    .any(|(known, summary)| known.is_some() && summary.is_none())
                });
            stream_properties::refine_video_properties(
                &self.resolver,
                &self.process,
                &candidate,
                &staged.staging_dir,
                &cancellation,
                &mut properties,
                missing_proven_copy_color
                    || (staged.evidence.video_processing
                        == Some(crate::domain::output_policy::StreamProcessing::Transcode)
                        && staged.evidence.source_video.as_ref().is_some_and(|source| {
                            matches!(
                                source.color_transfer.as_deref(),
                                Some("smpte2084" | "arib-std-b67")
                            )
                        })),
            )?;
            stream.pix_fmt = properties.pixel_format;
            stream.color_range = properties.color_range;
            stream.color_space = properties.color_space;
            stream.color_transfer = properties.color_transfer;
            stream.color_primaries = properties.color_primaries;
            stream.side_data_list = properties.hdr_side_data;
        }
        validate_probe(
            probe,
            plan,
            &container,
            byte_length,
            staged.requested_clip,
            &staged.evidence,
        )
        .map(|validated| VerifiedOutput {
            path: candidate,
            container,
            has_video: validated.has_video,
            has_audio: validated.has_audio,
            duration_ms: validated.duration_ms,
            byte_length,
        })
    }
}

fn validate_staged_contract(
    staged: &StagedDownload,
    plan: &SelectionPlan,
) -> Result<(), DownloadErrorCode> {
    use crate::domain::output_policy::{
        ContainerSelection, OutputContainer, StreamProcessing, TargetAudioCodec,
    };
    let evidence = &staged.evidence;
    let invalid = DownloadErrorCode::OutputInvalid;
    if !valid_opaque_id(&staged.job_id)
        || staged
            .staging_dir
            .file_name()
            .and_then(|name| name.to_str())
            != Some(&staged.job_id)
        || staged.expected_operation != plan.operation
        || staged.expected_container.as_deref() != Some(evidence.actual_container.as_str())
        || (plan.container_selection != ContainerSelection::Automatic
            && staged.expected_container != plan.output_container)
        || staged.requested_clip != plan.clip
        || evidence.source_video.is_some() != plan.video_format_id.is_some()
        || evidence.source_audio.is_some() != plan.audio_format_id.is_some()
        || plan
            .output_container
            .as_deref()
            .and_then(OutputContainer::parse)
            .is_none()
        || (plan.compatibility_mode
            != (plan.container_selection == ContainerSelection::Compatibility))
    {
        return Err(invalid);
    }
    let decision = if plan.compatibility_mode {
        crate::infrastructure::ffmpeg::compatibility::decide_compatibility(
            evidence.source_video.as_ref(),
            evidence.source_audio.as_ref(),
        )
        .map_err(|_| invalid)?
    } else {
        crate::infrastructure::ffmpeg::compatibility::CompatibilityDecision {
            video_processing: evidence
                .source_video
                .as_ref()
                .map(|_| StreamProcessing::Copy),
            audio_processing: evidence
                .source_audio
                .as_ref()
                .map(|_| StreamProcessing::Copy),
            hdr_to_sdr: false,
        }
    };
    let video_target = evidence.source_video.as_ref().map(|video| {
        if plan.compatibility_mode {
            "h264"
        } else {
            video.codec.as_str()
        }
    });
    let video_prediction = evidence.source_video.as_ref().map(|_| {
        if plan.compatibility_mode {
            StreamProcessing::Transcode
        } else {
            StreamProcessing::Copy
        }
    });
    let audio_target = evidence.source_audio.as_ref().map(|audio| {
        if plan.compatibility_mode {
            TargetAudioCodec::Aac
        } else {
            audio.codec
        }
    });
    let audio_prediction = evidence.source_audio.as_ref().map(|audio| {
        if Some(audio.codec) == audio_target {
            StreamProcessing::Copy
        } else {
            StreamProcessing::Transcode
        }
    });
    if evidence.video_processing != decision.video_processing
        || evidence.audio_processing != decision.audio_processing
        || plan.video_processing != video_prediction
        || plan.audio_processing != audio_prediction
        || plan.output_video_codec.as_deref() != video_target
        || plan.output_audio_codec != audio_target
    {
        return Err(invalid);
    }
    let expected = if plan.compatibility_mode {
        vec![if evidence.source_video.is_some() {
            OutputContainer::Mp4
        } else {
            OutputContainer::M4a
        }]
    } else if let Some(video) = &evidence.source_video {
        crate::domain::mux_capabilities::lossless_video_containers(
            &video.codec,
            evidence.source_audio.as_ref().map(|audio| audio.codec),
        )
        .to_vec()
    } else {
        vec![crate::domain::mux_capabilities::lossless_audio_container(
            evidence.source_audio.as_ref().ok_or(invalid)?.codec,
        )]
    };
    if !expected.contains(&evidence.actual_container)
        || (plan.container_selection == ContainerSelection::Automatic
            && expected.first() != Some(&evidence.actual_container))
    {
        return Err(invalid);
    }
    Ok(())
}

fn find_candidate(staged: &StagedDownload) -> Result<PathBuf, DownloadErrorCode> {
    let directory_metadata =
        fs::symlink_metadata(&staged.staging_dir).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !directory_metadata.is_dir()
        || directory_metadata.file_type().is_symlink()
        || is_reparse_point(&directory_metadata)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let canonical_staging =
        fs::canonicalize(&staged.staging_dir).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&canonical_staging).map_err(|_| DownloadErrorCode::OutputInvalid)? {
        if entries.len() >= MAX_STAGING_ENTRIES {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        entries.push(entry.map_err(|_| DownloadErrorCode::OutputInvalid)?);
    }
    let [entry] = entries.as_slice() else {
        return Err(DownloadErrorCode::OutputInvalid);
    };
    let name = entry
        .file_name()
        .to_str()
        .map(str::to_owned)
        .ok_or(DownloadErrorCode::OutputInvalid)?;
    if !name.starts_with("stage.") || name.matches('.').count() != 1 {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let metadata =
        fs::symlink_metadata(entry.path()).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let candidate = fs::canonicalize(entry.path()).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if candidate.parent() != Some(canonical_staging.as_path()) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(candidate)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeDocument {
    #[serde(default)]
    programs: Vec<serde_json::Value>,
    #[serde(default)]
    stream_groups: Vec<serde_json::Value>,
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeFormat {
    format_name: String,
    duration: String,
    size: String,
}

struct ValidatedProbe {
    has_video: bool,
    has_audio: bool,
    duration_ms: u64,
}

fn validate_probe(
    probe: ProbeDocument,
    plan: &SelectionPlan,
    extension: &str,
    byte_length: u64,
    requested_clip: Option<crate::domain::format_selection::ClipRange>,
    evidence: &crate::application::local_processing::ProcessingEvidence,
) -> Result<ValidatedProbe, DownloadErrorCode> {
    if !probe.programs.is_empty()
        || !probe.stream_groups.is_empty()
        || !container_matches(extension, &probe.format.format_name)
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let reported_size = probe
        .format
        .size
        .parse::<u64>()
        .ok()
        .filter(|size| *size == byte_length && *size > 0 && *size <= MAX_FILE_BYTES)
        .ok_or(DownloadErrorCode::OutputInvalid)?;
    debug_assert_eq!(reported_size, byte_length);
    let duration = probe
        .format
        .duration
        .parse::<f64>()
        .ok()
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or(DownloadErrorCode::OutputInvalid)?;
    let duration_ms_f64 = (duration * 1000.0).round();
    if !duration_ms_f64.is_finite()
        || duration_ms_f64 < 1.0
        || duration_ms_f64 > MAX_FILE_BYTES as f64
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let duration_ms = duration_ms_f64 as u64;
    if let Some(clip) = requested_clip {
        let requested_duration = clip
            .end_ms
            .checked_sub(clip.start_ms)
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        let maximum = requested_duration
            .checked_add(CLIP_DURATION_TOLERANCE_MS)
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        if duration_ms > maximum {
            return Err(DownloadErrorCode::OutputInvalid);
        }
    }

    let mut video_count = 0_u8;
    let mut audio_count = 0_u8;
    for stream in probe.streams {
        match stream.codec_type.as_str() {
            "video" => {
                video_count = video_count.saturating_add(1);
                let source = evidence
                    .source_video
                    .as_ref()
                    .ok_or(DownloadErrorCode::OutputInvalid)?;
                let actual = stream.video_properties()?;
                let unchanged = if evidence.video_processing
                    == Some(crate::domain::output_policy::StreamProcessing::Copy)
                {
                    actual.unchanged_from(source)
                } else if plan.compatibility_mode {
                    compatible_video_matches(&actual, source)?
                } else {
                    false
                };
                if !unchanged {
                    return Err(DownloadErrorCode::OutputInvalid);
                }
            }
            "audio" => {
                audio_count = audio_count.saturating_add(1);
                let target = plan
                    .output_audio_codec
                    .ok_or(DownloadErrorCode::OutputInvalid)?;
                let source = evidence
                    .source_audio
                    .as_ref()
                    .ok_or(DownloadErrorCode::OutputInvalid)?;
                let actual = stream.audio_properties()?;
                let unchanged = if evidence.audio_processing
                    == Some(crate::domain::output_policy::StreamProcessing::Copy)
                {
                    actual.unchanged_from(source)
                } else {
                    plan.compatibility_mode
                        && actual.codec == crate::domain::output_policy::TargetAudioCodec::Aac
                        && actual.channels == source.channels
                        && actual.sample_rate_hz == source.sample_rate_hz
                        && actual.channel_layout == source.channel_layout
                };
                if !target.matches(stream.codec_name.as_deref())
                    || plan.audio_processing.is_none()
                    || !unchanged
                {
                    return Err(DownloadErrorCode::OutputInvalid);
                }
            }
            _ => return Err(DownloadErrorCode::OutputInvalid),
        }
    }
    if plan.audio_format_id.is_none()
        && (plan.output_audio_codec.is_some() || plan.audio_processing.is_some())
    {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let expected_video = usize::from(plan.video_format_id.is_some());
    let expected_audio = usize::from(plan.audio_format_id.is_some());
    if usize::from(video_count) != expected_video || usize::from(audio_count) != expected_audio {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(ValidatedProbe {
        has_video: video_count == 1,
        has_audio: audio_count == 1,
        duration_ms,
    })
}

fn recognized_extension(extension: &str) -> bool {
    matches!(extension, "mp4" | "webm" | "mkv" | "m4a" | "mp3" | "flac")
}

fn compatible_video_matches(
    actual: &stream_properties::VideoStreamProperties,
    source: &stream_properties::VideoStreamProperties,
) -> Result<bool, DownloadErrorCode> {
    let hdr = crate::infrastructure::ffmpeg::compatibility::compatibility_filter(source)?.is_some();
    Ok(actual.codec == "h264"
        && matches!(
            actual.profile.as_deref(),
            Some("Baseline" | "Constrained Baseline" | "Main" | "High")
        )
        && actual.pixel_format.as_deref() == Some("yuv420p")
        && actual.width == source.width
        && actual.height == source.height
        && stream_properties::frame_rate(&actual.frame_rate)
            .zip(stream_properties::frame_rate(&source.frame_rate))
            .is_some_and(|(a, b)| stream_properties::rates_match(a, b))
        && actual.color_space.as_deref() == Some("bt709")
        && actual.color_transfer.as_deref() == Some("bt709")
        && actual.color_primaries.as_deref() == Some("bt709")
        && actual.color_range.as_deref()
            == if hdr {
                Some("tv")
            } else {
                source.color_range.as_deref()
            }
        && actual.hdr_side_data.is_empty())
}

fn container_matches(extension: &str, format_name: &str) -> bool {
    if format_name.len() > 128 {
        return false;
    }
    let names: Vec<_> = format_name.split(',').map(str::trim).collect();
    if names.is_empty()
        || names.iter().any(|name| {
            name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    {
        return false;
    }
    match extension {
        "mp4" | "m4a" | "m4v" | "mov" | "3gp" => names
            .iter()
            .any(|name| matches!(*name, "mov" | "mp4" | "m4a" | "3gp")),
        "mkv" => names.contains(&"matroska"),
        "webm" => names
            .iter()
            .any(|name| matches!(*name, "matroska" | "webm")),
        "ogg" | "opus" => names.contains(&"ogg"),
        "mp3" | "flac" | "wav" | "aac" => names.contains(&extension),
        _ => false,
    }
}

fn map_tool_error(error: ManagedToolError) -> DownloadErrorCode {
    match error.code {
        ManagedToolErrorCode::ToolMissing => DownloadErrorCode::ToolMissing,
        ManagedToolErrorCode::ToolIntegrityFailed
        | ManagedToolErrorCode::InvalidToolManifest
        | ManagedToolErrorCode::ToolVersionMismatch => DownloadErrorCode::ToolIntegrityFailed,
        ManagedToolErrorCode::UnsupportedTarget
        | ManagedToolErrorCode::ToolProbeFailed
        | ManagedToolErrorCode::ToolActivationFailed => DownloadErrorCode::ProcessFailed,
    }
}

fn map_process_error(error: ProcessFailureKind) -> DownloadErrorCode {
    match error {
        ProcessFailureKind::StdoutLimitExceeded
        | ProcessFailureKind::StderrLimitExceeded
        | ProcessFailureKind::LineLimitExceeded => DownloadErrorCode::OutputInvalid,
        ProcessFailureKind::LaunchFailed
        | ProcessFailureKind::JobControlFailed
        | ProcessFailureKind::IoFailed
        | ProcessFailureKind::TimedOut
        | ProcessFailureKind::InactivityTimedOut
        | ProcessFailureKind::Canceled => DownloadErrorCode::ProcessFailed,
    }
}

struct DiscardProcessEvents;

impl ProcessEventSink for DiscardProcessEvents {
    fn on_line(&self, _stream: crate::application::process::ProcessStream, _line: &[u8]) {}
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}
