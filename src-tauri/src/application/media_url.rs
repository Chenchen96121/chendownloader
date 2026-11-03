use crate::domain::{
    instagram::InstagramUrlKind,
    media::{AnalysisErrorCode, Platform},
};
use url::{Host, Url};

/// Native-only source identity. Deliberately neither serializable nor Debug.
#[derive(Clone)]
pub struct ValidatedMediaUrl {
    url: String,
    platform: Platform,
    media_id: String,
    instagram_kind: Option<InstagramUrlKind>,
}

impl ValidatedMediaUrl {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn media_id(&self) -> &str {
        &self.media_id
    }

    pub fn instagram_kind(&self) -> Option<InstagramUrlKind> {
        self.instagram_kind
    }
}

pub fn validate_media_url(input: &str) -> Result<ValidatedMediaUrl, AnalysisErrorCode> {
    use AnalysisErrorCode::{InvalidUrl, UnsupportedMedia, UnsupportedPlatform};
    if input.chars().any(char::is_control) || input.contains('\\') {
        return Err(InvalidUrl);
    }
    let input = input.trim();
    if input.len() > 2048 {
        return Err(InvalidUrl);
    }
    let parsed = Url::parse(input).map_err(|_| InvalidUrl)?;
    // url accepts browser-style scheme shorthand and erases empty userinfo.
    // Inspect these syntax markers before canonicalization; authority parsing
    // and host/port interpretation remain solely the URL parser's job.
    let (_, after_scheme) = input.split_once("://").ok_or(InvalidUrl)?;
    let authority_syntax = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if parsed.scheme() != "https"
        || authority_syntax.is_empty()
        || authority_syntax.contains('@')
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return Err(InvalidUrl);
    }
    let host = match parsed.host() {
        Some(Host::Domain(host)) => host,
        _ => return Err(InvalidUrl),
    };
    let platform = match host {
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be" => Platform::Youtube,
        "bilibili.com" | "www.bilibili.com" => Platform::Bilibili,
        "x.com" | "www.x.com" | "twitter.com" | "www.twitter.com" => Platform::X,
        "instagram.com" | "www.instagram.com" => Platform::Instagram,
        _ => return Err(UnsupportedPlatform),
    };
    if platform == Platform::X && authority_syntax.contains(':') {
        return Err(InvalidUrl);
    }
    // Reject paths whose spelling was changed by dot-segment normalization;
    // otherwise a non-resource route could be disguised as an allowed route.
    let original_path = after_scheme[authority_syntax.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if original_path != parsed.path() {
        return Err(UnsupportedMedia);
    }
    let path = parsed.path();
    let path = path.strip_suffix('/').unwrap_or(path);
    let segments = path.split('/').collect::<Vec<_>>();
    let (media_id, instagram_kind) = match (platform, segments.as_slice()) {
        (Platform::Youtube, ["", "watch"]) if host != "youtu.be" => {
            let mut ids = parsed.query_pairs().filter(|(key, _)| key == "v");
            let (_, id) = ids.next().ok_or(UnsupportedMedia)?;
            if ids.next().is_some() || !youtube_id(&id) {
                return Err(UnsupportedMedia);
            }
            // query_pairs decodes escapes, so inspect the selected raw value.
            let encoded_id = parsed.query().is_some_and(|query| {
                query.split('&').any(|pair| {
                    let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                    url::form_urlencoded::parse(key.as_bytes())
                        .next()
                        .is_some_and(|(decoded_key, _)| decoded_key == "v")
                        && value.contains('%')
                })
            });
            if encoded_id {
                return Err(UnsupportedMedia);
            }
            (id.into_owned(), None)
        }
        (Platform::Youtube, ["", id]) if host == "youtu.be" && youtube_id(id) => {
            ((*id).to_owned(), None)
        }
        (Platform::Youtube, ["", "shorts", id]) if host != "youtu.be" && youtube_id(id) => {
            ((*id).to_owned(), None)
        }
        (Platform::Bilibili, ["", "video", id]) if bilibili_id(id) => ((*id).to_owned(), None),
        (Platform::X, ["", "i", "web", "status", id]) if digits(id, 20) => ((*id).to_owned(), None),
        (Platform::X, ["", user, "status", id])
            if ascii_token(user, 15, |b| b.is_ascii_alphanumeric() || b == b'_')
                && digits(id, 20) =>
        {
            ((*id).to_owned(), None)
        }
        (Platform::X, ["", user, "status", id, "photo" | "video", ordinal])
            if ascii_token(user, 15, |b| b.is_ascii_alphanumeric() || b == b'_')
                && digits(id, 20)
                && valid_x_ordinal(ordinal) =>
        {
            ((*id).to_owned(), None)
        }
        (Platform::X, ["", "i", "web", "status", id, "photo" | "video", ordinal])
            if digits(id, 20) && valid_x_ordinal(ordinal) =>
        {
            ((*id).to_owned(), None)
        }
        (Platform::Instagram, ["", route @ ("p" | "reel" | "reels"), id])
            if ascii_token(id, 64, |b| {
                b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
            }) =>
        {
            let kind = if *route == "p" {
                InstagramUrlKind::Post
            } else {
                InstagramUrlKind::Reel
            };
            ((*id).to_owned(), Some(kind))
        }
        _ => return Err(UnsupportedMedia),
    };
    let url = if platform == Platform::X {
        format!("https://x.com/i/web/status/{media_id}")
    } else if let Some(kind) = instagram_kind {
        format!(
            "https://www.instagram.com/{}/{}/",
            kind.path_segment(),
            media_id
        )
    } else {
        parsed.into()
    };
    Ok(ValidatedMediaUrl {
        url,
        platform,
        media_id,
        instagram_kind,
    })
}

fn valid_x_ordinal(value: &str) -> bool {
    digits(value, 2)
        && !value.starts_with('0')
        && value.parse::<u8>().is_ok_and(|n| (1..=32).contains(&n))
}

fn ascii_token(value: &str, max: usize, allowed: impl Fn(u8) -> bool) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(allowed)
}

fn youtube_id(id: &str) -> bool {
    id.len() == 11
        && ascii_token(id, 11, |b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
        })
}

fn digits(value: &str, max: usize) -> bool {
    ascii_token(value, max, |b| b.is_ascii_digit())
}

fn bilibili_id(id: &str) -> bool {
    id.strip_prefix("BV").is_some_and(|suffix| {
        suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    }) || id
        .strip_prefix("av")
        .is_some_and(|suffix| digits(suffix, 20))
}
