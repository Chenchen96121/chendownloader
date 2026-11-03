pub fn allow_main_navigation(url: &url::Url, development: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    (url.scheme() == "tauri" && url.host_str() == Some("localhost") && url.port().is_none())
        || (url.scheme() == "http"
            && url.host_str() == Some("tauri.localhost")
            && url.port().is_none())
        || (development
            && url.scheme() == "http"
            && url.host_str() == Some("localhost")
            && url.port() == Some(1420))
}
