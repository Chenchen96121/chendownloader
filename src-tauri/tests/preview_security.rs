use local_media_downloader_lib::application::preview_security::allow_main_navigation;

#[test]
fn privileged_root_can_only_navigate_to_the_exact_local_application_origin() {
    for url in ["http://tauri.localhost/", "tauri://localhost/index.html"] {
        assert!(allow_main_navigation(&url.parse().unwrap(), false));
    }
    for url in [
        "https://www.youtube-nocookie.com/embed/demoYT00001",
        "https://example.com",
        "file:///C:/private",
        "http://evil@tauri.localhost",
        "http://tauri.localhost:9000",
        "http://tauri.localhost.evil.invalid",
        "http://localhost:1420/",
    ] {
        assert!(!allow_main_navigation(&url.parse().unwrap(), false));
    }
    assert!(allow_main_navigation(
        &"http://localhost:1420/".parse().unwrap(),
        true
    ));
    assert!(!allow_main_navigation(
        &"http://localhost:1421/".parse().unwrap(),
        true
    ));
}
