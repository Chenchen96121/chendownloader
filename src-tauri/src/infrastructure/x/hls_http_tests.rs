use super::{
    retry_segment, HlsByteRange, HlsSegmentTransport, HlsTransferError, HttpHlsSegmentTransport,
    SegmentAttemptError,
};
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

// A real HTTP proxy boundary: HTTPS requests must first send CONNECT. Rejecting
// that tunnel avoids certificates, external DNS, and requests to the CDN.
struct RejectingConnectProxy {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<std::io::Result<Vec<String>>>>,
}

impl RejectingConnectProxy {
    fn new() -> Self {
        Self::with_reply(
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
    }

    fn with_reply(reply: &'static [u8]) -> Self {
        Self::with_delays(reply, vec![])
    }

    fn with_delays(reply: &'static [u8], delays: Vec<Duration>) -> Self {
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
                        if let Some(delay) = delays.get(requests.len() - 1) {
                            thread::sleep(*delay);
                        }
                        // A deadline test deliberately closes its socket first.
                        let _ = connection.write_all(reply);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        // Drain pending connections before honoring stop so an
                        // extra request cannot disappear during fixture cleanup.
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

#[test]
fn segment_retries_one_tls_unexpected_eof_through_the_same_captured_proxy() {
    // Complete CONNECT but close before TLS: the observed intermittent CDN
    // disconnect must not be flattened into a final error after one attempt.
    let proxy = RejectingConnectProxy::with_reply(b"HTTP/1.1 200 Connection Established\r\n\r\n");
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("segment.m4s");
    let result = HttpHlsSegmentTransport.fetch(
        &segment_url(),
        None,
        &destination,
        1024,
        &context(proxy.network()),
    );
    assert_eq!(
        result.err().map(|error| error.analysis_code()),
        Some(AnalysisErrorCode::NetworkFailed)
    );
    assert!(!destination.exists());
    assert_eq!(
        proxy.finish(),
        [
            "CONNECT video.twimg.com:443 HTTP/1.1",
            "CONNECT video.twimg.com:443 HTTP/1.1"
        ]
    );
}

impl Drop for RejectingConnectProxy {
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

fn segment_url() -> url::Url {
    url::Url::parse("https://video.twimg.com/fixture/segment.m4s").unwrap()
}

#[test]
fn segment_session_reuses_its_native_client_on_plain_threads_with_independent_deadlines() {
    let proxy = RejectingConnectProxy::with_delays(
        b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        vec![
            Duration::ZERO,
            Duration::from_millis(1200),
            Duration::from_millis(250),
        ],
    );
    let network = proxy.network();
    let root = tempfile::tempdir().unwrap();
    let mut first = context(network.clone());
    first.deadline = Instant::now() + Duration::from_secs(1);
    let fetch = |request: XRequestContext| {
        let path = root.path().join("unused.m4s");
        thread::spawn(move || {
            HttpHlsSegmentTransport.fetch(&segment_url(), None, &path, 1024, &request)
        })
        .join()
        .unwrap()
        .unwrap_err()
        .analysis_code()
    };
    assert_eq!(fetch(first.clone()), AnalysisErrorCode::NetworkFailed);
    assert!(
        network.x_video_http.segment.get().is_some(),
        "native GET must initialize the session's segment pool"
    );
    assert!(network.x_video_http.manifest.get().is_none());
    let client = network
        .x_video_http
        .segment
        .get()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    first.cancellation.cancel();
    let mut longer = context(network.clone());
    longer.deadline = Instant::now() + Duration::from_secs(3);
    assert_eq!(
        fetch(longer),
        AnalysisErrorCode::NetworkFailed,
        "first request timeout/cancellation must not stick to shared client"
    );
    assert!(Arc::ptr_eq(
        &client,
        network
            .x_video_http
            .segment
            .get()
            .unwrap()
            .as_ref()
            .unwrap()
    ));
    let mut shorter = context(network.clone());
    shorter.deadline = Instant::now() + Duration::from_millis(100);
    let started = Instant::now();
    assert_eq!(fetch(shorter), AnalysisErrorCode::AnalysisTimeout);
    assert!(started.elapsed() < Duration::from_millis(700));
    assert_eq!(proxy.finish().len(), 3);
    assert!(!root.path().join("unused.m4s").exists());
}

fn disconnected() -> SegmentAttemptError {
    SegmentAttemptError {
        error: AnalysisErrorCode::NetworkFailed.into(),
        retryable: true,
    }
}

#[test]
fn retry_removes_only_owned_partial_data_before_second_attempt() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("segment.m4s");
    let request = context(Arc::new(SystemProxies::default().snapshot()));
    let mut calls = 0;
    let result = retry_segment(&path, &request, |created| {
        calls += 1;
        assert!(!path.exists(), "retry must not reuse partial content");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        *created = true;
        if calls == 1 {
            file.write_all(b"partial").unwrap();
            Err(disconnected())
        } else {
            file.write_all(b"complete").unwrap();
            Ok(8)
        }
    });
    assert_eq!(result.unwrap(), 8);
    assert_eq!(calls, 2);
    assert_eq!(std::fs::read(&path).unwrap(), b"complete");
}

#[test]
fn second_disconnect_is_final_and_removes_partial_output() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("segment.m4s");
    let request = context(Arc::new(SystemProxies::default().snapshot()));
    let mut calls = 0;
    let result = retry_segment(&path, &request, |created| {
        calls += 1;
        assert!(!path.exists());
        std::fs::write(&path, b"partial").unwrap();
        *created = true;
        Err(disconnected())
    });
    assert_eq!(
        result.unwrap_err().analysis_code(),
        AnalysisErrorCode::NetworkFailed
    );
    assert_eq!(calls, 2);
    assert!(!path.exists());
}

#[test]
fn permanent_media_and_local_io_errors_do_not_retry_or_delete_existing_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("existing.m4s");
    std::fs::write(&path, b"user-owned").unwrap();
    let request = context(Arc::new(SystemProxies::default().snapshot()));
    for failure in [
        AnalysisErrorCode::AuthenticationRequired.into(),
        AnalysisErrorCode::AccessDenied.into(),
        AnalysisErrorCode::ContentUnavailable.into(),
        AnalysisErrorCode::RateLimited.into(),
        AnalysisErrorCode::InvalidMetadata.into(),
        AnalysisErrorCode::NetworkFailed.into(),
        HlsTransferError::Io(std::io::ErrorKind::StorageFull),
        HlsTransferError::Io(std::io::ErrorKind::AlreadyExists),
    ] {
        let mut failure = Some(failure);
        let mut calls = 0;
        let result = retry_segment(&path, &request, |_| {
            calls += 1;
            Err(failure.take().unwrap().into())
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"user-owned");
    }
}

#[test]
fn cancellation_between_attempts_cleans_up_without_another_request() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("segment.m4s");
    let request = context(Arc::new(SystemProxies::default().snapshot()));
    let mut calls = 0;
    let result = retry_segment(&path, &request, |created| {
        calls += 1;
        std::fs::write(&path, b"partial").unwrap();
        *created = true;
        request.cancellation.cancel();
        Err(disconnected())
    });
    assert_eq!(
        result.unwrap_err().analysis_code(),
        AnalysisErrorCode::ProcessFailed
    );
    assert_eq!(calls, 1);
    assert!(!path.exists());
}

#[test]
fn retry_never_refreshes_the_absolute_deadline() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("segment.m4s");
    let mut request = context(Arc::new(SystemProxies::default().snapshot()));
    request.deadline = Instant::now() + Duration::from_millis(10);
    let mut calls = 0;
    let result = retry_segment(&path, &request, |_| {
        calls += 1;
        thread::sleep(
            request.deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
        );
        Err(disconnected())
    });
    assert_eq!(
        result.unwrap_err().analysis_code(),
        AnalysisErrorCode::AnalysisTimeout
    );
    assert_eq!(calls, 1);
    assert!(!path.exists());
}

#[test]
fn failed_owned_partial_cleanup_stops_retrying() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("segment.m4s");
    let request = context(Arc::new(SystemProxies::default().snapshot()));
    let mut calls = 0;
    let result = retry_segment(&path, &request, |created| {
        calls += 1;
        std::fs::create_dir(&path).unwrap();
        *created = true;
        Err(disconnected())
    });
    assert!(matches!(result, Err(HlsTransferError::Io(_))));
    assert_eq!(calls, 1);
    assert!(path.is_dir());
}

#[test]
fn segment_fetch_uses_captured_https_proxy_and_leaves_no_file_when_connect_fails() {
    // Selecting the HTTP route or re-reading changed settings must miss the
    // captured HTTPS proxy and fail these boundary observations.
    let captured_proxy = RejectingConnectProxy::new();
    let other_proxy = RejectingConnectProxy::new();
    let mut settings = SystemProxies {
        http: Some(other_proxy.endpoint()),
        https: Some(captured_proxy.endpoint()),
        bypass: None,
    };
    let network = Arc::new(settings.snapshot());
    settings.https = Some(other_proxy.endpoint());
    settings.bypass = Some("*".to_owned());
    drop(settings);
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("segment.m4s");

    let result =
        HttpHlsSegmentTransport.fetch(&segment_url(), None, &destination, 1024, &context(network));

    assert_eq!(
        result.err().map(|error| error.analysis_code()),
        Some(AnalysisErrorCode::NetworkFailed)
    );
    assert!(!destination.exists());
    assert_eq!(
        captured_proxy.finish(),
        ["CONNECT video.twimg.com:443 HTTP/1.1"]
    );
    assert!(other_proxy.finish().is_empty());
}

#[test]
fn segment_fetch_rejects_unapproved_urls_before_connecting_or_creating_a_file() {
    // Removing URL policy enforcement must cause a proxy connection, even if
    // that request would ultimately fail for a separate transport reason.
    let proxy = RejectingConnectProxy::new();
    let request_context = context(proxy.network());
    let directory = tempfile::tempdir().unwrap();
    for (index, raw) in [
        "http://video.twimg.com/fixture/segment.m4s",
        "https://pbs.twimg.com/fixture/segment.m4s",
        "https://video.twimg.com.evil.test/fixture/segment.m4s",
        "https://user:secret@video.twimg.com/fixture/segment.m4s",
        "https://video.twimg.com:8443/fixture/segment.m4s",
        "https://video.twimg.com/fixture/segment.m4s#fragment",
        "https://video.twimg.com/fixture/%73egment.m4s",
    ]
    .iter()
    .enumerate()
    {
        let destination = directory.path().join(format!("segment-{index}.m4s"));
        let result = HttpHlsSegmentTransport.fetch(
            &url::Url::parse(raw).unwrap(),
            None,
            &destination,
            1024,
            &request_context,
        );
        assert_eq!(
            result.err().map(|error| error.analysis_code()),
            Some(AnalysisErrorCode::InvalidMetadata),
            "{raw}"
        );
        assert!(!destination.exists(), "{raw}");
    }
    assert!(proxy.finish().is_empty());
}

#[test]
fn segment_fetch_rejects_precancellation_before_connecting_or_creating_a_file() {
    let proxy = RejectingConnectProxy::new();
    let request_context = context(proxy.network());
    request_context.cancellation.cancel();
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("segment.m4s");

    let result =
        HttpHlsSegmentTransport.fetch(&segment_url(), None, &destination, 1024, &request_context);

    assert_eq!(
        result.err().map(|error| error.analysis_code()),
        Some(AnalysisErrorCode::ProcessFailed)
    );
    assert!(!destination.exists());
    assert!(proxy.finish().is_empty());
}

#[test]
fn segment_fetch_rejects_expired_deadline_before_connecting_or_creating_a_file() {
    let proxy = RejectingConnectProxy::new();
    let mut request_context = context(proxy.network());
    request_context.deadline = Instant::now() - Duration::from_secs(1);
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("segment.m4s");

    let result =
        HttpHlsSegmentTransport.fetch(&segment_url(), None, &destination, 1024, &request_context);

    assert_eq!(
        result.err().map(|error| error.analysis_code()),
        Some(AnalysisErrorCode::AnalysisTimeout)
    );
    assert!(!destination.exists());
    assert!(proxy.finish().is_empty());
}

#[test]
fn segment_fetch_rejects_invalid_byte_limits_before_connecting_or_creating_a_file() {
    let proxy = RejectingConnectProxy::new();
    let request_context = context(proxy.network());
    let directory = tempfile::tempdir().unwrap();
    for (index, maximum) in [0, 512 * 1024 * 1024 + 1, u64::MAX].into_iter().enumerate() {
        let destination = directory.path().join(format!("segment-{index}.m4s"));
        let result = HttpHlsSegmentTransport.fetch(
            &segment_url(),
            None,
            &destination,
            maximum,
            &request_context,
        );
        assert_eq!(
            result.err().map(|error| error.analysis_code()),
            Some(AnalysisErrorCode::InvalidMetadata),
            "maximum={maximum}"
        );
        assert!(!destination.exists(), "maximum={maximum}");
    }
    assert!(proxy.finish().is_empty());
}

#[test]
fn segment_fetch_rejects_invalid_ranges_before_connecting_or_creating_a_file() {
    // Zero-length ranges at nonzero offsets used to produce reversed Range
    // headers; overflow and a range exceeding its write budget are also invalid.
    let proxy = RejectingConnectProxy::new();
    let request_context = context(proxy.network());
    let directory = tempfile::tempdir().unwrap();
    for (index, range) in [
        HlsByteRange {
            offset: 1,
            length: 0,
        },
        HlsByteRange {
            offset: u64::MAX,
            length: 2,
        },
        HlsByteRange {
            offset: 0,
            length: 33,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let destination = directory.path().join(format!("segment-{index}.m4s"));
        let result = HttpHlsSegmentTransport.fetch(
            &segment_url(),
            Some(range),
            &destination,
            32,
            &request_context,
        );
        assert_eq!(
            result.err().map(|error| error.analysis_code()),
            Some(AnalysisErrorCode::InvalidMetadata),
            "range={range:?}"
        );
        assert!(!destination.exists(), "range={range:?}");
    }
    assert!(proxy.finish().is_empty());
}
