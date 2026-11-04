use crate::{
    application::media_url::validate_media_url,
    domain::media::{AnalysisErrorCode, Platform},
};
use url::Url;

pub fn normalize_tiktok_input(input: &str) -> Result<String, AnalysisErrorCode> {
    if input.len() > 8192
        || input.contains('\\')
        || input.chars().any(|c| c.is_control() && !c.is_whitespace())
    {
        return Err(AnalysisErrorCode::InvalidUrl);
    }
    let urls = input
        .split_whitespace()
        .filter(|part| part.starts_with("https://") || part.starts_with("http://"))
        .collect::<Vec<_>>();
    if urls.len() != 1 {
        return Err(AnalysisErrorCode::InvalidUrl);
    }
    let raw = urls[0].trim_end_matches(['。', '，', ',', ';', '；']);
    let url = Url::parse(raw).map_err(|_| AnalysisErrorCode::InvalidUrl)?;
    let authority = raw
        .split_once("://")
        .ok_or(AnalysisErrorCode::InvalidUrl)?
        .1
        .split(['/', '?', '#'])
        .next()
        .ok_or(AnalysisErrorCode::InvalidUrl)?;
    if url.scheme() != "https"
        || authority.contains('@')
        || authority.contains(':')
        || url.password().is_some()
        || !url.username().is_empty()
        || url.port().is_some()
    {
        return Err(AnalysisErrorCode::InvalidUrl);
    }
    let original_path = raw.split_once("://").unwrap().1[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if original_path != url.path() {
        return Err(AnalysisErrorCode::InvalidUrl);
    }
    let host = url.host_str().ok_or(AnalysisErrorCode::InvalidUrl)?;
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    let token =
        |s: &str| !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric());
    if matches!(host, "vm.tiktok.com" | "vt.tiktok.com") && parts.len() == 1 && token(parts[0]) {
        return Ok(format!("https://{host}{}", url.path()));
    }
    if matches!(host, "www.tiktok.com" | "tiktok.com")
        && parts.len() == 2
        && parts[0] == "t"
        && token(parts[1])
    {
        return Ok(format!("https://www.tiktok.com{}", url.path()));
    }
    if !matches!(host, "www.tiktok.com" | "tiktok.com") {
        return Err(AnalysisErrorCode::UnsupportedPlatform);
    }
    let canonical = format!("https://www.tiktok.com{}", url.path().trim_end_matches('/'));
    let validated = validate_media_url(&canonical)?;
    if validated.platform() != Platform::Tiktok {
        return Err(AnalysisErrorCode::UnsupportedPlatform);
    }
    Ok(canonical)
}

pub fn validate_tiktok_asset_url(raw: &str) -> Result<Url, AnalysisErrorCode> {
    if raw.len() > 8192 || raw.contains('\\') || raw.chars().any(char::is_control) {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let url = Url::parse(raw).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    let authority = raw
        .split_once("://")
        .ok_or(AnalysisErrorCode::InvalidMetadata)?
        .1
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let host = url.host_str().ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let domains = [
        "tiktokcdn.com",
        "tiktokcdn-us.com",
        "tiktokcdn-eu.com",
        "tiktokv.com",
        "byteoversea.com",
        "ibytedtos.com",
        "muscdn.com",
    ];
    if url.scheme() != "https"
        || authority.contains('@')
        || authority.contains(':')
        || url.port().is_some()
        || url.password().is_some()
        || !url.username().is_empty()
        || !matches!(
            host,
            "v16-webapp-prime.tiktok.com" | "v19-webapp-prime.tiktok.com"
        ) && !domains
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(url)
}
