pub mod assets;
mod ffmpeg_http;
pub mod hls_local;
mod hls_probe;
pub(crate) mod http_session;
pub mod inventory;
pub mod manifest;
pub mod url_policy;
pub struct GalleryDlXAdapter<R, P> {
    pub(crate) resolver: R,
    pub(crate) process: P,
}
impl<R, P> GalleryDlXAdapter<R, P> {
    pub fn new(resolver: R, process: P) -> Self {
        Self { resolver, process }
    }
}
