use std::{
    collections::HashSet,
    ffi::OsString,
    time::{Duration, Instant},
};

use serde_json::{Map, Value};

use crate::{
    application::{
        instagram_video::{
            resolve_instagram_video, InstagramVideoSelector, ResolvedInstagramVideo,
        },
        media_url::ValidatedMediaUrl,
        process::{
            CancellationToken, OutputMode, ProcessEventSink, ProcessProgram, ProcessRequest,
            ProcessStream, ProcessSupervisor,
        },
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        instagram::InstagramUrlKind,
        managed_tools::ToolName,
        media::{AnalysisErrorCode, FileSize, MediaFormat, MediaInfo, Platform, StreamKind},
    },
};

use super::errors;

pub(super) const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
const INSTAGRAM_TIMEOUT: Duration = Duration::from_secs(45);
const INSTAGRAM_STDOUT_LIMIT: usize = 1_048_576;
const INSTAGRAM_STDERR_LIMIT: usize = 65_536;
const INSTAGRAM_ID_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub struct YtDlpInstagramVideoAnalyzer<R, P> {
    resolver: R,
    process: P,
}

impl<R, P> YtDlpInstagramVideoAnalyzer<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}

impl<R, P> YtDlpInstagramVideoAnalyzer<R, P>
where
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    pub fn analyze_reel(
        &self,
        source: &ValidatedMediaUrl,
        compatibility_mode: bool,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        self.analyze(
            source,
            InstagramVideoSelector::reel(),
            compatibility_mode,
            cancellation,
        )
    }

    pub fn analyze_post_item(
        &self,
        source: &ValidatedMediaUrl,
        ordinal: u16,
        expected_media_id: &str,
        compatibility_mode: bool,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        self.analyze(
            source,
            InstagramVideoSelector::post_item(ordinal, expected_media_id)?,
            compatibility_mode,
            cancellation,
        )
    }

    fn analyze(
        &self,
        source: &ValidatedMediaUrl,
        selector: InstagramVideoSelector,
        compatibility_mode: bool,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedInstagramVideo, AnalysisErrorCode> {
        validate_selector_source(source, &selector)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let yt_dlp = self
            .resolver
            .resolve_verified(ToolName::YtDlp)
            .map_err(errors::map_tool_error)?;
        let deno = self
            .resolver
            .resolve_verified(ToolName::Deno)
            .map_err(errors::map_tool_error)?;
        let working = tempfile::tempdir().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let mut deno_runtime = OsString::from("deno:");
        deno_runtime.push(&deno.path);
        let mut args = vec![
            OsString::from("--ignore-config"),
            OsString::from("--no-remote-components"),
            OsString::from("--no-plugin-dirs"),
            OsString::from("--impersonate"),
            OsString::from(super::INSTAGRAM_BROWSER_PROFILE),
            OsString::from("--js-runtimes"),
            deno_runtime,
            OsString::from("--simulate"),
            OsString::from("--no-warnings"),
        ];
        match selector.playlist_item() {
            None => args.extend([
                OsString::from("--dump-single-json"),
                OsString::from("--no-playlist"),
            ]),
            Some(ordinal) => args.extend([
                OsString::from("--dump-json"),
                OsString::from("--yes-playlist"),
                OsString::from("--playlist-items"),
                OsString::from(ordinal.to_string()),
            ]),
        }
        args.extend([OsString::from("--"), OsString::from(source.url())]);
        let exit = self
            .process
            .run(
                ProcessRequest {
                    stdin: None,
                    program: ProcessProgram::from_resolved_tool(yt_dlp)
                        .with_explicit_https_proxy_for_host("www.instagram.com"),
                    args,
                    working_dir: working.path().to_owned(),
                    timeout: Some(INSTAGRAM_TIMEOUT),
                    inactivity_timeout: None,
                    output_mode: OutputMode::Capture {
                        stdout_limit: INSTAGRAM_STDOUT_LIMIT,
                        stderr_limit: INSTAGRAM_STDERR_LIMIT,
                    },
                },
                cancellation.clone(),
                &NoopSink,
            )
            .map_err(errors::map_process_error)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        if exit.code != 0 {
            let classified = errors::classify_nonzero_exit(&exit.stderr);
            if selector.playlist_item().is_some()
                && matches!(
                    classified,
                    AnalysisErrorCode::UnsupportedMedia
                        | AnalysisErrorCode::FormatUnavailable
                        | AnalysisErrorCode::Unknown
                )
                && contains_no_video(&exit.stderr)
            {
                return Err(AnalysisErrorCode::ContentChanged);
            }
            return Err(classified);
        }
        let json =
            std::str::from_utf8(&exit.stdout).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        let mut value: Value =
            serde_json::from_str(json).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        validate_instagram_identity(source, &selector, &value)?;
        super::instagram_probe::hydrate_formats(
            &mut value,
            &self.resolver,
            &self.process,
            working.path(),
            cancellation,
            deadline,
        )?;
        let json = serde_json::to_string(&value).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        let media = normalize_instagram_video(source, &selector, &json)?;
        resolve_instagram_video(media, selector, compatibility_mode)
    }
}

pub fn normalize_instagram_video(
    source: &ValidatedMediaUrl,
    selector: &InstagramVideoSelector,
    json: &str,
) -> Result<MediaInfo, AnalysisErrorCode> {
    validate_selector_source(source, selector)?;
    if json.len() > INSTAGRAM_STDOUT_LIMIT {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let value: Value =
        serde_json::from_str(json).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    validate_instagram_identity(source, selector, &value)?;
    let media = normalize_metadata(Platform::Instagram, json)?;
    if !media
        .formats
        .iter()
        .any(|format| format.kind != StreamKind::AudioOnly)
    {
        return Err(AnalysisErrorCode::FormatUnavailable);
    }
    Ok(media)
}

fn validate_instagram_identity(
    source: &ValidatedMediaUrl,
    selector: &InstagramVideoSelector,
    value: &Value,
) -> Result<(), AnalysisErrorCode> {
    let object = value
        .as_object()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    validate_root(object)?;
    if string(object, "extractor_key") != Some("Instagram") {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let actual_id = string(object, "id").ok_or(AnalysisErrorCode::InvalidMetadata)?;
    match selector {
        InstagramVideoSelector::Reel => {
            if actual_id != source.media_id()
                || object
                    .get("playlist_id")
                    .is_some_and(|value| !value.is_null())
                || object
                    .get("playlist_index")
                    .is_some_and(|value| !value.is_null())
            {
                return Err(AnalysisErrorCode::ContentChanged);
            }
        }
        InstagramVideoSelector::PostItem {
            ordinal,
            expected_media_id,
        } => {
            let expected_id = instagram_media_shortcode(expected_media_id)
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            let playlist_index = object.get("playlist_index").and_then(Value::as_u64);
            let playlist_count = object.get("playlist_count").and_then(Value::as_u64);
            if actual_id != expected_id
                || string(object, "playlist_id") != Some(source.media_id())
                || playlist_index != Some(u64::from(*ordinal))
                || !playlist_count.is_some_and(|count| (u64::from(*ordinal)..=32).contains(&count))
            {
                return Err(AnalysisErrorCode::ContentChanged);
            }
        }
    }
    Ok(())
}

fn validate_selector_source(
    source: &ValidatedMediaUrl,
    selector: &InstagramVideoSelector,
) -> Result<(), AnalysisErrorCode> {
    let valid = source.platform() == Platform::Instagram
        && matches!(
            (source.instagram_kind(), selector),
            (Some(InstagramUrlKind::Reel), InstagramVideoSelector::Reel)
                | (
                    Some(InstagramUrlKind::Post),
                    InstagramVideoSelector::PostItem { .. }
                )
        );
    if valid {
        Ok(())
    } else {
        Err(AnalysisErrorCode::UnsupportedMedia)
    }
}

fn instagram_media_shortcode(value: &str) -> Option<String> {
    let mut parts = value.split('_');
    let primary = parts.next()?;
    let owner = parts.next();
    if primary.is_empty()
        || !primary.bytes().all(|byte| byte.is_ascii_digit())
        || owner
            .is_some_and(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
        || parts.next().is_some()
    {
        return None;
    }
    let mut number = primary.parse::<u128>().ok().filter(|value| *value > 0)?;
    let mut encoded = Vec::new();
    while number > 0 {
        encoded.push(INSTAGRAM_ID_ALPHABET[(number % 64) as usize]);
        number /= 64;
    }
    encoded.reverse();
    String::from_utf8(encoded).ok()
}

fn contains_no_video(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    text.contains("there is no video") || text.contains("no video formats found")
}

struct NoopSink;
impl ProcessEventSink for NoopSink {
    fn on_line(&self, _stream: ProcessStream, _line: &[u8]) {}
}

/// Read bounded JSON and explicitly project only safe display/stream metadata.
pub fn normalize_metadata(platform: Platform, json: &str) -> Result<MediaInfo, AnalysisErrorCode> {
    if json.len() > MAX_METADATA_BYTES {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let value: Value =
        serde_json::from_str(json).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let object = value
        .as_object()
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    validate_root(object)?;

    let media_id = string(object, "id")
        .filter(|value| safe_token(value, 128))
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let title = string(object, "title")
        .and_then(|value| display_text(value, 512))
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let rows = object
        .get("formats")
        .and_then(Value::as_array)
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    if rows.len() > 512 {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let mut formats = Vec::new();
    let mut ids = HashSet::new();
    for row in rows {
        if let Some(format) = row.as_object().and_then(normalize_format) {
            if !ids.insert(format.id.clone()) {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            formats.push(format);
        }
    }
    if formats.is_empty() {
        return Err(AnalysisErrorCode::FormatUnavailable);
    }
    let media = MediaInfo {
        platform,
        media_id: media_id.to_owned(),
        title,
        creator: string(object, "uploader").and_then(|value| display_text(value, 256)),
        duration_seconds: number(object, "duration")
            .filter(|value| (0.0..=31_536_000.0).contains(value)),
        formats,
    };
    media.validate()?;
    Ok(media)
}

fn normalize_format(object: &Map<String, Value>) -> Option<MediaFormat> {
    if object.get("has_drm").and_then(Value::as_bool) == Some(true)
        || ["vcodec", "acodec"]
            .iter()
            .any(|key| matches!(string(object, key), Some("images" | "storyboard")))
    {
        return None;
    }
    let id = string(object, "format_id").filter(|value| safe_token(value, 64))?;
    let container = string(object, "ext").filter(|value| {
        !value.is_empty()
            && value.len() <= 16
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })?;
    let video_codec = codec(object, "vcodec");
    let audio_codec = codec(object, "acodec");
    let kind = match (video_codec.is_some(), audio_codec.is_some()) {
        (true, true) => StreamKind::Combined,
        (true, false) => StreamKind::VideoOnly,
        (false, true) => StreamKind::AudioOnly,
        (false, false) => return None,
    };
    let has_video = kind != StreamKind::AudioOnly;
    let has_audio = kind != StreamKind::VideoOnly;
    let file_size = bytes(object, "filesize")
        .map(|bytes| FileSize {
            bytes,
            is_approximate: false,
        })
        .or_else(|| {
            bytes(object, "filesize_approx").map(|bytes| FileSize {
                bytes,
                is_approximate: true,
            })
        });
    Some(MediaFormat {
        id: id.to_owned(),
        container: container.to_owned(),
        kind,
        width: has_video.then(|| dimension(object, "width")).flatten(),
        height: has_video.then(|| dimension(object, "height")).flatten(),
        fps: has_video
            .then(|| number(object, "fps").filter(|value| *value > 0.0 && *value <= 240.0))
            .flatten(),
        video_bitrate_kbps: has_video
            .then(|| number(object, "vbr").filter(|value| *value > 0.0 && *value <= 1_000_000.0))
            .flatten(),
        audio_bitrate_kbps: has_audio
            .then(|| number(object, "abr").filter(|value| *value > 0.0 && *value <= 100_000.0))
            .flatten(),
        audio_sample_rate_hz: has_audio
            .then(|| bounded_u64(object, "asr", 768_000).map(|value| value as u32))
            .flatten(),
        audio_channels: has_audio
            .then(|| bounded_u64(object, "audio_channels", 64).map(|value| value as u8))
            .flatten(),
        video_codec,
        audio_codec,
        dynamic_range: has_video
            .then(|| {
                string(object, "dynamic_range")
                    .filter(|value| {
                        matches!(
                            *value,
                            "SDR" | "HDR" | "HDR10" | "HDR10+" | "HDR12" | "HLG" | "DV"
                        )
                    })
                    .map(str::to_owned)
            })
            .flatten(),
        file_size,
    })
}

fn validate_root(object: &Map<String, Value>) -> Result<(), AnalysisErrorCode> {
    if object
        .get("_type")
        .is_some_and(|value| value.as_str() != Some("video"))
        || object.contains_key("entries")
        || object.get("is_live").and_then(Value::as_bool) == Some(true)
        || matches!(
            string(object, "live_status"),
            Some("is_live" | "is_upcoming" | "post_live")
        )
        || object.get("has_drm").and_then(Value::as_bool) == Some(true)
    {
        Err(AnalysisErrorCode::UnsupportedMedia)
    } else {
        Ok(())
    }
}

fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn safe_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn display_text(value: &str, max: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn codec(object: &Map<String, Value>, key: &str) -> Option<String> {
    string(object, key)
        .filter(|value| *value != "none" && safe_token(value, 64))
        .map(str::to_owned)
}

fn number(object: &Map<String, Value>, key: &str) -> Option<f64> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn dimension(object: &Map<String, Value>, key: &str) -> Option<u32> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .filter(|value| (1..=16384).contains(value))
        .map(|value| value as u32)
}

fn bounded_u64(object: &Map<String, Value>, key: &str, max: u64) -> Option<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .filter(|value| (1..=max).contains(value))
}

fn bytes(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .filter(|value| (1..=9_007_199_254_740_991).contains(value))
}
