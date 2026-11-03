//! Bounded direct-media fact probes. Never relabel unknown HDR as SDR.
use crate::{
    application::{
        bilibili_source::{BilibiliPageEvidence, BilibiliRequestContext},
        process::{OutputMode, ProcessProgram, ProcessRequest, ProcessSupervisor},
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        managed_tools::ToolName,
        media::AnalysisErrorCode,
        output_policy::{normalized_audio_codec, normalized_video_codec},
    },
    infrastructure::{
        bilibili::{
            json::UniqueValue,
            url_policy::{validate_bilibili_asset_url, BilibiliAssetKind},
        },
        ffprobe::stream_properties::{
            frame_rate, AudioStreamProperties, ProbeStream, VideoStreamProperties,
        },
    },
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::Path,
    time::Duration,
};

type Properties = (Option<VideoStreamProperties>, Option<AudioStreamProperties>);
const ENTRIES:&str="format=format_name,duration,size:stream=codec_type,codec_name,profile,width,height,avg_frame_rate,pix_fmt,color_range,color_space,color_transfer,color_primaries,channels,sample_rate,channel_layout,bit_rate:stream_side_data";

#[allow(clippy::too_many_arguments)]
pub(super) fn hydrate<R: ManagedToolResolver, P: ProcessSupervisor>(
    raw: &mut Value,
    evidence: &BilibiliPageEvidence,
    forced: Option<&[String]>,
    resolver: &R,
    process: &P,
    working: &Path,
    context: &BilibiliRequestContext,
    properties: &mut HashMap<String, Properties>,
) -> Result<(), AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let mut probed = HashSet::new();
    loop {
        let rows = raw["formats"].as_array().ok_or(invalid)?;
        let mut ids: HashSet<String> = rows
            .iter()
            .filter(|row| {
                needs_probe(row)
                    // API FPS is an estimate, not an immutable source fact.
                    // Prove every selectable video before ranking/freezing;
                    // never relax the existing 16-call / 120-second budget.
                    || (forced.is_none() && codec(row, "vcodec") != Some("none")
                        && row["format_id"].as_str().is_some_and(|id| !probed.contains(id)))
                    || forced.is_some_and(|ids| {
                        ids.iter().any(|id| row["format_id"].as_str() == Some(id))
                    })
            })
            .filter_map(|row| row["format_id"].as_str().map(str::to_owned))
            .collect();
        // After all structural facts can be projected, prove quality ties rather
        // than allowing a missing bitrate/range to lose to a lower candidate.
        if ids.is_empty() && forced.is_none() {
            ids = crate::domain::format_selection::unproven_quality_format_ids(
                &super::bilibili::project(raw)?,
            );
        }
        if ids.is_empty() {
            return Ok(());
        }
        if ids.iter().any(|id| probed.contains(id)) || probed.len() + ids.len() > 16 {
            return Err(AnalysisErrorCode::FormatUnavailable);
        }
        for row in raw["formats"]
            .as_array_mut()
            .ok_or(invalid)?
            .iter_mut()
            .filter(|row| row["format_id"].as_str().is_some_and(|id| ids.contains(id)))
        {
            let id = row["format_id"].as_str().ok_or(invalid)?.to_owned();
            let url = validate_bilibili_asset_url(
                row["url"].as_str().ok_or(invalid)?,
                BilibiliAssetKind::Media,
            )?;
            let tool = resolver
                .resolve_verified(ToolName::Ffprobe)
                .map_err(super::errors::map_tool_error)?;
            let network = context.network();
            let proxy = network.https_proxy_for_host(url.host_str().ok_or(invalid)?);
            if proxy.is_some_and(|p| p.scheme() != "http") {
                return Err(AnalysisErrorCode::AnalysisUnavailable);
            }
            let args: Vec<OsString> = [
                "-v",
                "error",
                "-max_redirects",
                "0",
                "-protocol_whitelist",
                "https,http,tcp,tls,httpproxy",
                "-format_whitelist",
                "mov,matroska,webm,flac,aac,mp3,ogg",
                "-rw_timeout",
                "8000000",
                "-http_proxy",
                proxy.map(|p| p.as_str()).unwrap_or(""),
                "-user_agent",
                "Mozilla/5.0",
                "-referer",
                evidence.source().url(),
                "-probesize",
                "8388608",
                "-analyzeduration",
                "3000000",
                "-show_entries",
                ENTRIES,
                "-of",
                "json",
                "-i",
                url.as_str(),
            ]
            .into_iter()
            .map(Into::into)
            .collect();
            let exit = process
                .run(
                    ProcessRequest {
                        program: ProcessProgram::from_resolved_tool(tool)
                            .with_network_profile(context.network()),
                        args,
                        working_dir: working.into(),
                        timeout: Some(context.remaining(Duration::from_secs(15))?),
                        inactivity_timeout: None,
                        output_mode: OutputMode::Capture {
                            stdout_limit: 65536,
                            stderr_limit: 65536,
                        },
                    },
                    context.cancellation(),
                    &super::bilibili::Discard,
                )
                .map_err(super::errors::map_process_error)?;
            context.remaining(Duration::from_secs(120))?;
            if exit.stdout.len() > 65536 || exit.stderr.len() > 65536 {
                return Err(invalid);
            }
            if exit.code != 0 {
                return Err(super::errors::classify_nonzero_exit(&exit.stderr));
            }
            let probe = serde_json::from_slice::<UniqueValue>(&exit.stdout)
                .map_err(|_| invalid)?
                .0;
            let result = apply(row, &probe, evidence.identity().duration_seconds)?;
            properties.insert(id.clone(), result);
            probed.insert(id);
        }
        if forced.is_some() {
            return Ok(());
        }
        // A probe without evidence for a mandatory field must not spin or use
        // an invented default. Unknown non-ranking facts may remain unknown.
        let rows = raw["formats"].as_array().ok_or(invalid)?;
        if rows.iter().any(|row| {
            needs_probe(row)
                && row["format_id"]
                    .as_str()
                    .is_some_and(|id| probed.contains(id))
        }) {
            // Unknown range alone is allowed if it cannot change the winner.
            let actual = super::bilibili::project(raw)?;
            let unresolved = crate::domain::format_selection::unproven_quality_format_ids(&actual);
            if unresolved.iter().any(|id| probed.contains(id)) {
                return Err(AnalysisErrorCode::FormatUnavailable);
            }
            return Ok(());
        }
    }
}
fn codec<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}
fn needs_probe(row: &Value) -> bool {
    let (v, a) = (codec(row, "vcodec"), codec(row, "acodec"));
    if v.is_none() || a.is_none() {
        return true;
    }
    (v != Some("none")
        && (normalized_video_codec(v).is_none()
            || ["width", "height", "fps", "dynamic_range"]
                .iter()
                .any(|k| row.get(*k).is_none_or(Value::is_null))))
        || (a != Some("none")
            && (normalized_audio_codec(a).is_none()
                || ["abr", "asr", "audio_channels"]
                    .iter()
                    .any(|k| row.get(*k).is_none_or(Value::is_null))))
}
fn apply(
    row: &mut Value,
    probe: &Value,
    full_duration: f64,
) -> Result<Properties, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if let Some(duration) = probe
        .get("format")
        .and_then(|v| v.get("duration"))
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| v.as_f64())
        })
    {
        if !duration.is_finite()
            || duration <= 0.0
            || (duration - full_duration).abs() > 2.0_f64.max(full_duration * 0.01)
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
    }
    let streams = probe["streams"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 2)
        .ok_or(invalid)?;
    let (mut video, mut audio) = (None, None);
    let mut video_rate = None;
    let mut audio_rate = None;
    for stream in streams {
        let mut stream = stream.clone();
        let rate = stream
            .get("bit_rate")
            .and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| v.as_f64())
            })
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v / 1000.0);
        stream.as_object_mut().ok_or(invalid)?.remove("bit_rate");
        let parsed: ProbeStream = serde_json::from_value(stream).map_err(|_| invalid)?;
        match parsed.codec_type.as_str() {
            "video" if video.is_none() => {
                video = Some(parsed.video_properties().map_err(|_| invalid)?);
                video_rate = rate;
            }
            "audio" if audio.is_none() => {
                audio = Some(parsed.audio_properties().map_err(|_| invalid)?);
                audio_rate = rate;
            }
            _ => return Err(invalid),
        }
    }
    if codec(row, "vcodec") == Some("none") && video.is_some()
        || codec(row, "acodec") == Some("none") && audio.is_some()
    {
        return Err(invalid);
    }
    if codec(row, "vcodec").is_some_and(|s| s != "none") && video.is_none()
        || codec(row, "acodec").is_some_and(|s| s != "none") && audio.is_none()
    {
        return Err(invalid);
    }
    if let Some(v) = &video {
        if codec(row, "vcodec")
            .filter(|s| *s != "none")
            .is_some_and(|s| normalized_video_codec(Some(s)) != Some(v.codec.as_str()))
            || row["width"]
                .as_u64()
                .is_some_and(|n| n != u64::from(v.width))
            || row["height"]
                .as_u64()
                .is_some_and(|n| n != u64::from(v.height))
        {
            return Err(invalid);
        }
        let range = dynamic_range(v);
        if row["dynamic_range"]
            .as_str()
            .zip(range)
            .is_some_and(|(known, actual)| known != actual)
        {
            return Err(invalid);
        }
        if row["vcodec"].is_null() {
            row["vcodec"] = v.codec.clone().into();
        }
        put_missing(row, "width", v.width.into());
        put_missing(row, "height", v.height.into());
        // The declaration is kept separately for pre-probe refresh checks.
        // Selection/UI and the frozen plan use the actual original stream rate.
        row["fps"] = frame_rate(&v.frame_rate).ok_or(invalid)?.into();
        if let Some(range) = range {
            put_missing(row, "dynamic_range", range.into());
        }
        if let Some(rate) = video_rate {
            put_missing(row, "vbr", rate.into());
        }
    } else {
        row["vcodec"] = "none".into();
    }
    if let Some(a) = &audio {
        if codec(row, "acodec")
            .filter(|s| *s != "none")
            .is_some_and(|s| normalized_audio_codec(Some(s)) != Some(a.codec))
            || row["asr"]
                .as_u64()
                .is_some_and(|n| n != u64::from(a.sample_rate_hz))
            || row["audio_channels"]
                .as_u64()
                .is_some_and(|n| n != u64::from(a.channels))
        {
            return Err(invalid);
        }
        if row["acodec"].is_null() {
            row["acodec"] = a.codec.as_str().into();
        }
        put_missing(row, "asr", a.sample_rate_hz.into());
        put_missing(row, "audio_channels", a.channels.into());
        if let Some(rate) = audio_rate {
            put_missing(row, "abr", rate.into());
        }
    } else {
        row["acodec"] = "none".into();
    }
    Ok((video, audio))
}
fn put_missing(row: &mut Value, key: &str, value: Value) {
    if row.get(key).is_none_or(Value::is_null) {
        row[key] = value;
    }
}
fn dynamic_range(video: &VideoStreamProperties) -> Option<&'static str> {
    if video.hdr_side_data.iter().any(|data| {
        data.get("side_data_type")
            .and_then(Value::as_str)
            .is_some_and(|s| s.to_ascii_lowercase().contains("dovi"))
    }) {
        return Some("DV");
    }
    match video.color_transfer.as_deref() {
        Some("smpte2084") => Some("HDR10"),
        Some("arib-std-b67") => Some("HLG"),
        Some("bt709" | "bt470m" | "bt470bg" | "smpte170m" | "smpte240m" | "iec61966-2-1") => {
            Some("SDR")
        }
        _ => None,
    }
}
