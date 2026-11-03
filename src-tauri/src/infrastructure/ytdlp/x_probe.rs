//! X extractors can omit codec facts. Probe approved native-only CDN
//! URLs before selection; never invent codecs or silently prefer a lower stream.
use std::{
    collections::HashMap,
    ffi::OsString,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use crate::{
    application::{
        process::{
            OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest, ProcessStream,
            ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        managed_tools::ToolName,
        media::AnalysisErrorCode,
        output_policy::{normalized_audio_codec, normalized_video_codec},
    },
    infrastructure::ffprobe::stream_properties::frame_rate,
};

const ENTRIES: &str = "format=duration,size:stream=codec_type,codec_name,width,height,avg_frame_rate,color_transfer,bit_rate,sample_rate,channels";
const LIMIT: usize = 65_536;

pub(super) fn hydrate_formats<R: ManagedToolResolver, P: ProcessSupervisor>(
    metadata: &mut Value,
    hls: &HashMap<String, Arc<crate::infrastructure::x::manifest::VerifiedHlsSnapshot>>,
    resolver: &R,
    process: &P,
    segments: &dyn crate::infrastructure::x::hls_local::HlsSegmentTransport,
    working_dir: &Path,
    context: &crate::application::x_source::XRequestContext,
) -> Result<(), AnalysisErrorCode> {
    let cancellation = &context.cancellation;
    let deadline = context.deadline;
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let mut probed = std::collections::HashSet::new();
    let mut process_attempts = 0;
    let mut mandatory = true;
    loop {
        let rows = metadata
            .get("formats")
            .and_then(Value::as_array)
            .ok_or(invalid)?;
        if rows.len() > 512 {
            return Err(invalid);
        }
        let ids: std::collections::HashSet<String> = if mandatory {
            rows.iter()
                .filter(|row| row.as_object().is_some_and(needs_probe))
                .map(|row| {
                    row.get("format_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or(invalid)
                })
                .collect::<Result<_, _>>()?
        } else {
            let media = super::normalize_metadata(
                crate::domain::media::Platform::X,
                &serde_json::to_string(metadata).map_err(|_| invalid)?,
            )?;
            if media.formats.len() != rows.len() {
                return Err(AnalysisErrorCode::FormatUnavailable);
            }
            crate::domain::format_selection::unproven_quality_format_ids(&media)
        };
        if ids.is_empty() {
            if mandatory {
                mandatory = false;
                continue;
            }
            return Ok(());
        }
        if ids.iter().any(|id| probed.contains(id)) {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        if probed.len() + ids.len() > 16 {
            return Err(invalid);
        }
        let tool = resolver
            .resolve_verified(ToolName::Ffprobe)
            .map_err(super::errors::map_tool_error)?;
        let proxies = &context.network;
        let rows = metadata
            .get_mut("formats")
            .and_then(Value::as_array_mut)
            .ok_or(invalid)?;
        for row in rows
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .filter(|row| {
                row.get("format_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.contains(id))
            })
        {
            probed.insert(
                row.get("format_id")
                    .and_then(Value::as_str)
                    .ok_or(invalid)?
                    .to_owned(),
            );
            if cancellation.is_canceled() {
                return Err(AnalysisErrorCode::ProcessFailed);
            }
            let timeout = deadline
                .checked_duration_since(Instant::now())
                .filter(|n| !n.is_zero())
                .ok_or(AnalysisErrorCode::AnalysisTimeout)?
                .min(Duration::from_secs(15));
            let probe_deadline = Instant::now() + timeout;
            let raw = row
                .get("url")
                .and_then(Value::as_str)
                .filter(|raw| raw.len() <= 16_384 && !raw.chars().any(char::is_control))
                .ok_or(invalid)?;
            let url = Url::parse(raw).map_err(|_| invalid)?;
            crate::infrastructure::x::url_policy::validate_x_asset_url(
                url.as_str(),
                crate::infrastructure::x::url_policy::XAssetKind::Video,
            )?;
            let snapshot = match row
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("https")
            {
                "https" => None,
                "m3u8" | "m3u8_native" => {
                    let id = row
                        .get("format_id")
                        .and_then(Value::as_str)
                        .ok_or(invalid)?;
                    let snapshot = hls.get(id).ok_or(invalid)?;
                    snapshot.revalidate()?;
                    Some(snapshot)
                }
                _ => return Err(AnalysisErrorCode::UnsupportedMedia),
            };
            let local = snapshot
                .map(|snapshot| {
                    use crate::infrastructure::x::hls_local::{
                        materialize_hls, HlsMaterializationRequest, HlsReadMode,
                    };
                    materialize_hls(
                        HlsMaterializationRequest {
                            snapshot,
                            parent: working_dir,
                            mode: HlsReadMode::Probe,
                            network: context.network.clone(),
                            cancellation: cancellation.clone(),
                            deadline: Some(probe_deadline),
                        },
                        segments,
                    )
                })
                .transpose()
                .map_err(|error| error.analysis_code())?;
            let protocols = if local.is_some() {
                "file"
            } else {
                "https,http,tcp,tls,httpproxy"
            };
            let formats = if snapshot.is_some() {
                "hls,mpegts,mov,aac,webvtt"
            } else {
                "mov,matroska,webm"
            };
            let mut args: Vec<OsString> = [
                "-v",
                "error",
                "-protocol_whitelist",
                protocols,
                "-format_whitelist",
                formats,
                "-probesize",
                "8388608",
                "-analyzeduration",
                "3000000",
            ]
            .into_iter()
            .map(Into::into)
            .collect();
            if local.is_none() {
                args.extend([
                    "-max_redirects".into(),
                    "0".into(),
                    "-rw_timeout".into(),
                    "8000000".into(),
                    "-user_agent".into(),
                    "Mozilla/5.0".into(),
                    "-referer".into(),
                    "https://x.com/".into(),
                ]);
                if let Some(proxy) = proxies.https_proxy_for_host(url.host_str().ok_or(invalid)?) {
                    if proxy.scheme() != "http" {
                        return Err(AnalysisErrorCode::AnalysisUnavailable);
                    }
                    args.extend(["-http_proxy".into(), proxy.as_str().into()]);
                }
            }
            let input: OsString = match &local {
                Some(local) => {
                    local.revalidate(cancellation)?;
                    local.input().as_os_str().to_owned()
                }
                None => url.as_str().into(),
            };
            args.extend([
                "-show_entries".into(),
                ENTRIES.into(),
                "-of".into(),
                "json".into(),
                "-i".into(),
                input,
            ]);
            let mut retried = false;
            let exit = loop {
                if cancellation.is_canceled() {
                    return Err(AnalysisErrorCode::ProcessFailed);
                }
                let attempt_deadline = if retried {
                    deadline.min(Instant::now() + Duration::from_secs(15))
                } else {
                    probe_deadline
                };
                let remaining = attempt_deadline
                    .checked_duration_since(Instant::now())
                    .filter(|n| !n.is_zero())
                    .ok_or(AnalysisErrorCode::AnalysisTimeout)?;
                if process_attempts >= 16 {
                    return Err(AnalysisErrorCode::FormatUnavailable);
                }
                process_attempts += 1;
                let result = process.run(
                    ProcessRequest {
                        program: ProcessProgram::from_resolved_tool(
                            crate::application::tool_resolver::ResolvedTool {
                                name: tool.name,
                                version: tool.version.clone(),
                                path: tool.path.clone(),
                            },
                        ),
                        args: args.clone(),
                        working_dir: working_dir.to_owned(),
                        timeout: Some(remaining),
                        inactivity_timeout: None,
                        output_mode: OutputMode::Capture {
                            stdout_limit: LIMIT,
                            stderr_limit: LIMIT,
                        },
                    },
                    cancellation.clone(),
                    &Discard,
                );
                if cancellation.is_canceled() {
                    return Err(AnalysisErrorCode::ProcessFailed);
                }
                let error = match result {
                    Ok(exit) if exit.code == 0 => break exit,
                    Ok(exit) if local.is_none() => classify_remote_probe_exit(&exit.stderr),
                    Ok(exit) => super::errors::classify_nonzero_exit(&exit.stderr),
                    Err(error) => super::errors::map_process_error(error),
                };
                // One retry of the identical progressive source, never page
                // re-extraction or lower-quality fallback. Both attempts count
                // against the original deadline and shared process budget.
                if local.is_some()
                    || retried
                    || !matches!(
                        error,
                        AnalysisErrorCode::AnalysisTimeout | AnalysisErrorCode::NetworkFailed
                    )
                {
                    return Err(error);
                }
                retried = true;
            };
            if exit.stdout.len() > LIMIT {
                return Err(invalid);
            }
            let probe: Probe = serde_json::from_slice(&exit.stdout).map_err(|_| invalid)?;
            apply_probe(
                row,
                &probe,
                snapshot.is_some(),
                local
                    .as_ref()
                    .is_some_and(|input| input.has_probe_samples()),
            )?;
        }
        mandatory = false;
    }
}

fn classify_remote_probe_exit(stderr: &[u8]) -> AnalysisErrorCode {
    let classified = super::errors::classify_nonzero_exit(stderr);
    if classified != AnalysisErrorCode::Unknown {
        return classified;
    }
    // FFprobe's TLS backend reports this transport interruption differently
    // from yt-dlp. Only remote X probes may use the existing bounded retry;
    // never reinterpret a local-file I/O or a higher-priority HTTP denial.
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if !text.contains("certificate")
        && text.contains("[tls @")
        && text.contains("error in the pull function.")
        && text.contains("io error: i/o error")
    {
        AnalysisErrorCode::NetworkFailed
    } else {
        classified
    }
}

fn needs_probe(row: &Map<String, Value>) -> bool {
    row.get("has_drm").and_then(Value::as_bool) != Some(true)
        && matches!(
            row.get("ext").and_then(Value::as_str),
            Some("mp4" | "webm" | "mkv" | "mov" | "m4a")
        )
        && !["vcodec", "acodec"].iter().any(|key| {
            matches!(
                row.get(*key).and_then(Value::as_str),
                Some("images" | "storyboard")
            )
        })
        && (["vcodec", "acodec"].iter().any(|key| {
            matches!(
                row.get(*key).and_then(Value::as_str),
                None | Some("" | "unknown")
            )
        }) || (row.get("vcodec").and_then(Value::as_str) != Some("none")
            && ["width", "height", "fps"].iter().any(|key| {
                !row.get(*key)
                    .and_then(Value::as_f64)
                    .is_some_and(|n| n > 0.0)
            })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    streams: Vec<Stream>,
    format: Option<Format>,
    #[serde(default)]
    programs: Vec<Program>,
    #[serde(default)]
    stream_groups: Vec<Value>,
}
#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Stream {
    codec_type: String,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    color_transfer: Option<String>,
    bit_rate: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u8>,
    #[serde(default)]
    side_data_list: Vec<UnrequestedSideData>,
}
// With ENTRIES above FFprobe can still emit empty side-data objects. No
// unrequested side-data fields are trusted for selection or HDR declarations.
#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct UnrequestedSideData {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    streams: Vec<Stream>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Format {
    duration: Option<String>,
    size: Option<String>,
}

fn apply_probe(
    row: &mut Map<String, Value>,
    probe: &Probe,
    hls: bool,
    sampled: bool,
) -> Result<(), AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if probe.streams.is_empty()
        || probe.streams.len() > 16
        || probe.programs.len() > usize::from(hls)
        || probe.programs.iter().any(|p| p.streams != probe.streams)
        || probe.streams.iter().any(|s| s.side_data_list.len() > 16)
        || !probe.stream_groups.is_empty()
    {
        return Err(invalid);
    }
    let videos: Vec<_> = probe
        .streams
        .iter()
        .filter(|s| s.codec_type == "video")
        .collect();
    let audios: Vec<_> = probe
        .streams
        .iter()
        .filter(|s| s.codec_type == "audio")
        .collect();
    if videos.len() > 1 || audios.len() > 1 {
        return Err(invalid);
    }
    for (key, stream) in [("vcodec", videos.first()), ("acodec", audios.first())] {
        let actual = stream
            .and_then(|s| s.codec_name.as_deref())
            .unwrap_or("none");
        if stream.is_some()
            && (actual == "none"
                || actual.is_empty()
                || actual.len() > 64
                || !actual
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-')))
        {
            return Err(invalid);
        }
        if let Some(declared) = row
            .get(key)
            .and_then(Value::as_str)
            .filter(|c| !matches!(*c, "" | "unknown"))
        {
            let equivalent = declared == actual
                || if key == "vcodec" {
                    normalized_video_codec(Some(declared)).is_some()
                        && normalized_video_codec(Some(declared))
                            == normalized_video_codec(Some(actual))
                } else {
                    normalized_audio_codec(Some(declared)).is_some()
                        && normalized_audio_codec(Some(declared))
                            == normalized_audio_codec(Some(actual))
                };
            if !equivalent {
                return Err(AnalysisErrorCode::ContentChanged);
            }
        }
        row.insert(key.to_owned(), actual.into());
    }
    if let Some(video) = videos.first() {
        for (key, actual) in [("width", video.width), ("height", video.height)] {
            let actual = actual.filter(|n| (1..=16384).contains(n)).ok_or(invalid)?;
            if row
                .get(key)
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .is_some_and(|n| n != u64::from(actual))
            {
                return Err(AnalysisErrorCode::ContentChanged);
            }
            row.insert(key.into(), actual.into());
        }
        if let Some(rate) = video.avg_frame_rate.as_deref().and_then(frame_rate) {
            row.insert("fps".into(), rate.into());
        }
        let range = match video.color_transfer.as_deref() {
            Some("smpte2084") => Some("HDR10"),
            Some("arib-std-b67") => Some("HLG"),
            Some("bt709" | "smpte170m" | "bt2020-10" | "bt2020-12") => Some("SDR"),
            _ => None,
        };
        if let Some(range) = range {
            if !matches!(
                row.get("dynamic_range").and_then(Value::as_str),
                Some("SDR" | "HDR" | "HDR10" | "HDR10+" | "HDR12" | "HLG")
            ) {
                row.insert("dynamic_range".into(), range.into());
            }
        }
        // A localized HLS probe may contain only the beginning of a fragment.
        // Retain manifest-declared whole-rendition rates, never sample estimates.
        if !sampled {
            insert_rate(row, "vbr", video.bit_rate.as_deref(), 1_000_000.0);
        }
    }
    if let Some(audio) = audios.first() {
        if !sampled {
            insert_rate(row, "abr", audio.bit_rate.as_deref(), 100_000.0);
        }
        if let Some(n) = audio
            .sample_rate
            .as_deref()
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| (1..=768000).contains(n))
        {
            row.insert("asr".into(), n.into());
        }
        if let Some(n) = audio.channels.filter(|n| (1..=64).contains(n)) {
            row.insert("audio_channels".into(), n.into());
        }
    }
    if let Some(size) = probe
        .format
        .as_ref()
        .and_then(|f| f.size.as_deref())
        .and_then(|n| n.parse::<u64>().ok())
        .filter(|n| !hls && (1..=9_007_199_254_740_991).contains(n))
    {
        row.insert("filesize".into(), size.into());
    }
    // Duration belongs to the root object; stream selection does not need it.
    if probe
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .is_some_and(|n| {
            n.parse::<f64>().map_or(true, |n| {
                !n.is_finite() || !(0.0..=31_536_000.0).contains(&n)
            })
        })
    {
        return Err(invalid);
    }
    Ok(())
}
fn insert_rate(row: &mut Map<String, Value>, key: &str, text: Option<&str>, max: f64) {
    if let Some(rate) = text
        .and_then(|n| n.parse::<f64>().ok())
        .map(|n| n / 1000.0)
        .filter(|n| n.is_finite() && *n > 0.0 && *n <= max)
    {
        row.insert(key.into(), rate.into());
    }
}
struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _: ProcessStream, _: &[u8]) {}
}

#[cfg(test)]
mod sample_tests {
    use super::*;
    #[test]
    fn hls_sample_does_not_replace_declared_rates_size_or_duration() {
        let probe: Probe = serde_json::from_value(serde_json::json!({"streams":[
            {"codec_type":"video","codec_name":"h264","width":2160,"height":3240,"avg_frame_rate":"60/1","bit_rate":"999999999"},
            {"codec_type":"audio","codec_name":"aac","bit_rate":"8000","sample_rate":"48000","channels":2}
        ],"format":{"size":"1049481","duration":"3"}})).unwrap();
        for declared in [false, true] {
            let mut row = serde_json::json!({"duration":100.0})
                .as_object()
                .unwrap()
                .clone();
            if declared {
                row.insert("vbr".into(), 27234.49.into());
                row.insert("abr".into(), 128.into());
            }
            apply_probe(&mut row, &probe, true, true).unwrap();
            assert_eq!(
                row.get("vbr"),
                declared.then_some(&serde_json::json!(27234.49))
            );
            assert_eq!(row.get("abr"), declared.then_some(&serde_json::json!(128)));
            assert_eq!(row["duration"], 100.0);
            assert!(!row.contains_key("filesize"));
            assert_eq!(row["width"], 2160);
            assert_eq!(row["fps"], 60.0);
        }
        let mut progressive = Map::new();
        apply_probe(&mut progressive, &probe, false, false).unwrap();
        assert_eq!(progressive["vbr"], 999999.999);
        assert_eq!(progressive["abr"], 8.0);
        assert_eq!(progressive["filesize"], 1049481);
    }
    #[test]
    fn unsampled_hls_hydrates_rates_needed_to_compare_quality_contenders() {
        let probe: Probe=serde_json::from_value(serde_json::json!({"streams":[{"codec_type":"audio","codec_name":"aac","bit_rate":"128000","sample_rate":"48000","channels":2}],"format":{"size":"48000","duration":"3"}})).unwrap();
        let mut row=serde_json::json!({"format_id":"hls","ext":"m4a","protocol":"m3u8_native","vcodec":"none","acodec":"aac","asr":48000,"audio_channels":2}).as_object().unwrap().clone();
        apply_probe(&mut row, &probe, true, false).unwrap();
        assert_eq!(
            row["abr"], 128.0,
            "unmodified TS/AAC/full-fragment path must still hydrate bitrate"
        );
        assert!(!row.contains_key("filesize"));
        let mut contender = row.clone();
        contender.insert("format_id".into(), "other".into());
        contender.insert("abr".into(), 64.into());
        let metadata = serde_json::json!({"id":"x","title":"fixture","webpage_url":"https://x.com/author/status/2110906404831912440","formats":[row,contender]});
        let media = super::super::normalize_metadata(
            crate::domain::media::Platform::X,
            &metadata.to_string(),
        )
        .unwrap();
        assert!(crate::domain::format_selection::unproven_quality_format_ids(&media).is_empty());
    }
}
