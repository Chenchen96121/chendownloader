//! Native-only immutable network settings, never accepted through IPC.
use crate::infrastructure::{system_proxy::SystemProxies, x::http_session::XHttpClients};
use std::sync::Arc;
use url::Url;

#[derive(Clone)]
pub struct NetworkProfile {
    pub(crate) proxies: SystemProxies,
    pub(crate) x_video_http: Arc<XHttpClients>,
}

impl NetworkProfile {
    pub fn current() -> Self {
        SystemProxies::current().snapshot()
    }
    pub fn https_proxy_for_host(&self, host: &str) -> Option<&Url> {
        self.proxies.https_for_host(host)
    }
}
