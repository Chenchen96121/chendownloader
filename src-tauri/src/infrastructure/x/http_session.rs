//! Native-only session resources; never part of a frontend request or DTO.
use crate::{application::network_profile::NetworkProfile, domain::media::AnalysisErrorCode};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

#[derive(Clone, Copy)]
pub(crate) enum XHttpPolicy {
    Manifest,
    Segment,
}

type ClientSlot = OnceLock<Result<Arc<reqwest::Client>, AnalysisErrorCode>>;
#[derive(Default)]
pub(crate) struct XHttpClients {
    pub(super) manifest: ClientSlot,
    pub(super) segment: ClientSlot,
    pub(super) ffmpeg: OnceLock<super::ffmpeg_http::FfmpegHttpTool>,
}

// Registration is session-local and optional. Fixture or mismatched resolver
// versions never become a native executable fallback.
pub(crate) fn register_x_http_ffmpeg(
    network: &NetworkProfile,
    tool: crate::application::tool_resolver::ResolvedTool,
) {
    if let Some(tool) = super::ffmpeg_http::FfmpegHttpTool::from_resolved(tool) {
        let _ = network.x_video_http.ffmpeg.set(tool);
    }
}

pub(crate) fn x_video_http_client(
    network: &NetworkProfile,
    policy: XHttpPolicy,
) -> Result<Arc<reqwest::Client>, AnalysisErrorCode> {
    let slot = match policy {
        XHttpPolicy::Manifest => &network.x_video_http.manifest,
        XHttpPolicy::Segment => &network.x_video_http.segment,
    };
    slot.get_or_init(|| build_client(network, policy)).clone()
}

fn build_client(
    network: &NetworkProfile,
    policy: XHttpPolicy,
) -> Result<Arc<reqwest::Client>, AnalysisErrorCode> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(2);
    if matches!(policy, XHttpPolicy::Segment) {
        builder = builder
            .connect_timeout(Duration::from_secs(8))
            .read_timeout(Duration::from_secs(8));
    }
    if let Some(proxy) = network.https_proxy_for_host("video.twimg.com") {
        builder = builder.proxy(
            reqwest::Proxy::https(proxy.as_str())
                .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?,
        );
    }
    builder
        .build()
        .map(Arc::new)
        .map_err(|_| AnalysisErrorCode::NetworkFailed)
}

#[cfg(test)]
mod fallback_registration_tests {
    use super::*;
    use crate::{
        application::{portable_package::EMBEDDED_TOOL_MANIFEST, tool_resolver::ResolvedTool},
        domain::managed_tools::{ManagedToolManifest, ToolName},
        infrastructure::system_proxy::SystemProxies,
    };

    #[test]
    fn registration_ignores_fixture_versions_and_is_shared_only_by_profile_clones() {
        let profile = SystemProxies::default().snapshot();
        register_x_http_ffmpeg(
            &profile,
            ResolvedTool {
                name: ToolName::Ffmpeg,
                version: "fixture".into(),
                path: "never-spawn.exe".into(),
            },
        );
        assert!(profile.x_video_http.ffmpeg.get().is_none());
        let manifest: ManagedToolManifest = serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).unwrap();
        let version = manifest
            .tools
            .iter()
            .find(|t| t.name == ToolName::Ffmpeg)
            .unwrap()
            .version
            .clone();
        register_x_http_ffmpeg(
            &profile,
            ResolvedTool {
                name: ToolName::Ffmpeg,
                version,
                path: "must-revalidate-before-spawn.exe".into(),
            },
        );
        assert!(profile.clone().x_video_http.ffmpeg.get().is_some());
        assert!(SystemProxies::default()
            .snapshot()
            .x_video_http
            .ffmpeg
            .get()
            .is_none());
        assert!(profile.x_video_http.manifest.get().is_none());
        assert!(profile.x_video_http.segment.get().is_none());
    }
}

#[cfg(test)]
#[path = "http_session_tests.rs"]
mod tests;
