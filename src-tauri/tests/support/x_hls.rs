//! Explicit network double for tests using fake probe/process outputs.
use local_media_downloader_lib::{
    application::x_source::XRequestContext,
    infrastructure::x::hls_local::{HlsByteRange, HlsSegmentTransport, HlsTransferError},
};
pub struct Segments;
impl HlsSegmentTransport for Segments {
    fn fetch(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        path: &std::path::Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError> {
        assert_eq!(url.host_str(), Some("video.twimg.com"));
        assert!(!context.cancellation.is_canceled());
        assert!(range.is_none());
        let bytes = b"explicit fake segment bytes";
        assert!(bytes.len() as u64 <= maximum);
        std::fs::write(path, bytes).unwrap();
        Ok(bytes.len() as u64)
    }
}
