use super::{manifest_remote_error, HlsManifestTransport, HttpXManifestTransport};
use crate::{
    application::{
        network_profile::NetworkProfile, process::CancellationToken, x_source::XRequestContext,
    },
    domain::media::AnalysisErrorCode,
    infrastructure::system_proxy::SystemProxies,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

// A local CONNECT proxy exercises the native HTTPS client without external DNS,
// certificates, or CDN requests. A successful tunnel followed by close produces
// the same typed TLS EOF as the observed manifest disconnect.
struct ConnectProxy {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<std::io::Result<Vec<String>>>>,
}
impl ConnectProxy {
    fn new(reply: &'static [u8]) -> Self {
        Self::with_action(reply, None, Duration::ZERO)
    }
    fn with_action(
        reply: &'static [u8],
        cancellation: Option<CancellationToken>,
        delay: Duration,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            loop {
                match listener.accept() {
                    Ok((mut connection, _)) => {
                        connection.set_nonblocking(false)?;
                        connection.set_read_timeout(Some(Duration::from_secs(2)))?;
                        connection.set_write_timeout(Some(Duration::from_secs(2)))?;
                        let mut header = Vec::new();
                        let mut buffer = [0_u8; 1024];
                        while !header.windows(4).any(|window| window == b"\r\n\r\n") {
                            let count = connection.read(&mut buffer)?;
                            if count == 0 || header.len() + count > 8192 {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "missing or oversized proxy CONNECT header",
                                ));
                            }
                            header.extend_from_slice(&buffer[..count]);
                        }
                        requests.push(
                            String::from_utf8_lossy(&header)
                                .lines()
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                        );
                        if let Some(token) = &cancellation {
                            token.cancel();
                        }
                        if !delay.is_zero() {
                            thread::sleep(delay);
                        }
                        // A request may time out while this local fixture is
                        // delayed; its closed client socket is then expected.
                        let _ = connection.write_all(reply);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if worker_stop.load(Ordering::Acquire) {
                            return Ok(requests);
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => return Err(error),
                }
            }
        });
        Self {
            address,
            stop,
            worker: Some(worker),
        }
    }
    fn endpoint(&self) -> url::Url {
        url::Url::parse(&format!("http://{}/", self.address)).unwrap()
    }
    fn network(&self) -> Arc<NetworkProfile> {
        Arc::new(
            SystemProxies {
                https: Some(self.endpoint()),
                ..SystemProxies::default()
            }
            .snapshot(),
        )
    }
    fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap().unwrap()
    }
}
impl Drop for ConnectProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn context(network: Arc<NetworkProfile>) -> XRequestContext {
    XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(5),
        network,
    )
}
fn manifest_url() -> url::Url {
    url::Url::parse("https://video.twimg.com/fixture/rendition.m3u8").unwrap()
}
const CONNECT_OK: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

#[test]
fn manifest_session_native_read_initializes_only_its_own_role_and_survives_failure() {
    let proxy = ConnectProxy::new(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
    let network = proxy.network();
    for _ in 0..2 {
        let request = context(network.clone());
        let result = thread::spawn(move || HttpXManifestTransport.read(&manifest_url(), &request))
            .join()
            .unwrap();
        assert_eq!(result, Err(AnalysisErrorCode::NetworkFailed));
        assert!(
            network.x_video_http.manifest.get().is_some(),
            "native read must initialize the session manifest pool"
        );
        assert!(network.x_video_http.segment.get().is_none());
    }
    assert_eq!(proxy.finish().len(), 2);
}

#[test]
fn manifest_retries_one_tls_eof_using_the_same_captured_https_proxy() {
    let proxy = ConnectProxy::new(CONNECT_OK);
    let other_proxy = ConnectProxy::new(CONNECT_OK);
    let mut settings = SystemProxies {
        http: Some(other_proxy.endpoint()),
        https: Some(proxy.endpoint()),
        bypass: None,
    };
    let network = Arc::new(settings.snapshot());
    settings.https = Some(other_proxy.endpoint());
    settings.bypass = Some("*".into());
    drop(settings);

    assert_eq!(
        HttpXManifestTransport.read(&manifest_url(), &context(network)),
        Err(AnalysisErrorCode::NetworkFailed)
    );
    assert_eq!(
        proxy.finish(),
        [
            "CONNECT video.twimg.com:443 HTTP/1.1",
            "CONNECT video.twimg.com:443 HTTP/1.1",
        ],
        "a transient disconnect gets exactly one retry through the captured route"
    );
    assert!(other_proxy.finish().is_empty());
}

#[test]
fn manifest_does_not_retry_rejected_connect_statuses() {
    for reply in [
        b"HTTP/1.1 301 Moved Permanently\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n".as_slice(),
    ] {
        let proxy = ConnectProxy::new(reply);
        assert_eq!(
            HttpXManifestTransport.read(&manifest_url(), &context(proxy.network())),
            Err(AnalysisErrorCode::NetworkFailed)
        );
        assert_eq!(proxy.finish(), ["CONNECT video.twimg.com:443 HTTP/1.1"]);
    }
}

#[test]
fn manifest_rejects_invalid_urls_before_connecting() {
    let proxy = ConnectProxy::new(CONNECT_OK);
    let request = context(proxy.network());
    for raw in [
        "http://video.twimg.com/fixture/rendition.m3u8",
        "https://pbs.twimg.com/fixture/rendition.m3u8",
        "https://video.twimg.com.evil.test/fixture/rendition.m3u8",
        "https://user:secret@video.twimg.com/fixture/rendition.m3u8",
        "https://video.twimg.com:8443/fixture/rendition.m3u8",
    ] {
        assert_eq!(
            HttpXManifestTransport.read(&raw.parse().unwrap(), &request),
            Err(AnalysisErrorCode::InvalidMetadata),
            "{raw}"
        );
    }
    assert!(proxy.finish().is_empty());
    assert!(request.network.x_video_http.manifest.get().is_none());
}

#[test]
fn manifest_cancellation_prevents_initial_and_retry_requests() {
    let proxy = ConnectProxy::new(CONNECT_OK);
    let request = context(proxy.network());
    request.cancellation.cancel();
    assert_eq!(
        HttpXManifestTransport.read(&manifest_url(), &request),
        Err(AnalysisErrorCode::ProcessFailed)
    );
    assert!(proxy.finish().is_empty());

    let cancellation = CancellationToken::new();
    let proxy = ConnectProxy::with_action(CONNECT_OK, Some(cancellation.clone()), Duration::ZERO);
    let mut request = context(proxy.network());
    request.cancellation = cancellation;
    assert_eq!(
        HttpXManifestTransport.read(&manifest_url(), &request),
        Err(AnalysisErrorCode::ProcessFailed)
    );
    assert_eq!(proxy.finish(), ["CONNECT video.twimg.com:443 HTTP/1.1"]);
}

#[test]
fn manifest_retry_does_not_restart_the_original_context_deadline() {
    let proxy = ConnectProxy::with_action(CONNECT_OK, None, Duration::from_millis(100));
    let mut request = context(proxy.network());
    request.deadline = Instant::now() + Duration::from_millis(60);
    assert_eq!(
        HttpXManifestTransport.read(&manifest_url(), &request),
        Err(AnalysisErrorCode::AnalysisTimeout)
    );
    assert_eq!(proxy.finish(), ["CONNECT video.twimg.com:443 HTTP/1.1"]);
}

#[test]
fn manifest_remote_body_errors_preserve_wrapped_typed_causes() {
    use std::io::{Error, ErrorKind};
    // blocking Response::read wraps its underlying remote error in an Other
    // io::Error. Error::source skips that immediate payload, so inspecting only
    // source() loses a typed disconnect at this exact boundary.
    for kind in [
        ErrorKind::UnexpectedEof,
        ErrorKind::ConnectionReset,
        ErrorKind::ConnectionAborted,
        ErrorKind::BrokenPipe,
        ErrorKind::TimedOut,
    ] {
        let wrapped = Error::other(Error::new(kind, "remote body failure"));
        let failure = manifest_remote_error(&wrapped);
        assert!(failure.retryable, "must retain wrapped {kind:?}");
        assert_eq!(
            failure.error,
            if kind == ErrorKind::TimedOut {
                AnalysisErrorCode::AnalysisTimeout
            } else {
                AnalysisErrorCode::NetworkFailed
            }
        );
    }
    for kind in [
        ErrorKind::InvalidData,
        ErrorKind::PermissionDenied,
        ErrorKind::Other,
    ] {
        let wrapped = Error::other(Error::new(kind, "unexpected EOF connection reset"));
        assert!(
            !manifest_remote_error(&wrapped).retryable,
            "text alone must not make {kind:?} retryable"
        );
    }
}
