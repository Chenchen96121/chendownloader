//! Instagram extractors can omit codec facts. Probe approved native-only CDN
//! URLs before selection; never invent codecs or silently prefer a lower stream.
use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use crate::{
    application::{
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest,
            ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        managed_tools::ToolName,
        media::AnalysisErrorCode,
        output_policy::{normalized_audio_codec, normalized_video_codec},
    },
    infrastructure::{ffprobe::stream_properties::frame_rate, system_proxy::SystemProxies},
};

const ENTRIES: &str = "format=duration,size:stream=codec_type,codec_name,width,height,avg_frame_rate,color_transfer,bit_rate,sample_rate,channels";
const LIMIT: usize = 65_536;

pub(super) fn hydrate_formats<R: ManagedToolResolver, P: ProcessSupervisor>(
    metadata: &mut Value,
    resolver: &R,
    process: &P,
    working_dir: &Path,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<(), AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let rows = metadata
        .get_mut("formats")
        .and_then(Value::as_array_mut)
        .ok_or(invalid)?;
    if rows.len() > 512 {
        return Err(invalid);
    }
    let candidates = rows
        .iter()
        .filter(|row| row.as_object().is_some_and(needs_probe))
        .count();
    if candidates == 0 {
        return Ok(());
    }
    if candidates > 16 {
        return Err(invalid);
    }
    let tool = resolver
        .resolve_verified(ToolName::Ffprobe)
        .map_err(super::errors::map_tool_error)?;
    let proxies = SystemProxies::current();
    for row in rows
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .filter(|row| needs_probe(row))
    {
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .filter(|n| !n.is_zero())
            .ok_or(AnalysisErrorCode::AnalysisTimeout)?
            .min(Duration::from_secs(15));
        let raw = row
            .get("url")
            .and_then(Value::as_str)
            .filter(|raw| raw.len() <= 16_384 && !raw.chars().any(char::is_control))
            .ok_or(invalid)?;
        let url = Url::parse(raw).map_err(|_| invalid)?;
        super::super::instagram_web::validate_asset_url(&url)?;
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-protocol_whitelist",
            "https,http,tcp,tls,httpproxy",
            "-format_whitelist",
            "mov,matroska,webm",
            "-max_redirects",
            "0",
            "-rw_timeout",
            "8000000",
            "-probesize",
            "8388608",
            "-analyzeduration",
            "3000000",
            "-user_agent",
            "Mozilla/5.0",
            "-referer",
            "https://www.instagram.com/",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        if let Some(proxy) = proxies.https_for_host(url.host_str().ok_or(invalid)?) {
            if proxy.scheme() != "http" {
                return Err(AnalysisErrorCode::AnalysisUnavailable);
            }
            args.extend(["-http_proxy".into(), proxy.as_str().into()]);
        }
        args.extend([
            "-show_entries".into(),
            ENTRIES.into(),
            "-of".into(),
            "json".into(),
            "-i".into(),
            url.as_str().into(),
        ]);
        let exit = process
            .run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_resolved_tool(
                        crate::application::tool_resolver::ResolvedTool {
                            name: tool.name,
                            version: tool.version.clone(),
                            path: tool.path.clone(),
                        },
                    ),
                    args,
                    working_dir: working_dir.to_owned(),
                    timeout: Some(timeout),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: LIMIT,
                        stderr_limit: LIMIT,
                    },
                },
                cancellation.clone(),
                &Discard,
            )
            .map_err(super::errors::map_process_error)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        if exit.code != 0 {
            return Err(super::errors::classify_nonzero_exit(&exit.stderr));
        }
        if exit.stdout.len() > LIMIT {
            return Err(invalid);
        }
        let probe: Probe = serde_json::from_slice(&exit.stdout).map_err(|_| invalid)?;
        apply_probe(row, &probe)?;
    }
    Ok(())
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
        && ["vcodec", "acodec"].iter().any(|key| {
            matches!(
                row.get(*key).and_then(Value::as_str),
                None | Some("" | "unknown")
            )
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    streams: Vec<Stream>,
    format: Option<Format>,
    #[serde(default)]
    programs: Vec<Value>,
    #[serde(default)]
    stream_groups: Vec<Value>,
}
#[derive(Deserialize)]
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
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Format {
    duration: Option<String>,
    size: Option<String>,
}

fn apply_probe(row: &mut Map<String, Value>, probe: &Probe) -> Result<(), AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if probe.streams.is_empty()
        || probe.streams.len() > 16
        || !probe.programs.is_empty()
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
            row.entry("dynamic_range").or_insert(range.into());
        }
        insert_rate(row, "vbr", video.bit_rate.as_deref(), 1_000_000.0);
    }
    if let Some(audio) = audios.first() {
        insert_rate(row, "abr", audio.bit_rate.as_deref(), 100_000.0);
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
        .filter(|n| (1..=9_007_199_254_740_991).contains(n))
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
