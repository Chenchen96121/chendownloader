//! Pinned ordinary-video extraction with an independent native page witness.
use crate::{
    application::{
        bilibili_source::{
            BilibiliAnalysisScope, BilibiliAnalyzedSource, BilibiliAnalyzer, BilibiliPageEvidence,
            BilibiliPageLoader, BilibiliRequestContext, BilibiliStreamSource,
        },
        process::{
            OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest, ProcessStream,
            ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        bilibili::BilibiliVideoRequest,
        managed_tools::ToolName,
        media::{AnalysisErrorCode, MediaInfo, Platform},
    },
    infrastructure::bilibili::{
        json::UniqueValue,
        url_policy::{validate_bilibili_asset_url, BilibiliAssetKind},
    },
};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    time::Duration,
};

const STDOUT_LIMIT: usize = 16 * 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

pub struct YtDlpBilibiliAnalyzer<R, P, W> {
    resolver: R,
    process: P,
    pages: W,
}
impl<R, P, W> YtDlpBilibiliAnalyzer<R, P, W> {
    pub fn new(resolver: R, process: P, pages: W) -> Self {
        Self {
            resolver,
            process,
            pages,
        }
    }
}
pub struct VerifiedBilibiliMetadata {
    raw: Value,
    evidence: BilibiliPageEvidence,
}

pub fn match_bilibili_metadata(
    evidence: &BilibiliPageEvidence,
    raw: &Value,
) -> Result<VerifiedBilibiliMetadata, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let root = raw.as_object().ok_or(invalid)?;
    let identity = evidence.identity();
    if root
        .get("_type")
        .is_some_and(|v| v.as_str() != Some("video"))
        || root.contains_key("entries")
        || root.get("is_live").and_then(Value::as_bool) == Some(true)
        || root.get("has_drm").and_then(Value::as_bool) == Some(true)
        || matches!(
            root.get("live_status").and_then(Value::as_str),
            Some("is_live" | "is_upcoming" | "post_live")
        )
    {
        return Err(AnalysisErrorCode::UnsupportedMedia);
    }
    if root.get("extractor_key").and_then(Value::as_str) != Some("BiliBili") {
        return Err(AnalysisErrorCode::UnsupportedMedia);
    }
    let expected_id = format!("{}_p{}", identity.bvid, identity.part);
    let archive = format!("bilibili {}_part{}", identity.aid, identity.part);
    if root.get("id").and_then(Value::as_str) != Some(&expected_id)
        || root
            .get("_old_archive_ids")
            .and_then(Value::as_array)
            .is_none_or(|ids| ids.len() != 1 || ids[0].as_str() != Some(&archive))
    {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    for flag in [
        "is_trial",
        "is_preview",
        "is_pay",
        "is_paid",
        "is_upower_exclusive",
    ] {
        if root
            .get(flag)
            .is_some_and(|v| !(v.as_bool() == Some(false) || v.as_u64() == Some(0)))
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
    }
    let duration = root
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or(invalid)?;
    if (duration - identity.duration_seconds).abs() > 2.0_f64.max(identity.duration_seconds * 0.01)
    {
        return Err(AnalysisErrorCode::UnsupportedMedia);
    }
    let formats = root
        .get("formats")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty() && v.len() <= 512)
        .ok_or(invalid)?;
    let mut ids = HashSet::new();
    let mut accepted = Vec::new();
    for format in formats {
        let mut normalized = format.clone();
        let row = normalized.as_object_mut().ok_or(invalid)?;
        // The fixed extractor emits vbr=0 for no-video audio streams and
        // abr=0 for no-audio video streams. Only that exact absent-role sentinel
        // is omitted; present-role zero or contradictory positive values fail.
        for (role, rate) in [("vcodec", "vbr"), ("acodec", "abr")] {
            if row.get(role).and_then(Value::as_str) == Some("none") {
                if let Some(value) = row.get(rate).filter(|value| !value.is_null()) {
                    if value.as_f64() != Some(0.0) {
                        return Err(invalid);
                    }
                    row.remove(rate);
                }
            }
        }
        if row.get("has_drm").and_then(Value::as_bool) == Some(true)
            || ["vcodec", "acodec"].iter().any(|k| {
                matches!(
                    row.get(*k).and_then(Value::as_str),
                    Some("images" | "storyboard")
                )
            })
        {
            continue;
        }
        let id = row
            .get("format_id")
            .and_then(Value::as_str)
            .filter(|id| safe_token(id, 64))
            .ok_or(invalid)?;
        if !ids.insert(id.to_owned()) {
            return Err(invalid);
        }
        if row.contains_key("fragments")
            || row.contains_key("entries")
            || row
                .get("protocol")
                .is_some_and(|v| v.as_str() != Some("https"))
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        let url = validate_bilibili_asset_url(
            row.get("url").and_then(Value::as_str).ok_or(invalid)?,
            BilibiliAssetKind::Media,
        )?;
        let ext = row.get("ext").and_then(Value::as_str).ok_or(invalid)?;
        if !matches!(
            ext,
            "mp4" | "m4a" | "webm" | "mkv" | "flac" | "aac" | "mp3" | "ogg" | "opus"
        ) || url.path().to_ascii_lowercase().ends_with(".m3u8")
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        for codec in ["vcodec", "acodec"] {
            if row
                .get(codec)
                .is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(|s| safe_token(s, 64)))
            {
                return Err(invalid);
            }
        }
        for (key, maximum, integer) in [
            ("width", 16384.0, true),
            ("height", 16384.0, true),
            ("fps", 240.0, false),
            ("tbr", 1_000_000.0, false),
            ("vbr", 1_000_000.0, false),
            ("abr", 100_000.0, false),
            ("asr", 768000.0, true),
            ("audio_channels", 64.0, true),
            ("filesize", 9_007_199_254_740_991.0, true),
            ("filesize_approx", 9_007_199_254_740_991.0, true),
        ] {
            if row.get(key).is_some_and(|v| {
                !v.is_null()
                    && (!v
                        .as_f64()
                        .is_some_and(|n| n.is_finite() && n > 0.0 && n <= maximum)
                        || (integer && v.as_u64().is_none()))
            }) {
                return Err(invalid);
            }
        }
        accepted.push(normalized);
    }
    if accepted.is_empty() {
        return Err(AnalysisErrorCode::FormatUnavailable);
    }
    let mut raw = raw.clone();
    raw["formats"] = Value::Array(accepted);
    Ok(VerifiedBilibiliMetadata {
        raw,
        evidence: evidence.clone(),
    })
}

impl<R, P, W> BilibiliAnalyzer for YtDlpBilibiliAnalyzer<R, P, W>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
    W: BilibiliPageLoader,
{
    fn analyze(
        &self,
        request: &BilibiliVideoRequest,
        scope: BilibiliAnalysisScope,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliAnalyzedSource, AnalysisErrorCode> {
        context.remaining(Duration::from_secs(120))?;
        let evidence = self.pages.inspect(request, context)?;
        if context
            .expected_identity()
            .is_some_and(|old| old != evidence.identity())
        {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        let tool = self
            .resolver
            .resolve_verified(ToolName::YtDlp)
            .map_err(super::errors::map_tool_error)?;
        if tool.version != "2026.08.19" {
            return Err(AnalysisErrorCode::ToolIntegrityFailed);
        }
        let working = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let mut args: Vec<OsString> = [
            "--ignore-config",
            "--no-remote-components",
            "--no-plugin-dirs",
            "--simulate",
            "--dump-single-json",
            "--write-pages",
            "--no-playlist",
            "--quiet",
            "--verbose",
            "--no-progress",
            "--color",
            "never",
            "--encoding",
            "utf-8",
            "--no-cookies",
            "--no-cookies-from-browser",
            "--no-update",
            "--no-cache-dir",
            "--no-check-formats",
            "--socket-timeout",
            "15",
            "--retries",
            "0",
            "--extractor-retries",
            "0",
            "--use-extractors",
            "BiliBili$",
            "--",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(evidence.source().url().into());
        let exit = super::bilibili_witness::run_with_budget(
            working.path(),
            context,
            |process_cancellation| {
                self.process
                    .run(
                        ProcessRequest {
                            program: ProcessProgram::from_resolved_tool(tool)
                                .with_network_profile(context.network()),
                            args,
                            working_dir: working.path().into(),
                            timeout: Some(context.remaining(Duration::from_secs(45))?),
                            inactivity_timeout: None,
                            output_mode: OutputMode::Capture {
                                stdout_limit: STDOUT_LIMIT,
                                stderr_limit: STDERR_LIMIT,
                            },
                        },
                        process_cancellation,
                        &Discard,
                    )
                    .map_err(super::errors::map_process_error)
            },
        )?;
        context.remaining(Duration::from_secs(120))?;
        if exit.stdout.len() > STDOUT_LIMIT || exit.stderr.len() > STDERR_LIMIT {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        if exit.code != 0 {
            return Err(super::errors::classify_nonzero_exit(&exit.stderr));
        }
        let stderr =
            std::str::from_utf8(&exit.stderr).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        let notice = stderr
            .lines()
            .filter(|line| !line.trim_start().starts_with("[debug]"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        if [
            "试看",
            "preview video",
            "only preview",
            "only the preview",
            "dropping incompatible flv",
            "trial video",
            "trial version",
        ]
        .iter()
        .any(|s| notice.contains(s))
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
        // Quiet+verbose routes diagnostic screens to private stderr; stdout is
        // solely JSON. Never persist/emit this diagnostic stream or raw JSON.
        let raw = serde_json::from_slice::<UniqueValue>(&exit.stdout)
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?
            .0;
        let mut verified = match_bilibili_metadata(&evidence, &raw)?;
        let force = match scope {
            BilibiliAnalysisScope::Discover => None,
            BilibiliAnalysisScope::RefreshSelected(ids) => {
                let unique: HashSet<_> = ids.iter().collect();
                if ids.is_empty() || ids.len() > 2 || unique.len() != ids.len() {
                    return Err(AnalysisErrorCode::InvalidMetadata);
                }
                let rows = verified.raw["formats"]
                    .as_array_mut()
                    .ok_or(AnalysisErrorCode::InvalidMetadata)?;
                rows.retain(|row| {
                    row["format_id"]
                        .as_str()
                        .is_some_and(|id| unique.iter().any(|wanted| wanted.as_str() == id))
                });
                if rows.len() != ids.len() {
                    return Err(AnalysisErrorCode::ContentChanged);
                }
                Some(ids)
            }
        };
        project_bandwidth(&mut verified.raw)?;
        validate_expected_declarations(context.expected_formats(), &verified.raw)?;
        // Unlike a second HTTP request, these are the exact response bytes
        // consumed by the pinned extractor, including its selected CID.
        super::bilibili_witness::verify_consumed_page(working.path(), request, &evidence)?;
        let declared_fps: HashMap<String, Option<f64>> = verified.raw["formats"]
            .as_array()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?
            .iter()
            .map(|row| {
                Ok((
                    row["format_id"]
                        .as_str()
                        .ok_or(AnalysisErrorCode::InvalidMetadata)?
                        .to_owned(),
                    row["fps"].as_f64(),
                ))
            })
            .collect::<Result<_, AnalysisErrorCode>>()?;
        let mut properties = HashMap::new();
        super::bilibili_probe::hydrate(
            &mut verified.raw,
            &verified.evidence,
            force.as_deref(),
            &self.resolver,
            &self.process,
            working.path(),
            context,
            &mut properties,
        )?;
        let media = project(&verified.raw)?;
        let mut streams = Vec::new();
        for format in &media.formats {
            let row = verified.raw["formats"]
                .as_array()
                .ok_or(AnalysisErrorCode::InvalidMetadata)?
                .iter()
                .find(|row| row["format_id"].as_str() == Some(&format.id))
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            let url = validate_bilibili_asset_url(
                row["url"]
                    .as_str()
                    .ok_or(AnalysisErrorCode::InvalidMetadata)?,
                BilibiliAssetKind::Media,
            )?;
            let (video, audio) = properties.remove(&format.id).unwrap_or_default();
            streams.push(BilibiliStreamSource {
                declared_fps: declared_fps.get(&format.id).copied().flatten(),
                format: format.clone(),
                url,
                video,
                audio,
            });
        }
        context.remaining(Duration::from_secs(120))?;
        Ok(BilibiliAnalyzedSource {
            request: request.clone(),
            evidence,
            media,
            streams,
        })
    }
}

fn safe_token(s: &str, maximum: usize) -> bool {
    !s.is_empty()
        && s.len() <= maximum
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}
fn validate_expected_declarations(
    expected: &[crate::domain::media::MediaFormat],
    raw: &Value,
) -> Result<(), AnalysisErrorCode> {
    use crate::domain::output_policy::{normalized_audio_codec, normalized_video_codec};
    for old in expected {
        let row = raw["formats"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["format_id"].as_str() == Some(&old.id))
            })
            .ok_or(AnalysisErrorCode::ContentChanged)?;
        let video = row["vcodec"].as_str();
        let audio = row["acodec"].as_str();
        if video.is_some_and(|s| {
            (s != "none") != old.video_codec.is_some()
                || (s != "none"
                    && normalized_video_codec(Some(s))
                        != normalized_video_codec(old.video_codec.as_deref()))
        }) || audio.is_some_and(|s| {
            (s != "none") != old.audio_codec.is_some()
                || (s != "none"
                    && normalized_audio_codec(Some(s))
                        != normalized_audio_codec(old.audio_codec.as_deref()))
        }) || row["ext"].as_str().is_some_and(|ext| ext != old.container)
            || row["dynamic_range"]
                .as_str()
                .zip(old.dynamic_range.as_deref())
                .is_some_and(|(new, old)| new != old)
        {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        for (key, known) in [
            ("width", old.width.map(f64::from)),
            ("height", old.height.map(f64::from)),
            ("fps", old.fps),
            ("vbr", old.video_bitrate_kbps),
            ("abr", old.audio_bitrate_kbps),
            ("asr", old.audio_sample_rate_hz.map(f64::from)),
            ("audio_channels", old.audio_channels.map(f64::from)),
        ] {
            if row[key]
                .as_f64()
                .zip(known)
                .is_some_and(|(new, old)| new != old)
            {
                return Err(AnalysisErrorCode::ContentChanged);
            }
        }
        if let Some(size) = old.file_size {
            if row["filesize"]
                .as_u64()
                .or_else(|| row["filesize_approx"].as_u64())
                .is_some_and(|n| n != size.bytes)
            {
                return Err(AnalysisErrorCode::ContentChanged);
            }
        }
    }
    Ok(())
}
fn project_bandwidth(raw: &mut Value) -> Result<(), AnalysisErrorCode> {
    for row in raw["formats"]
        .as_array_mut()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?
    {
        let video = row["vcodec"].as_str().is_some_and(|s| s != "none");
        let audio = row["acodec"].as_str().is_some_and(|s| s != "none");
        let key = match (video, audio) {
            (true, false) => Some("vbr"),
            (false, true) => Some("abr"),
            _ => None,
        };
        if let Some(key) = key {
            if row.get(key).is_none_or(Value::is_null)
                && row.get("tbr").is_some_and(Value::is_number)
            {
                row[key] = row["tbr"].clone();
            }
        }
    }
    Ok(())
}
pub(super) fn project(raw: &Value) -> Result<MediaInfo, AnalysisErrorCode> {
    let mut projected = Map::new();
    for key in ["id", "title", "uploader", "duration"] {
        if let Some(v) = raw.get(key) {
            projected.insert(key.into(), v.clone());
        }
    }
    let fields = [
        "format_id",
        "ext",
        "vcodec",
        "acodec",
        "width",
        "height",
        "fps",
        "vbr",
        "abr",
        "asr",
        "audio_channels",
        "dynamic_range",
        "filesize",
        "filesize_approx",
    ];
    let formats = raw["formats"]
        .as_array()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let rows = formats
        .iter()
        .map(|row| {
            let mut safe = Map::new();
            for key in fields {
                if let Some(v) = row.get(key) {
                    safe.insert(key.into(), v.clone());
                }
            }
            Value::Object(safe)
        })
        .collect();
    projected.insert("formats".into(), Value::Array(rows));
    let text = serde_json::to_string(&projected).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let media = super::normalize_metadata(Platform::Bilibili, &text)?;
    if media.formats.len() != formats.len() {
        return Err(AnalysisErrorCode::FormatUnavailable);
    }
    Ok(media)
}
pub(super) struct Discard;
impl ProcessEventSink for Discard {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
}
