use crate::domain::media::AnalysisErrorCode;
use url::{Host, Url};
#[derive(Clone, Copy)]
pub enum XAssetKind {
    Image,
    Video,
}
pub fn validate_x_asset_url(raw: &str, kind: XAssetKind) -> Result<Url, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if raw.len() > 16_384
        || raw.trim() != raw
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
    {
        return Err(invalid);
    }
    let rest = raw.strip_prefix("https://").ok_or(invalid)?;
    let authority = rest.split(['/', '?', '#']).next().ok_or(invalid)?;
    if authority.contains(['@', ':']) || authority.is_empty() {
        return Err(invalid);
    }
    let url = Url::parse(raw).map_err(|_| invalid)?;
    let host = match kind {
        XAssetKind::Image => "pbs.twimg.com",
        XAssetKind::Video => "video.twimg.com",
    };
    let path = rest[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if url.scheme() != "https"
        || url.host() != Some(Host::Domain(host))
        || url.fragment().is_some()
        || path != url.path()
        || path.contains('%')
    {
        return Err(invalid);
    }
    Ok(url)
}
