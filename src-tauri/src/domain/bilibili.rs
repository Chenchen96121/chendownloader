//! Strict native-only ordinary-video selection. No signed URLs in public DTOs.
use super::media::AnalysisErrorCode;
use url::{Host, Url};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BilibiliAnalysisPreview {
    pub analysis_id: String,
    pub part: u16,
    pub media: super::media::MediaInfo,
    pub ranked_video_format_ids: Vec<String>,
    pub ranked_audio_format_ids: Vec<String>,
    pub outcome: crate::application::preview_selection::SelectionOutcome,
}
#[derive(Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BilibiliCoverPayload {
    pub mime_type: String,
    pub data_base64: String,
}
impl BilibiliCoverPayload {
    pub(crate) fn valid(&self) -> bool {
        let s = self.data_base64.as_bytes();
        let padding = s.iter().rev().take_while(|&&b| b == b'=').count();
        !s.is_empty()
            && s.len() <= 11_184_812
            && s.len().is_multiple_of(4)
            && padding <= 2
            && s.len() / 4 * 3 - padding <= 8 * 1024 * 1024
            && s[..s.len() - padding]
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
            && match self.mime_type.as_str() {
                "image/png" => self.data_base64.starts_with("iVBORw0KGgo"),
                "image/jpeg" => self.data_base64.starts_with("/9j/"),
                "image/webp" => self.data_base64.starts_with("UklGR"),
                _ => false,
            }
    }
}

#[derive(Clone)]
pub struct BilibiliVideoRequest {
    key: String,
    part: u16,
}
impl BilibiliVideoRequest {
    pub fn part(&self) -> u16 {
        self.part
    }
    pub fn page_url(&self) -> String {
        format!(
            "https://www.bilibili.com/video/{}?p={}",
            self.key, self.part
        )
    }
    pub(crate) fn with_part(mut self, part: u16) -> Self {
        self.part = part;
        self
    }
    pub(crate) fn matches(&self, bvid: &str, aid: &str) -> bool {
        if self.key.starts_with("BV") {
            self.key == bvid
        } else {
            self.key.strip_prefix("av") == Some(aid)
        }
    }
}

pub fn validate_bilibili_url(input: &str) -> Result<BilibiliVideoRequest, AnalysisErrorCode> {
    let parsed = strict_web_url(input, false, 2048)?;
    if !matches!(parsed.host_str(), Some("bilibili.com" | "www.bilibili.com")) {
        return Err(AnalysisErrorCode::UnsupportedPlatform);
    }
    let key = parsed
        .path()
        .strip_prefix("/video/")
        .map(|path| path.strip_suffix('/').unwrap_or(path))
        .filter(|key| valid_bvid(key) || valid_avid(key))
        .ok_or(AnalysisErrorCode::UnsupportedMedia)?;
    let mut part = None;
    for pair in parsed
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|s| !s.is_empty())
    {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        // Decoding is used only to spot disguised selector keys. Values are raw.
        let decoded = url::form_urlencoded::parse(pair.as_bytes())
            .next()
            .map(|(key, _)| key.into_owned())
            .unwrap_or_default();
        if decoded.eq_ignore_ascii_case("p") {
            if key != "p" || part.is_some() || value.starts_with('0') || !decimal(value, 5) {
                return Err(AnalysisErrorCode::InvalidUrl);
            }
            part = Some(
                value
                    .parse::<u16>()
                    .ok()
                    .filter(|n| (1..=10000).contains(n))
                    .ok_or(AnalysisErrorCode::InvalidUrl)?,
            );
        }
    }
    let key = if let Some(aid) = key.strip_prefix("av") {
        format!("av{}", aid.trim_start_matches('0'))
    } else {
        key.to_owned()
    };
    Ok(BilibiliVideoRequest {
        key,
        part: part.unwrap_or(1),
    })
}

pub(crate) fn valid_bvid(value: &str) -> bool {
    value
        .strip_prefix("BV")
        .is_some_and(|tail| tail.len() == 10 && tail.bytes().all(|b| b.is_ascii_alphanumeric()))
}
fn valid_avid(value: &str) -> bool {
    value
        .strip_prefix("av")
        .is_some_and(|tail| decimal(tail, 20) && tail.bytes().any(|b| b != b'0'))
}
pub(crate) fn decimal(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && value.bytes().all(|b| b.is_ascii_digit())
}
pub(crate) fn strict_web_url(
    input: &str,
    allow_http: bool,
    maximum: usize,
) -> Result<Url, AnalysisErrorCode> {
    strict_web_url_with_port(input, allow_http, maximum, None)
}

// The caller must separately authorize the host and port pair. All raw-input
// checks remain identical; page and cover callers use the no-port wrapper.
pub(crate) fn strict_web_url_with_port(
    input: &str,
    allow_http: bool,
    maximum: usize,
    allowed_port: Option<u16>,
) -> Result<Url, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidUrl;
    if input.len() > maximum || input.chars().any(char::is_control) || input.contains('\\') {
        return Err(invalid);
    }
    let input = input.trim();
    if input.chars().any(char::is_whitespace) {
        return Err(invalid);
    }
    let parsed = Url::parse(input).map_err(|_| invalid)?;
    let (_, tail) = input.split_once("://").ok_or(invalid)?;
    let authority = tail.split(['/', '?', '#']).next().ok_or(invalid)?;
    let raw_path = tail[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let valid_authority = match allowed_port {
        None => !authority.contains(':') && parsed.port().is_none(),
        Some(port) => {
            parsed.scheme() == "https"
                && parsed.port() == Some(port)
                && parsed
                    .host_str()
                    .is_some_and(|host| authority == format!("{host}:{port}"))
        }
    };
    if !matches!(parsed.host(), Some(Host::Domain(_)))
        || authority.is_empty()
        || authority.contains('@')
        || !valid_authority
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !(parsed.scheme() == "https" || (allow_http && parsed.scheme() == "http"))
        || raw_path != parsed.path()
        || raw_path.contains('%')
    {
        return Err(invalid);
    }
    Ok(parsed)
}
