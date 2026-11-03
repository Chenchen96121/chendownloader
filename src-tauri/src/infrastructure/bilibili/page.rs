use super::{
    json::UniqueValue,
    url_policy::{validate_bilibili_asset_url, BilibiliAssetKind},
};
use crate::{
    application::{
        bilibili_source::{
            BilibiliPageEvidence, BilibiliPageIdentity, BilibiliPageLoader, BilibiliRequestContext,
        },
        media_url::validate_media_url,
    },
    domain::{
        bilibili::{decimal, valid_bvid, validate_bilibili_url, BilibiliVideoRequest},
        media::AnalysisErrorCode,
    },
};
use serde_json::Value;
use std::{
    collections::HashSet,
    io::Read,
    time::{Duration, Instant},
};
use url::Url;

const PAGE_LIMIT: usize = 4 * 1024 * 1024;
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct BilibiliPageResponse {
    pub status: u16,
    pub location: Option<String>,
    pub body: Vec<u8>,
}
pub trait BilibiliPageTransport: Send + Sync {
    fn get(
        &self,
        url: &Url,
        context: &BilibiliRequestContext,
        deadline: Instant,
    ) -> Result<BilibiliPageResponse, AnalysisErrorCode>;
}
pub struct HttpBilibiliPageLoader<T = ReqwestBilibiliPageTransport> {
    transport: T,
}
impl Default for HttpBilibiliPageLoader {
    fn default() -> Self {
        Self {
            transport: ReqwestBilibiliPageTransport,
        }
    }
}
impl<T> HttpBilibiliPageLoader<T> {
    pub fn with_transport(transport: T) -> Self {
        Self { transport }
    }
}
impl<T: BilibiliPageTransport> BilibiliPageLoader for HttpBilibiliPageLoader<T> {
    fn inspect(
        &self,
        request: &BilibiliVideoRequest,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliPageEvidence, AnalysisErrorCode> {
        let deadline = Instant::now() + context.remaining(PAGE_TIMEOUT)?;
        let mut current = request.clone();
        let mut current_url =
            Url::parse(&current.page_url()).map_err(|_| AnalysisErrorCode::InvalidUrl)?;
        let mut visited = vec![current.clone()];
        let mut total_bytes = 0_usize;
        for hop in 0..=3 {
            ensure_time(context, deadline)?;
            let response = self.transport.get(&current_url, context, deadline)?;
            ensure_time(context, deadline)?;
            total_bytes = total_bytes
                .checked_add(response.body.len())
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            if total_bytes > PAGE_LIMIT {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                if hop == 3 {
                    return Err(AnalysisErrorCode::AccessDenied);
                }
                let location = response
                    .location
                    .ok_or(AnalysisErrorCode::InvalidMetadata)?;
                let next = if location.starts_with("/video/") {
                    format!("https://www.bilibili.com{location}")
                } else {
                    location
                };
                current = validate_bilibili_url(&next)?.with_part(request.part());
                current_url =
                    Url::parse(&current.page_url()).map_err(|_| AnalysisErrorCode::InvalidUrl)?;
                // Preserve the validated site's canonical trailing slash. The
                // stable request identity still freezes P and drops tracking.
                if Url::parse(&next)
                    .map_err(|_| AnalysisErrorCode::InvalidUrl)?
                    .path()
                    .ends_with('/')
                {
                    current_url.set_path(&format!("{}/", current_url.path().trim_end_matches('/')));
                }
                visited.push(current.clone());
                continue;
            }
            match response.status {
                200 => {}
                401 => return Err(AnalysisErrorCode::AuthenticationRequired),
                403 => return Err(AnalysisErrorCode::AccessDenied),
                404 | 410 => return Err(AnalysisErrorCode::ContentUnavailable),
                429 => return Err(AnalysisErrorCode::RateLimited),
                _ => return Err(AnalysisErrorCode::NetworkFailed),
            }
            let evidence = parse_bilibili_page(request, &response.body)?;
            if visited
                .iter()
                .any(|r| !r.matches(&evidence.identity().bvid, &evidence.identity().aid))
            {
                return Err(AnalysisErrorCode::ContentChanged);
            }
            return Ok(evidence);
        }
        Err(AnalysisErrorCode::AnalysisUnavailable)
    }
}
fn ensure_time(
    context: &BilibiliRequestContext,
    deadline: Instant,
) -> Result<(), AnalysisErrorCode> {
    context.remaining(PAGE_TIMEOUT)?;
    if Instant::now() >= deadline {
        return Err(AnalysisErrorCode::AnalysisTimeout);
    }
    Ok(())
}
pub struct ReqwestBilibiliPageTransport;
impl BilibiliPageTransport for ReqwestBilibiliPageTransport {
    fn get(
        &self,
        url: &Url,
        context: &BilibiliRequestContext,
        deadline: Instant,
    ) -> Result<BilibiliPageResponse, AnalysisErrorCode> {
        ensure_time(context, deadline)?;
        validate_bilibili_url(url.as_str())?;
        let timeout = deadline.saturating_duration_since(Instant::now());
        let mut builder = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout);
        let network = context.network();
        if let Some(proxy) =
            network.https_proxy_for_host(url.host_str().ok_or(AnalysisErrorCode::InvalidUrl)?)
        {
            builder = builder.proxy(
                reqwest::Proxy::https(proxy.as_str())
                    .map_err(|_| AnalysisErrorCode::NetworkFailed)?,
            );
        }
        let mut response = builder
            .build()
            .map_err(|_| AnalysisErrorCode::NetworkFailed)?
            .get(url.clone())
            .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
            .header(reqwest::header::ACCEPT, "text/html")
            .send()
            .map_err(map_http)?;
        ensure_time(context, deadline)?;
        let status = response.status().as_u16();
        let headers = response.headers().get_all(reqwest::header::LOCATION);
        if headers.iter().count() > 1 {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let location = headers
            .iter()
            .next()
            .map(|v| v.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        if location.as_ref().is_some_and(|s| s.len() > 2048) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        if status != 200 {
            return Ok(BilibiliPageResponse {
                status,
                location,
                body: vec![],
            });
        }
        let encodings = response
            .headers()
            .get_all(reqwest::header::CONTENT_ENCODING);
        if encodings.iter().count() > 1 {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let gzip = match encodings
            .iter()
            .next()
            .map(|value| value.to_str())
            .transpose()
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?
        {
            None => false,
            Some(value) if value.eq_ignore_ascii_case("identity") => false,
            Some(value) if value.eq_ignore_ascii_case("gzip") => true,
            _ => return Err(AnalysisErrorCode::InvalidMetadata),
        };
        if response
            .content_length()
            .is_some_and(|n| n > PAGE_LIMIT as u64)
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let mut body = Vec::new();
        let mut buffer = [0_u8; 16384];
        loop {
            ensure_time(context, deadline)?;
            let count = response
                .read(&mut buffer)
                .map_err(|_| AnalysisErrorCode::NetworkFailed)?;
            if count == 0 {
                break;
            }
            if body.len() + count > PAGE_LIMIT {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            body.extend_from_slice(&buffer[..count]);
        }
        let body = decode_page_body(body, gzip, context, deadline)?;
        Ok(BilibiliPageResponse {
            status,
            location,
            body,
        })
    }
}
fn decode_page_body(
    wire: Vec<u8>,
    gzip: bool,
    context: &BilibiliRequestContext,
    deadline: Instant,
) -> Result<Vec<u8>, AnalysisErrorCode> {
    ensure_time(context, deadline)?;
    if wire.len() > PAGE_LIMIT {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    if !gzip {
        return Ok(wire);
    }
    let mut decoder = flate2::bufread::GzDecoder::new(wire.as_slice());
    let mut body = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        ensure_time(context, deadline)?;
        let count = decoder
            .read(&mut buffer)
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
        ensure_time(context, deadline)?;
        if count == 0 {
            break;
        }
        if body.len() + count > PAGE_LIMIT {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        body.extend_from_slice(&buffer[..count]);
    }
    // CRC/trailer must validate and exactly one complete gzip document is read.
    if !decoder.get_ref().is_empty() {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(body)
}
fn map_http(error: reqwest::Error) -> AnalysisErrorCode {
    if error.is_timeout() {
        AnalysisErrorCode::AnalysisTimeout
    } else {
        AnalysisErrorCode::NetworkFailed
    }
}

pub fn parse_bilibili_page(
    request: &BilibiliVideoRequest,
    html: &[u8],
) -> Result<BilibiliPageEvidence, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if html.len() > PAGE_LIMIT {
        return Err(invalid);
    }
    let html = std::str::from_utf8(html).map_err(|_| invalid)?;
    let state = initial_state(html)?;
    if let Some(error) = state.get("error").filter(|v| !v.is_null()) {
        let error = error.as_object().ok_or(invalid)?;
        for key in ["trueCode", "code"] {
            if let Some(value) = error.get(key) {
                match value.as_i64().ok_or(invalid)? {
                    0 => {}
                    -403 => return Err(AnalysisErrorCode::AuthenticationRequired),
                    -404 => return Err(AnalysisErrorCode::ContentUnavailable),
                    _ => return Err(AnalysisErrorCode::AccessDenied),
                }
            }
        }
    }
    let data = state
        .get("videoData")
        .and_then(Value::as_object)
        .ok_or(AnalysisErrorCode::UnsupportedMedia)?;
    let rights = data
        .get("rights")
        .and_then(Value::as_object)
        .ok_or(AnalysisErrorCode::UnsupportedMedia)?;
    // A bounded ordinary-page witness needs affirmative free/ordinary fields,
    // not merely an absence of downloader warnings or login-popup text.
    for key in ["pay", "ugc_pay", "movie", "is_stein_gate"] {
        if !rights.get(key).is_some_and(zero_flag) {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
    }
    for key in [
        "pay",
        "ugc_pay",
        "movie",
        "is_stein_gate",
        "ugc_pay_preview",
        "is_upower_exclusive",
        "is_upower_play",
        "is_trial",
        "is_preview",
        "is_pay",
        "is_paid",
        "is_chargeable_season",
    ] {
        if rights
            .get(key)
            .into_iter()
            .chain(data.get(key))
            .any(|v| !zero_flag(v))
        {
            return Err(AnalysisErrorCode::UnsupportedMedia);
        }
    }
    if data
        .get("redirect_url")
        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        return Err(AnalysisErrorCode::UnsupportedMedia);
    }
    let bvid = data
        .get("bvid")
        .and_then(Value::as_str)
        .filter(|v| valid_bvid(v))
        .ok_or(invalid)?
        .to_owned();
    let aid = numeric_id(data.get("aid").ok_or(invalid)?)?;
    if !request.matches(&bvid, &aid) {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    let pages = data
        .get("pages")
        .and_then(Value::as_array)
        .filter(|p| !p.is_empty() && p.len() <= 10000)
        .ok_or(invalid)?;
    let mut seen = HashSet::new();
    let mut selected = None;
    for (index, page) in pages.iter().enumerate() {
        let part = page
            .get("page")
            .and_then(Value::as_u64)
            .filter(|n| (1..=10000).contains(n))
            .ok_or(invalid)? as u16;
        if usize::from(part) != index + 1 || !seen.insert(part) {
            return Err(invalid);
        }
        let cid = numeric_id(page.get("cid").ok_or(invalid)?)?;
        if part == request.part() {
            let duration = page
                .get("duration")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= 31_536_000.0)
                .ok_or(invalid)?;
            selected = Some((cid, duration));
        }
    }
    let (cid, duration_seconds) = selected.ok_or(AnalysisErrorCode::ContentUnavailable)?;
    let source = validate_media_url(&format!(
        "https://www.bilibili.com/video/{bvid}?p={}",
        request.part()
    ))?;
    let cover = data
        .get("pic")
        .and_then(Value::as_str)
        .and_then(|url| validate_bilibili_asset_url(url, BilibiliAssetKind::Cover).ok());
    Ok(BilibiliPageEvidence {
        identity: BilibiliPageIdentity {
            bvid,
            aid,
            part: request.part(),
            cid,
            duration_seconds,
        },
        source,
        cover,
    })
}
fn zero_flag(value: &Value) -> bool {
    matches!(value, Value::Bool(false)) || value.as_u64() == Some(0)
}
fn numeric_id(value: &Value) -> Result<String, AnalysisErrorCode> {
    let text = if let Some(text) = value.as_str() {
        text.to_owned()
    } else if let Some(n) = value.as_u64() {
        n.to_string()
    } else {
        return Err(AnalysisErrorCode::InvalidMetadata);
    };
    if !decimal(&text, 20) || text.starts_with('0') {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    Ok(text)
}
fn initial_state(html: &str) -> Result<Value, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let lower = html.to_ascii_lowercase();
    let mut offset = 0;
    let mut found = None;
    while let Some(start) = lower[offset..].find("<script") {
        let start = offset + start + "<script".len();
        if !lower
            .as_bytes()
            .get(start)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>')
        {
            offset = start;
            continue;
        }
        let Some(end_open) = lower[start..].find('>') else {
            return Err(invalid);
        };
        let content_start = start + end_open + 1;
        let Some(end) = lower[content_start..].find("</script>") else {
            return Err(invalid);
        };
        let end = content_start + end;
        let script = html[content_start..end].trim_start();
        if let Some(assignment) = script.strip_prefix("window.__INITIAL_STATE__") {
            let payload = assignment
                .trim_start()
                .strip_prefix('=')
                .ok_or(invalid)?
                .trim_start();
            let mut stream = serde_json::Deserializer::from_str(payload).into_iter::<UniqueValue>();
            let value = stream.next().ok_or(invalid)?.map_err(|_| invalid)?.0;
            let rest = payload[stream.byte_offset()..].trim_start();
            if !rest.starts_with(';') || found.is_some() {
                return Err(invalid);
            }
            found = Some(value);
        }
        offset = end + "</script>".len();
    }
    found.ok_or(AnalysisErrorCode::AnalysisUnavailable)
}

#[cfg(test)]
mod compression_tests {
    use super::*;
    use crate::application::{network_profile::NetworkProfile, process::CancellationToken};
    use std::{io::Write, sync::Arc};
    fn context() -> BilibiliRequestContext {
        BilibiliRequestContext::new(
            CancellationToken::new(),
            Instant::now() + PAGE_TIMEOUT,
            Arc::new(NetworkProfile::current()),
        )
    }
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }
    #[test]
    fn gzip_pages_are_decoded_with_wire_and_expanded_limits_and_integrity_checks() {
        let context = context();
        let deadline = Instant::now() + PAGE_TIMEOUT;
        let compressed = gzip(b"ordinary HTML");
        assert_eq!(
            decode_page_body(compressed.clone(), true, &context, deadline).unwrap(),
            b"ordinary HTML"
        );
        assert_eq!(
            decode_page_body(b"ordinary HTML".to_vec(), false, &context, deadline).unwrap(),
            b"ordinary HTML"
        );
        let mut corrupt = compressed.clone();
        let last = corrupt.len() - 8;
        corrupt[last] ^= 1;
        let mut trailing = compressed.clone();
        trailing.push(0);
        let mut multiple = compressed;
        multiple.extend(gzip(b"other document"));
        for bytes in [
            corrupt,
            trailing,
            multiple,
            gzip(&vec![b'x'; PAGE_LIMIT + 1]),
        ] {
            assert!(matches!(
                decode_page_body(bytes, true, &context, deadline),
                Err(AnalysisErrorCode::InvalidMetadata)
            ));
        }
        assert!(decode_page_body(vec![0; PAGE_LIMIT + 1], false, &context, deadline).is_err());
    }
}
