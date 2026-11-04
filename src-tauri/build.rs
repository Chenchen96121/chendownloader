fn main() {
    println!("cargo:rerun-if-changed=resources/managed-tools/manifest.json");
    let target = std::env::var("TARGET").expect("Cargo must provide TARGET to the build script");
    println!("cargo:rustc-env=BUILD_TARGET={target}");

    println!("cargo:rerun-if-changed=icons/chen.ico");
    let icon_path = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("Cargo must provide CARGO_MANIFEST_DIR to the build script"),
    )
    .join("icons/chen.ico");

    let windows = if target.contains("windows-msvc") {
        tauri_build::WindowsAttributes::new_without_app_manifest()
    } else {
        tauri_build::WindowsAttributes::new()
    }
    .window_icon_path(icon_path);
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(windows)
            .app_manifest(tauri_build::AppManifest::new().commands(&[
                "read_clipboard_text",
                "analyze_bilibili",
                "preview_bilibili_selection",
                "get_bilibili_resolution_options",
                "get_bilibili_cover",
                "open_bilibili_source",
                "start_bilibili_download",
                "cancel_bilibili_download",
                "get_bilibili_download_result",
                "open_bilibili_completed_location",
                "analyze_x",
                "get_x_preview",
                "open_x_source",
                "start_x_download",
                "cancel_x_download",
                "get_x_download_result",
                "open_x_completed_location",
                "analyze_tiktok",
                "get_tiktok_preview",
                "open_tiktok_source",
                "start_tiktok_download",
                "cancel_tiktok_download",
                "get_tiktok_download_result",
                "open_tiktok_completed_location",
                "get_tiktok_runtime_status",
                "get_download_queue",
                "set_download_concurrency",
                "get_tool_inventory",
                "get_runtime_status",
                "get_tool_provisioning_status",
                "repair_managed_tools",
                "choose_destination",
                "load_default_destination",
                "choose_default_destination",
                "approve_default_destination",
                "get_resolution_options",
                "analyze_media",
                "start_download",
                "cancel_download",
                "get_download_result",
                "open_completed_location",
                "open_media_page",
                "preview_media_fixture",
                "preview_media_selection",
                "preview_media_selection_fixture",
                "analyze_instagram",
                "get_instagram_preview",
                "open_instagram_source",
                "start_instagram_download",
                "cancel_instagram_download",
                "get_instagram_download_result",
                "open_instagram_completed_location",
            ])),
    )
    .expect("failed to build Tauri application metadata");

    if target.contains("windows-msvc") {
        // `tauri-build` normally embeds the Common Controls v6 activation
        // manifest only in binary targets. The crate's unit-test harness is also
        // a Windows PE executable, but Cargo does not classify it as an explicit
        // `test` target for `rustc-link-arg-tests`. Suppress that one manifest in
        // the Tauri resource above, then provide it once to every final MSVC link;
        // Tauri's resource still supplies the application icon and version data.
        let manifest_path = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR to the build script"),
        )
        .join("common-controls-v6.manifest");
        std::fs::write(
            &manifest_path,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
"#,
        )
        .expect("failed to stage the Windows activation manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
            manifest_path.display()
        );
    }
}
