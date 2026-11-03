use std::{
    collections::HashSet,
    error::Error,
    fs::{self, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::Path,
    time::Duration,
};

use reqwest::{
    blocking::{Client, Response},
    header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_LENGTH, COOKIE, LOCATION, REFERER, SET_COOKIE},
    StatusCode,
};
use serde_json::{json, Value};
use url::Url;

use crate::{
    application::{
        media_url::ValidatedMediaUrl,
        process::{CancellationToken, ProcessSupervisor},
        tool_resolver::ManagedToolResolver,
    },
    domain::{
        download::DownloadErrorCode,
        instagram::InstagramUrlKind,
        media::{AnalysisErrorCode, Platform},
    },
    infrastructure::gallery_dl::{
        acquire_direct_image, normalize_instagram_inventory, InstagramAssetAcquirer,
        InstagramInventory, InstagramInventoryExtractor, ResolvedInstagramItem,
        VerifiedInstagramImage,
    },
};

const GRAPHQL_URL: &str = "https://www.instagram.com/graphql/query";
const GRAPHQL_DOCUMENT_ID: &str = "27128499623469141";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_VARIANTS: usize = 128;
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const INITIAL_COOKIES: &str = "ig_pr=1; ig_vw=1920";
const MAX_COOKIE_HEADER_BYTES: usize = 4096;

pub struct InstagramGraphQlRequest {
    referer: String,
    csrf_token: String,
    form: Vec<(String, String)>,
}

impl InstagramGraphQlRequest {
    pub fn url(&self) -> &'static str {
        GRAPHQL_URL
    }

    pub fn referer(&self) -> &str {
        &self.referer
    }

    pub fn csrf_token(&self) -> &str {
        &self.csrf_token
    }

    pub fn form(&self) -> &[(String, String)] {
        &self.form
    }
}

pub trait InstagramWebTransport: Send + Sync {
    fn fetch_post(
        &self,
        request: &InstagramGraphQlRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, AnalysisErrorCode>;

    fn fetch_asset(
        &self,
        _url: &Url,
        _destination: &Path,
        _maximum_bytes: u64,
        _cancellation: &CancellationToken,
    ) -> Result<(), AnalysisErrorCode> {
        Err(AnalysisErrorCode::AnalysisUnavailable)
    }
}

pub struct ReqwestInstagramWebTransport {
    client: Client,
    homepage_url: String,
    graphql_url: String,
}

impl ReqwestInstagramWebTransport {
    pub fn new() -> Result<Self, AnalysisErrorCode> {
        Self::with_endpoints_and_proxy(
            "https://www.instagram.com/".to_owned(),
            GRAPHQL_URL.to_owned(),
            &crate::infrastructure::system_proxy::SystemProxies::current(),
        )
    }

    fn with_endpoints_and_proxy(
        homepage_url: String,
        graphql_url: String,
        proxies: &crate::infrastructure::system_proxy::SystemProxies,
    ) -> Result<Self, AnalysisErrorCode> {
        let mut builder = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            // Ambient proxy environment values remain disabled. Only validated
            // Windows settings chosen by the current user are applied below.
            .no_proxy();
        if let Some(proxy) = &proxies.http {
            builder = builder.proxy(
                reqwest::Proxy::http(proxy.as_str())
                    .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?
                    .no_proxy(reqwest::NoProxy::from_string(
                        proxies.bypass.as_deref().unwrap_or_default(),
                    )),
            );
        }
        if let Some(proxy) = &proxies.https {
            builder = builder.proxy(
                reqwest::Proxy::https(proxy.as_str())
                    .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?
                    .no_proxy(reqwest::NoProxy::from_string(
                        proxies.bypass.as_deref().unwrap_or_default(),
                    )),
            );
        }
        let client = builder
            .build()
            .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
        Ok(Self {
            client,
            homepage_url,
            graphql_url,
        })
    }

    #[cfg(test)]
    fn with_endpoints_for_test(
        homepage_url: String,
        graphql_url: String,
    ) -> Result<Self, AnalysisErrorCode> {
        Self::with_endpoints_and_proxy(
            homepage_url,
            graphql_url,
            &crate::infrastructure::system_proxy::SystemProxies::default(),
        )
    }

    fn initialize_session(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<AnonymousSession, AnalysisErrorCode> {
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let response = self
            .client
            .get(&self.homepage_url)
            .header(ACCEPT, "text/html,application/xhtml+xml")
            .header(COOKIE, INITIAL_COOKIES)
            .send()
            .map_err(map_request_error)?;
        ensure_success(&response)?;
        let mut cookies = vec![
            ("ig_pr".to_owned(), "1".to_owned()),
            ("ig_vw".to_owned(), "1920".to_owned()),
        ];
        for value in response.headers().get_all(SET_COOKIE) {
            let Ok(value) = value.to_str() else {
                continue;
            };
            let Some(cookie) = value.split(';').next() else {
                continue;
            };
            let Some((name, value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if !valid_cookie_pair(name, value) {
                continue;
            }
            cookies.retain(|(existing, _)| existing != name);
            cookies.push((name.to_owned(), value.to_owned()));
        }
        let csrf = cookies
            .iter()
            .find(|(name, value)| name == "csrftoken" && !value.is_empty())
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let cookie_header = cookies
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if cookie_header.len() > MAX_COOKIE_HEADER_BYTES {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(AnonymousSession {
            csrf,
            cookie_header,
        })
    }
}

struct AnonymousSession {
    csrf: String,
    cookie_header: String,
}

impl Default for ReqwestInstagramWebTransport {
    fn default() -> Self {
        Self::new().expect("fixed Instagram HTTP client configuration must be valid")
    }
}

impl InstagramWebTransport for ReqwestInstagramWebTransport {
    fn fetch_post(
        &self,
        request: &InstagramGraphQlRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, AnalysisErrorCode> {
        let session = if request.csrf_token().is_empty() {
            self.initialize_session(cancellation)?
        } else {
            AnonymousSession {
                csrf: request.csrf_token().to_owned(),
                cookie_header: format!("{INITIAL_COOKIES}; csrftoken={}", request.csrf_token()),
            }
        };
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let mut builder = self
            .client
            .post(&self.graphql_url)
            .header(ACCEPT, "*/*")
            .header(ACCEPT_LANGUAGE, "en-US,en;q=0.8")
            .header(REFERER, request.referer())
            .header("x-csrftoken", &session.csrf)
            .header("x-ig-app-id", "936619743392459")
            .form(request.form());
        if !session.cookie_header.is_empty() {
            builder = builder.header(COOKIE, &session.cookie_header);
        }
        let response = builder.send().map_err(map_request_error)?;
        read_bounded_response(response, cancellation)
    }

    fn fetch_asset(
        &self,
        url: &Url,
        destination: &Path,
        maximum_bytes: u64,
        cancellation: &CancellationToken,
    ) -> Result<(), AnalysisErrorCode> {
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        validate_asset_url(url)?;
        if maximum_bytes == 0 || destination.exists() {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let mut response = self
            .client
            .get(url.clone())
            .header(
                ACCEPT,
                "image/avif,image/webp,image/png,image/jpeg,*/*;q=0.8",
            )
            .send()
            .map_err(map_request_error)?;
        ensure_success(&response)?;
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length == 0 || length > maximum_bytes)
        {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let result = (|| {
            let mut total = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                if cancellation.is_canceled() {
                    return Err(AnalysisErrorCode::ProcessFailed);
                }
                let read = response
                    .read(&mut buffer)
                    .map_err(map_response_read_error)?;
                if read == 0 {
                    break;
                }
                total = total
                    .checked_add(read as u64)
                    .filter(|total| *total <= maximum_bytes)
                    .ok_or(AnalysisErrorCode::InvalidMetadata)?;
                file.write_all(&buffer[..read])
                    .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
            }
            if total == 0 {
                return Err(AnalysisErrorCode::InvalidMetadata);
            }
            file.sync_all()
                .map_err(|_| AnalysisErrorCode::ProcessFailed)
        })();
        if result.is_err() {
            drop(file);
            let _ = fs::remove_file(destination);
        }
        result
    }
}

pub struct InstagramWebAssetAcquirer<T, R, P> {
    transport: T,
    resolver: R,
    process: P,
}

impl<T, R, P> InstagramWebAssetAcquirer<T, R, P> {
    pub fn new(transport: T, resolver: R, process: P) -> Self {
        Self {
            transport,
            resolver,
            process,
        }
    }
}

impl<T, R, P> InstagramAssetAcquirer for InstagramWebAssetAcquirer<T, R, P>
where
    T: InstagramWebTransport,
    R: ManagedToolResolver + Send + Sync,
    P: ProcessSupervisor + Send + Sync,
{
    fn fetch_preview(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<VerifiedInstagramImage>, AnalysisErrorCode> {
        if !item.public_item().preview_available {
            return Ok(None);
        }
        let image = acquire_direct_image(
            &self.resolver,
            &self.process,
            item,
            protected_directory,
            cancellation,
            true,
            |destination, maximum_bytes| {
                self.transport.fetch_asset(
                    item.display_url(),
                    destination,
                    maximum_bytes,
                    cancellation,
                )
            },
        )
        .map_err(|error| error.analysis())?;
        if image.byte_length() > MAX_PREVIEW_BYTES || image.mime_type().is_none() {
            let _ = fs::remove_file(image.path());
            Ok(None)
        } else {
            Ok(Some(image))
        }
    }

    fn download_image(
        &self,
        item: &ResolvedInstagramItem,
        protected_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<VerifiedInstagramImage, DownloadErrorCode> {
        acquire_direct_image(
            &self.resolver,
            &self.process,
            item,
            protected_directory,
            cancellation,
            false,
            |destination, maximum_bytes| {
                self.transport.fetch_asset(
                    item.display_url(),
                    destination,
                    maximum_bytes,
                    cancellation,
                )
            },
        )
        .map_err(|error| error.download())
    }
}

pub(crate) fn validate_asset_url(url: &Url) -> Result<(), AnalysisErrorCode> {
    let host = url.host_str().ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let approved = host == "instagram.com"
        || host.ends_with(".instagram.com")
        || host == "cdninstagram.com"
        || host.ends_with(".cdninstagram.com")
        || host == "fbcdn.net"
        || host.ends_with(".fbcdn.net");
    if url.scheme() != "https"
        || !approved
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || url.path().is_empty()
        || url.path() == "/"
    {
        Err(AnalysisErrorCode::InvalidMetadata)
    } else {
        Ok(())
    }
}

fn valid_cookie_pair(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && value.len() <= 1024
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b';' | b',' | b' '))
}

fn read_bounded_response(
    mut response: Response,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, AnalysisErrorCode> {
    ensure_success(&response)?;
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length == 0 || length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let mut body = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let read = response
            .read(&mut buffer)
            .map_err(map_response_read_error)?;
        if read == 0 {
            break;
        }
        if body.len().saturating_add(read) > MAX_RESPONSE_BYTES {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        body.extend_from_slice(&buffer[..read]);
    }
    if body.is_empty() {
        Err(AnalysisErrorCode::InvalidMetadata)
    } else {
        Ok(body)
    }
}

fn ensure_success(response: &Response) -> Result<(), AnalysisErrorCode> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    Err(
        if status.is_redirection() && location.contains("/accounts/login") {
            AnalysisErrorCode::AuthenticationRequired
        } else {
            match status {
                StatusCode::BAD_REQUEST => AnalysisErrorCode::ExtractorOutdated,
                StatusCode::UNAUTHORIZED => AnalysisErrorCode::AuthenticationRequired,
                StatusCode::FORBIDDEN => AnalysisErrorCode::AccessDenied,
                StatusCode::NOT_FOUND | StatusCode::GONE => AnalysisErrorCode::ContentUnavailable,
                StatusCode::TOO_MANY_REQUESTS => AnalysisErrorCode::RateLimited,
                status if status.is_server_error() => AnalysisErrorCode::NetworkFailed,
                _ => AnalysisErrorCode::Unknown,
            }
        },
    )
}

fn map_request_error(error: reqwest::Error) -> AnalysisErrorCode {
    if error.is_timeout() {
        AnalysisErrorCode::AnalysisTimeout
    } else if error.is_builder() {
        AnalysisErrorCode::AnalysisUnavailable
    } else {
        AnalysisErrorCode::NetworkFailed
    }
}

fn map_response_read_error(error: std::io::Error) -> AnalysisErrorCode {
    let mut source = error.get_ref().map(|inner| inner as &(dyn Error + 'static));
    while let Some(current) = source {
        if current
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout)
        {
            return AnalysisErrorCode::AnalysisTimeout;
        }
        source = current.source();
    }
    if error.kind() == ErrorKind::TimedOut {
        AnalysisErrorCode::AnalysisTimeout
    } else {
        AnalysisErrorCode::NetworkFailed
    }
}

pub struct InstagramWebInventoryExtractor<T> {
    transport: T,
}

impl<T> InstagramWebInventoryExtractor<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T> InstagramInventoryExtractor for InstagramWebInventoryExtractor<T>
where
    T: InstagramWebTransport,
{
    fn extract_post(
        &self,
        source: &ValidatedMediaUrl,
        cancellation: &CancellationToken,
    ) -> Result<InstagramInventory, AnalysisErrorCode> {
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        let request = build_graphql_request(source, "")?;
        let response = self.transport.fetch_post(&request, cancellation)?;
        if cancellation.is_canceled() {
            return Err(AnalysisErrorCode::ProcessFailed);
        }
        parse_web_response(source, &response)
    }
}

pub fn build_graphql_request(
    source: &ValidatedMediaUrl,
    csrf_token: &str,
) -> Result<InstagramGraphQlRequest, AnalysisErrorCode> {
    validate_post_source(source)?;
    if csrf_token.len() > 256
        || csrf_token
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b';' | b',' | b' '))
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let variables = serde_json::to_string(&json!({
        "shortcode": source.media_id(),
        "__relay_internal__pv__PolarisAIGMMediaWebLabelEnabledrelayprovider": false,
    }))
    .map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    Ok(InstagramGraphQlRequest {
        referer: source.url().to_owned(),
        csrf_token: csrf_token.to_owned(),
        form: vec![
            ("variables".to_owned(), variables),
            ("doc_id".to_owned(), GRAPHQL_DOCUMENT_ID.to_owned()),
            ("server_timestamps".to_owned(), "true".to_owned()),
        ],
    })
}

pub fn parse_web_response(
    source: &ValidatedMediaUrl,
    response: &[u8],
) -> Result<InstagramInventory, AnalysisErrorCode> {
    validate_post_source(source)?;
    if response.is_empty() || response.len() > MAX_RESPONSE_BYTES {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let root: Value =
        serde_json::from_slice(response).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    if root
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != "ok")
    {
        return Err(AnalysisErrorCode::AccessDenied);
    }

    let polaris = root.pointer("/data/xig_polaris_media");
    if polaris.is_some()
        && (root
            .pointer("/data/xdt_api__v1__media__shortcode__web_info")
            .is_some()
            || root.pointer("/data/xdt_shortcode_media").is_some()
            || root.get("items").is_some())
    {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let post = if let Some(wrapper) = polaris {
        if root.get("errors").is_some_and(|errors| {
            !errors.is_null() && errors.as_array().is_none_or(|errors| !errors.is_empty())
        }) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        public_polaris_post(source, wrapper)?
    } else if let Some(items) = root
        .pointer("/data/xdt_api__v1__media__shortcode__web_info/items")
        .or_else(|| root.get("items"))
    {
        let items = items.as_array().ok_or(AnalysisErrorCode::InvalidMetadata)?;
        match items.as_slice() {
            [] => return Err(AnalysisErrorCode::ContentUnavailable),
            [post] => post,
            _ => return Err(AnalysisErrorCode::InvalidMetadata),
        }
    } else if let Some(post) = root.pointer("/data/xdt_shortcode_media") {
        post
    } else {
        return Err(AnalysisErrorCode::ExtractorOutdated);
    };

    let code = post
        .get("code")
        .or_else(|| post.get("shortcode"))
        .and_then(Value::as_str)
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    if code != source.media_id() {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    let mut inventory = normalize_web_post(source, post)?;
    inventory.mark_web_assets();
    Ok(inventory)
}

// Logged-out Polaris wraps the public media in an explicit gating boundary.
// Validate both identities before allowing the ordinary API-shaped normalizer
// to see it; a hidden/restricted wrapper is never a partial inventory.
fn public_polaris_post<'a>(
    source: &ValidatedMediaUrl,
    wrapper: &'a Value,
) -> Result<&'a Value, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if wrapper.get("code").and_then(Value::as_str) != Some(source.media_id()) {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    match wrapper.get("gating_ruling") {
        Some(Value::Null) => {}
        Some(_) => return Err(AnalysisErrorCode::AccessDenied),
        None => return Err(invalid),
    }
    let post = match wrapper.get("if_not_gated_logged_out") {
        Some(Value::Null) => return Err(AnalysisErrorCode::AuthenticationRequired),
        Some(post) if post.is_object() => post,
        _ => return Err(invalid),
    };
    let expected_type = match post.get("media_type").and_then(Value::as_u64) {
        Some(1) => "XIGPolarisImageMedia",
        Some(2) => "XIGPolarisVideoMedia",
        Some(8) => "XIGPolarisCarouselMedia",
        _ => return Err(invalid),
    };
    if wrapper.get("__typename").and_then(Value::as_str) != Some(expected_type)
        || post.get("__typename").and_then(Value::as_str) != Some(expected_type)
    {
        return Err(invalid);
    }
    let outer_pk = media_id(wrapper.get("pk"))?;
    if outer_pk.is_empty()
        || outer_pk.len() > 128
        || !outer_pk
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid);
    }
    if post.get("code").and_then(Value::as_str) != Some(source.media_id())
        || outer_pk != media_id(post.get("pk"))?
    {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    let outer_id = wrapper
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        .ok_or(invalid)?;
    if post.get("id").and_then(Value::as_str) != Some(outer_id) {
        return Err(AnalysisErrorCode::ContentChanged);
    }
    if expected_type == "XIGPolarisCarouselMedia" {
        let children = post
            .get("carousel_media")
            .and_then(Value::as_array)
            .ok_or(invalid)?;
        if children.is_empty() || children.len() > 32 {
            return Err(invalid);
        }
        for child in children {
            let child_type = match child.get("media_type").and_then(Value::as_u64) {
                Some(1) => "XIGPolarisImageMedia",
                Some(2) => "XIGPolarisVideoMedia",
                _ => return Err(invalid),
            };
            if child.get("__typename").and_then(Value::as_str) != Some(child_type) {
                return Err(invalid);
            }
        }
    } else if post.get("carousel_media").is_some() {
        return Err(invalid);
    }
    Ok(post)
}

pub(crate) fn normalize_web_post(
    source: &ValidatedMediaUrl,
    post: &Value,
) -> Result<InstagramInventory, AnalysisErrorCode> {
    validate_post_source(source)?;
    let graphql = post.get("shortcode").is_some() && post.get("code").is_none();
    let children = if graphql {
        if let Some(sidecar) = post.get("edge_sidecar_to_children") {
            let edges = sidecar
                .get("edges")
                .and_then(Value::as_array)
                .ok_or(AnalysisErrorCode::InvalidMetadata)?;
            check_count(sidecar.get("count"), edges.len())?;
            edges
                .iter()
                .map(|edge| edge.get("node").ok_or(AnalysisErrorCode::InvalidMetadata))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![post]
        }
    } else if let Some(carousel) = post.get("carousel_media") {
        let children = carousel
            .as_array()
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        check_count(post.get("carousel_media_count"), children.len())?;
        children.iter().collect()
    } else {
        vec![post]
    };
    if children.is_empty() || children.len() > 32 {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let title = if graphql {
        post.pointer("/edge_media_to_caption/edges/0/node/text")
    } else {
        post.pointer("/caption/text")
    }
    .and_then(Value::as_str);
    let creator = if graphql {
        post.pointer("/owner/username")
    } else {
        post.pointer("/user/username")
    }
    .and_then(Value::as_str);
    let mut ids = HashSet::new();
    let mut lines = Vec::with_capacity(children.len());
    for (index, item) in children.iter().enumerate() {
        let id = media_id(item.get(if graphql { "id" } else { "pk" }))?;
        if !ids.insert(id.clone()) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        let image = if graphql {
            choose_variant(
                item.get("display_resources"),
                "src",
                "config_width",
                "config_height",
            )?
            .or(direct_asset(item, "display_url")?)
        } else {
            choose_variant(
                item.pointer("/image_versions2/candidates"),
                "url",
                "width",
                "height",
            )?
        }
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let video = if graphql {
            let is_video = item
                .get("is_video")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    item.get("__typename")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.ends_with("GraphVideo"))
                });
            if is_video {
                Some(direct_asset(item, "video_url")?.ok_or(AnalysisErrorCode::InvalidMetadata)?)
            } else {
                if item.get("video_url").is_some_and(|url| !url.is_null()) {
                    return Err(AnalysisErrorCode::InvalidMetadata);
                }
                None
            }
        } else {
            match item.get("media_type").and_then(Value::as_u64) {
                Some(2) => Some(
                    choose_variant(item.get("video_versions"), "url", "width", "height")?
                        .ok_or(AnalysisErrorCode::InvalidMetadata)?,
                ),
                Some(1) => None,
                _ => return Err(AnalysisErrorCode::InvalidMetadata),
            }
        };
        let media = video.as_ref().unwrap_or(&image);
        let extension = asset_extension(&media.url)?;
        lines.push(
            serde_json::to_string(&json!({
                "category": "instagram",
                "type": "post",
                "post_shortcode": source.media_id(),
                "post_url": source.url(),
                "description": title,
                "username": creator,
                "num": index + 1,
                "media_id": id,
                "display_url": image.url,
                "video_url": video.as_ref().map(|asset| &asset.url),
                "width": media.width,
                "height": media.height,
                "extension": extension,
            }))
            .map_err(|_| AnalysisErrorCode::InvalidMetadata)?,
        );
    }
    normalize_instagram_inventory(source.media_id(), lines.join("\n").as_bytes())
}

fn validate_post_source(source: &ValidatedMediaUrl) -> Result<(), AnalysisErrorCode> {
    if source.platform() != Platform::Instagram
        || source.instagram_kind() != Some(InstagramUrlKind::Post)
    {
        Err(AnalysisErrorCode::UnsupportedMedia)
    } else {
        Ok(())
    }
}

fn check_count(count: Option<&Value>, actual: usize) -> Result<(), AnalysisErrorCode> {
    if count.is_some_and(|count| count.as_u64() != Some(actual as u64)) {
        Err(AnalysisErrorCode::InvalidMetadata)
    } else {
        Ok(())
    }
}

fn media_id(value: Option<&Value>) -> Result<String, AnalysisErrorCode> {
    let value = value.ok_or(AnalysisErrorCode::InvalidMetadata)?;
    match value {
        Value::String(id) => Ok(id.clone()),
        Value::Number(number) => number
            .as_u64()
            .map(|id| id.to_string())
            .ok_or(AnalysisErrorCode::InvalidMetadata),
        _ => Err(AnalysisErrorCode::InvalidMetadata),
    }
}

struct Asset {
    url: String,
    width: Option<u32>,
    height: Option<u32>,
}

fn dimensions(
    value: &Value,
    width: &str,
    height: &str,
) -> Result<(Option<u32>, Option<u32>), AnalysisErrorCode> {
    let w = value.get(width);
    let h = value.get(height);
    match (w, h) {
        (None, None) => Ok((None, None)),
        (Some(w), Some(h)) => {
            let w = w.as_u64().filter(|w| (1..=16_384).contains(w));
            let h = h.as_u64().filter(|h| (1..=16_384).contains(h));
            match (w, h) {
                (Some(w), Some(h)) => Ok((Some(w as u32), Some(h as u32))),
                _ => Err(AnalysisErrorCode::InvalidMetadata),
            }
        }
        _ => Err(AnalysisErrorCode::InvalidMetadata),
    }
}

fn direct_asset(item: &Value, key: &str) -> Result<Option<Asset>, AnalysisErrorCode> {
    let Some(value) = item.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let url = value.as_str().ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let (width, height) = match item.get("dimensions") {
        Some(dimensions_value) => dimensions(dimensions_value, "width", "height")?,
        None => (None, None),
    };
    Ok(Some(Asset {
        url: url.to_owned(),
        width,
        height,
    }))
}

fn choose_variant(
    value: Option<&Value>,
    url_key: &str,
    width_key: &str,
    height_key: &str,
) -> Result<Option<Asset>, AnalysisErrorCode> {
    let Some(value) = value else {
        return Ok(None);
    };
    let variants = value
        .as_array()
        .filter(|variants| variants.len() <= MAX_VARIANTS)
        .ok_or(AnalysisErrorCode::InvalidMetadata)?;
    let mut best: Option<Asset> = None;
    for variant in variants {
        let url = variant
            .get(url_key)
            .and_then(Value::as_str)
            .ok_or(AnalysisErrorCode::InvalidMetadata)?;
        let (width, height) = dimensions(variant, width_key, height_key)?;
        let area = u64::from(width.unwrap_or(0)) * u64::from(height.unwrap_or(0));
        if best.as_ref().is_none_or(|best| {
            area > u64::from(best.width.unwrap_or(0)) * u64::from(best.height.unwrap_or(0))
        }) {
            best = Some(Asset {
                url: url.to_owned(),
                width,
                height,
            });
        }
    }
    Ok(best)
}

fn asset_extension(value: &str) -> Result<String, AnalysisErrorCode> {
    let url = Url::parse(value).map_err(|_| AnalysisErrorCode::InvalidMetadata)?;
    url.path()
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| !extension.is_empty() && extension.len() <= 8)
        .ok_or(AnalysisErrorCode::InvalidMetadata)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use super::*;
    use crate::application::media_url::validate_media_url;

    #[test]
    fn loopback_bypass_reaches_the_origin_without_the_configured_proxy() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin_address = listener.local_addr().unwrap();
        let proxy_guard = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_address = proxy_guard.local_addr().unwrap();
        drop(proxy_guard);
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets can inherit nonblocking mode on Windows.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut request = [0u8; 4096];
                        let count = stream.read(&mut request).unwrap();
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                        return String::from_utf8(request[..count].to_vec()).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("local origin fixture failed: {error}"),
                }
            }
            String::new()
        });
        let proxies = crate::infrastructure::system_proxy::SystemProxies {
            http: Some(Url::parse(&format!("http://{proxy_address}")).unwrap()),
            https: None,
            bypass: Some("localhost,127.0.0.0/8,::1".to_owned()),
        };
        let origin_url = format!("http://{origin_address}/");
        let transport = ReqwestInstagramWebTransport::with_endpoints_and_proxy(
            origin_url.clone(),
            format!("http://{origin_address}/graphql/query"),
            &proxies,
        )
        .unwrap();
        let result = transport
            .client
            .get(origin_url)
            .send()
            .map_err(map_request_error)
            .and_then(|response| read_bounded_response(response, &CancellationToken::new()));
        let captured = server.join().unwrap();

        assert_eq!(result, Ok(b"{}".to_vec()));
        assert!(captured.starts_with("GET / HTTP/1.1"));
    }

    #[test]
    fn https_requests_use_connect_on_the_selected_local_proxy() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let proxy_address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut bytes = Vec::new();
                        let mut buffer = [0u8; 1024];
                        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                            let count = stream.read(&mut buffer).unwrap();
                            if count == 0 {
                                break;
                            }
                            bytes.extend_from_slice(&buffer[..count]);
                        }
                        stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        return String::from_utf8(bytes).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("local CONNECT fixture failed: {error}"),
                }
            }
            String::new()
        });
        let proxies = crate::infrastructure::system_proxy::SystemProxies {
            http: None,
            https: Some(Url::parse(&format!("http://{proxy_address}")).unwrap()),
            bypass: None,
        };
        let transport = ReqwestInstagramWebTransport::with_endpoints_and_proxy(
            "https://origin.invalid/".to_owned(),
            "https://origin.invalid/graphql/query".to_owned(),
            &proxies,
        )
        .unwrap();
        let source = validate_media_url("https://www.instagram.com/p/Browser_02/").unwrap();
        let result = transport.fetch_post(
            &build_graphql_request(&source, "").unwrap(),
            &CancellationToken::new(),
        );
        let captured = server.join().unwrap();

        assert!(captured.starts_with("CONNECT origin.invalid:443 HTTP/1.1"));
        assert_eq!(result, Err(AnalysisErrorCode::NetworkFailed));
    }

    #[test]
    fn configured_proxy_carries_anonymous_session_and_graphql_when_direct_origin_is_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let proxy_address = listener.local_addr().unwrap();
        let origin_guard = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_address = origin_guard.local_addr().unwrap();
        drop(origin_guard);
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut index = 0;
            while index < 2 && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("local proxy fixture failed: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    bytes.extend_from_slice(&buffer[..count]);
                    if count == 0 {
                        break;
                    }
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let header_end = end + 4;
                        let length = String::from_utf8_lossy(&bytes[..header_end])
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(str::trim)
                                    .map(str::to_owned)
                            })
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        if bytes.len() >= header_end + length {
                            break;
                        }
                    }
                }
                captured_tx.send(String::from_utf8(bytes).unwrap()).unwrap();
                if index == 0 {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nSet-Cookie: csrftoken=proxy_fixture; Path=/\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                } else {
                    let body = br#"{"items":[],"status":"ok"}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(body).unwrap();
                }
                index += 1;
            }
        });
        let proxies = crate::infrastructure::system_proxy::SystemProxies {
            http: Some(Url::parse(&format!("http://{proxy_address}")).unwrap()),
            https: Some(Url::parse(&format!("http://{proxy_address}")).unwrap()),
            bypass: None,
        };
        let transport = ReqwestInstagramWebTransport::with_endpoints_and_proxy(
            format!("http://{origin_address}/"),
            format!("http://{origin_address}/graphql/query"),
            &proxies,
        )
        .unwrap();
        let source = validate_media_url("https://www.instagram.com/p/Browser_02/").unwrap();
        let request = build_graphql_request(&source, "").unwrap();
        let result = transport.fetch_post(&request, &CancellationToken::new());
        server.join().unwrap();

        assert_eq!(result, Ok(br#"{"items":[],"status":"ok"}"#.to_vec()));
        let first = captured_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = captured_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(first.starts_with(&format!("GET http://{origin_address}/ HTTP/1.1")));
        assert!(second.starts_with(&format!(
            "POST http://{origin_address}/graphql/query HTTP/1.1"
        )));
        assert!(second
            .to_ascii_lowercase()
            .contains("x-csrftoken: proxy_fixture"));
        assert!(second.contains("doc_id=27128499623469141"));
    }

    #[test]
    fn native_transport_initializes_csrf_and_posts_the_closed_graphql_form() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
                    {
                        let header_end = header_end + 4;
                        let headers = String::from_utf8_lossy(&bytes[..header_end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(str::trim)
                                    .map(str::to_owned)
                            })
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        if bytes.len() >= header_end + length {
                            break;
                        }
                    }
                }
                captured_tx.send(String::from_utf8(bytes).unwrap()).unwrap();
                if index == 0 {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nSet-Cookie: csrftoken=csrf_fixture; Path=/; Secure\r\nSet-Cookie: mid=mid_fixture; Path=/; Secure\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                        .unwrap();
                } else {
                    let body = br#"{"items":[],"status":"ok"}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(body).unwrap();
                }
            }
        });
        let base = format!("http://{address}");
        let transport = ReqwestInstagramWebTransport::with_endpoints_for_test(
            format!("{base}/"),
            format!("{base}/graphql/query"),
        )
        .unwrap();
        let source = validate_media_url("https://www.instagram.com/p/Browser_02/").unwrap();
        let request = build_graphql_request(&source, "").unwrap();

        let response = transport
            .fetch_post(&request, &CancellationToken::new())
            .unwrap();

        assert_eq!(response, br#"{"items":[],"status":"ok"}"#);
        let first = captured_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = captured_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(first.starts_with("GET / HTTP/1.1"));
        assert!(first
            .to_ascii_lowercase()
            .contains("cookie: ig_pr=1; ig_vw=1920"));
        assert!(second.starts_with("POST /graphql/query HTTP/1.1"));
        let lower = second.to_ascii_lowercase();
        assert!(lower.contains("x-csrftoken: csrf_fixture"));
        assert!(lower.contains("x-ig-app-id: 936619743392459"));
        assert!(lower.contains("accept-language: en-us,en;q=0.8"));
        assert!(lower.contains("referer: https://www.instagram.com/p/browser_02/"));
        assert!(lower.contains("csrftoken=csrf_fixture"));
        assert!(lower.contains("mid=mid_fixture"));
        assert!(second.contains("doc_id=27128499623469141"));
        assert!(second.contains("server_timestamps=true"));
        assert!(second.contains("Browser_02"));
        server.join().unwrap();
    }

    #[test]
    fn response_body_timeout_remains_an_analysis_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 5\r\n\r\n",
                )
                .unwrap();
            thread::sleep(Duration::from_millis(200));
            let _ = stream.write_all(b"later");
        });
        let client = Client::builder()
            .timeout(Duration::from_millis(50))
            .no_proxy()
            .build()
            .unwrap();
        let response = client.get(format!("http://{address}/")).send().unwrap();

        assert_eq!(
            read_bounded_response(response, &CancellationToken::new()),
            Err(AnalysisErrorCode::AnalysisTimeout)
        );
        server.join().unwrap();
    }
}
