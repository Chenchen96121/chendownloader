use std::sync::Arc;

use tauri::Manager;

use crate::{
    application::{portable_package::EMBEDDED_TOOL_MANIFEST, tool_provisioning::ToolProvisioner},
    domain::managed_tools::BUILD_TARGET,
    infrastructure::{
        process::WindowsProcessSupervisor,
        tools::{
            FilesystemManagedToolStore, FilesystemPortableToolPackage, NativeToolVersionProbe,
        },
    },
    runtime::provisioning::OwnedOfflineToolProvisioner,
};

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod ipc;
pub mod runtime;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_local_data = app.path().app_local_data_dir()?;
            let tool_store = Arc::new(FilesystemManagedToolStore::new(
                app_local_data.join("managed-tools"),
                BUILD_TARGET,
            ));
            let source = FilesystemPortableToolPackage::from_executable(
                &std::env::current_exe()?,
                BUILD_TARGET,
                Arc::from(EMBEDDED_TOOL_MANIFEST),
            );
            let process = Arc::new(WindowsProcessSupervisor);
            let probe = NativeToolVersionProbe::new(process);
            let provisioner: Arc<dyn ToolProvisioner + Send + Sync> =
                Arc::new(OwnedOfflineToolProvisioner::new(
                    source,
                    Arc::clone(&tool_store),
                    probe,
                    BUILD_TARGET,
                ));
            let state = runtime::AppState::with_app_handle(
                app_local_data,
                app.handle().clone(),
                tool_store,
                provisioner,
            );
            let _ = state.start_tool_provisioning();
            app.manage(state);
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .on_navigation(|url| {
                    application::preview_security::allow_main_navigation(
                        url,
                        cfg!(debug_assertions),
                    )
                })
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::commands::read_clipboard_text,
            ipc::commands::analyze_bilibili,
            ipc::commands::preview_bilibili_selection,
            ipc::commands::get_bilibili_resolution_options,
            ipc::commands::get_bilibili_cover,
            ipc::commands::open_bilibili_source,
            ipc::commands::start_bilibili_download,
            ipc::commands::cancel_bilibili_download,
            ipc::commands::get_bilibili_download_result,
            ipc::commands::open_bilibili_completed_location,
            ipc::commands::analyze_x,
            ipc::commands::get_x_preview,
            ipc::commands::open_x_source,
            ipc::commands::start_x_download,
            ipc::commands::cancel_x_download,
            ipc::commands::get_x_download_result,
            ipc::commands::open_x_completed_location,
            ipc::commands::analyze_tiktok,
            ipc::commands::get_tiktok_preview,
            ipc::commands::open_tiktok_source,
            ipc::commands::start_tiktok_download,
            ipc::commands::cancel_tiktok_download,
            ipc::commands::get_tiktok_download_result,
            ipc::commands::open_tiktok_completed_location,
            ipc::commands::get_tiktok_runtime_status,
            ipc::commands::get_download_queue,
            ipc::commands::set_download_concurrency,
            ipc::commands::get_tool_inventory,
            ipc::commands::get_runtime_status,
            ipc::commands::get_tool_provisioning_status,
            ipc::commands::repair_managed_tools,
            ipc::commands::choose_destination,
            ipc::commands::load_default_destination,
            ipc::commands::choose_default_destination,
            ipc::commands::approve_default_destination,
            ipc::commands::get_resolution_options,
            ipc::commands::analyze_media,
            ipc::commands::start_download,
            ipc::commands::cancel_download,
            ipc::commands::get_download_result,
            ipc::commands::open_completed_location,
            ipc::commands::open_media_page,
            ipc::commands::preview_media_fixture,
            ipc::commands::preview_media_selection,
            ipc::commands::preview_media_selection_fixture,
            ipc::commands::analyze_instagram,
            ipc::commands::get_instagram_preview,
            ipc::commands::open_instagram_source,
            ipc::commands::start_instagram_download,
            ipc::commands::cancel_instagram_download,
            ipc::commands::get_instagram_download_result,
            ipc::commands::open_instagram_completed_location
        ])
        .build(tauri::generate_context!())
        .expect("failed to build chendownloader")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                if let Some(state) = app.try_state::<runtime::AppState>() {
                    state.shutdown();
                }
            }
        });
}
