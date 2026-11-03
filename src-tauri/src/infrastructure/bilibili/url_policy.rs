use crate::domain::{bilibili::strict_web_url_with_port, media::AnalysisErrorCode};
use url::Url;

#[derive(Clone, Copy)]
pub enum BilibiliAssetKind {
    Cover,
    Media,
}

pub fn validate_bilibili_asset_url(
    input: &str,
    kind: BilibiliAssetKind,
) -> Result<Url, AnalysisErrorCode> {
    let cover = matches!(kind, BilibiliAssetKind::Cover);
    // The fixed extractor can select HTTPS MCDN streams on 8082. This is
    // authority for native media only, not a general page/CDN port relaxation.
    let media_port = (!cover && input.len() <= 8192)
        .then(|| Url::parse(input.trim()).ok())
        .flatten()
        .filter(|url| {
            url.scheme() == "https"
                && url.port() == Some(8082)
                && url
                    .host_str()
                    .is_some_and(|host| host.ends_with(".mcdn.bilivideo.cn"))
        })
        .map(|_| 8082);
    let mut url = strict_web_url_with_port(input, cover, 8192, media_port)?;
    let host = url.host_str().ok_or(AnalysisErrorCode::InvalidUrl)?;
    let allowed = if cover {
        host.ends_with(".hdslb.com")
    } else {
        ["bilivideo.com", "bilivideo.cn"]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    };
    if !allowed || url.fragment().is_some() {
        return Err(AnalysisErrorCode::InvalidUrl);
    }
    if cover && url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|_| AnalysisErrorCode::InvalidUrl)?;
        return validate_bilibili_asset_url(url.as_str(), kind);
    }
    Ok(url)
}
