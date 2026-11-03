use tauri::{AppHandle, State};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnalyzeBilibiliRequest {
    url: String,
    intent: SelectionIntent,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewBilibiliRequest {
    analysis_id: String,
    intent: SelectionIntent,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BilibiliResolutionRequest {
    analysis_id: String,
    mode: crate::domain::format_selection::SelectionMode,
    compatibility_mode: bool,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BilibiliAnalysisRequest {
    analysis_id: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartBilibiliDownloadRequest {
    analysis_id: String,
    destination_id: String,
    intent: SelectionIntent,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BilibiliJobRequest {
    job_id: String,
}

#[tauri::command]
pub async fn analyze_bilibili(
    request: AnalyzeBilibiliRequest,
    state: State<'_, AppState>,
) -> Result<crate::domain::bilibili::BilibiliAnalysisPreview, AnalysisErrorCode> {
    let runtime = state.bilibili_analysis_handle();
    let readiness = state.bilibili_analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.analyze(&request.url, request.intent)
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}
#[tauri::command]
pub async fn preview_bilibili_selection(
    request: PreviewBilibiliRequest,
    state: State<'_, AppState>,
) -> Result<crate::domain::bilibili::BilibiliAnalysisPreview, AnalysisErrorCode> {
    let runtime = state.bilibili_analysis_handle();
    let readiness = state.bilibili_analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.preview(&request.analysis_id, request.intent)
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}
#[tauri::command]
pub async fn get_bilibili_resolution_options(
    request: BilibiliResolutionRequest,
    state: State<'_, AppState>,
) -> Result<crate::application::live_analysis::ResolutionOptions, AnalysisErrorCode> {
    let runtime = state.bilibili_analysis_handle();
    let readiness = state.bilibili_analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.resolution_options(
            &request.analysis_id,
            request.mode,
            request.compatibility_mode,
        )
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}
#[tauri::command]
pub async fn get_bilibili_cover(
    request: BilibiliAnalysisRequest,
    state: State<'_, AppState>,
) -> Result<crate::domain::bilibili::BilibiliCoverPayload, AnalysisErrorCode> {
    let runtime = state.bilibili_analysis_handle();
    let readiness = state.bilibili_analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.cover(&request.analysis_id)
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}
#[tauri::command]
pub fn open_bilibili_source(
    request: BilibiliAnalysisRequest,
    state: State<'_, AppState>,
) -> Result<(), AnalysisErrorCode> {
    state.open_bilibili_source(&request.analysis_id)
}
#[tauri::command]
pub fn start_bilibili_download(
    request: StartBilibiliDownloadRequest,
    state: State<'_, AppState>,
) -> Result<crate::application::download_job::StartedDownload, DownloadErrorCode> {
    state.start_bilibili_download(
        &request.analysis_id,
        &request.destination_id,
        request.intent,
    )
}
#[tauri::command]
pub fn cancel_bilibili_download(
    request: BilibiliJobRequest,
    state: State<'_, AppState>,
) -> Result<(), DownloadErrorCode> {
    state.cancel_bilibili_download(&request.job_id)
}
#[tauri::command]
pub fn get_bilibili_download_result(
    request: BilibiliJobRequest,
    state: State<'_, AppState>,
) -> Result<crate::application::download_job::DownloadResult, DownloadErrorCode> {
    state.bilibili_download_result(&request.job_id)
}
#[tauri::command]
pub fn open_bilibili_completed_location(
    request: BilibiliJobRequest,
    state: State<'_, AppState>,
) -> Result<(), DownloadErrorCode> {
    state.open_bilibili_completed_location(&request.job_id)
}

#[tauri::command]
pub async fn analyze_x(
    request: AnalyzeXRequest,
    state: State<'_, AppState>,
) -> Result<crate::domain::x::XAnalysisPreview, AnalysisErrorCode> {
    let runtime = state.x_analysis_handle();
    let readiness = state.x_analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.analyze(&request.url)
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}

#[tauri::command]
pub async fn get_x_preview(
    request: GetXPreviewRequest,
    state: State<'_, AppState>,
) -> Result<crate::domain::x::XPreviewPayload, AnalysisErrorCode> {
    let runtime = state.x_analysis_handle();
    let readiness = state.x_analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.load_preview(&request.analysis_id, &request.item_id)
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}

#[tauri::command]
pub fn open_x_source(
    request: OpenXSourceRequest,
    state: State<'_, AppState>,
) -> Result<(), AnalysisErrorCode> {
    state.open_x_source(&request.analysis_id)
}

#[tauri::command]
pub fn start_x_download(
    request: StartXDownloadRequest,
    state: State<AppState>,
) -> Result<crate::application::x_download_job::StartedXDownload, DownloadErrorCode> {
    state.start_x_download(
        &request.analysis_id,
        &request.destination_id,
        request.intent,
    )
}

#[tauri::command]
pub fn cancel_x_download(
    request: XDownloadJobRequest,
    state: State<AppState>,
) -> Result<(), DownloadErrorCode> {
    state.cancel_x_download(&request.job_id)
}

#[tauri::command]
pub fn get_x_download_result(
    request: XDownloadJobRequest,
    state: State<AppState>,
) -> Result<crate::domain::x::XDownloadResult, DownloadErrorCode> {
    state.x_download_result(&request.job_id)
}

#[tauri::command]
pub fn open_x_completed_location(
    request: XDownloadJobRequest,
    state: State<AppState>,
) -> Result<(), DownloadErrorCode> {
    state.open_x_completed_location(&request.job_id)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnalyzeXRequest {
    url: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetXPreviewRequest {
    analysis_id: String,
    item_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenXSourceRequest {
    analysis_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartXDownloadRequest {
    analysis_id: String,
    destination_id: String,
    intent: crate::domain::x::XDownloadIntent,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct XDownloadJobRequest {
    job_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadConcurrencyRequest {
    limit: u8,
}
#[tauri::command]
pub fn get_download_queue(
    state: State<crate::runtime::AppState>,
) -> Result<
    crate::application::download_activity::DownloadQueueSnapshot,
    crate::domain::download::DownloadErrorCode,
> {
    state.download_queue()
}
#[tauri::command]
pub fn set_download_concurrency(
    request: DownloadConcurrencyRequest,
    state: State<crate::runtime::AppState>,
) -> Result<
    crate::application::download_activity::DownloadQueueSnapshot,
    crate::domain::download::DownloadErrorCode,
> {
    state.set_download_concurrency(request.limit)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnalyzeInstagramRequest {
    url: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetInstagramPreviewRequest {
    analysis_id: String,
    item_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenInstagramSourceRequest {
    analysis_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartInstagramDownloadRequest {
    analysis_id: String,
    destination_id: String,
    intent: crate::domain::instagram::InstagramDownloadIntent,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstagramDownloadJobRequest {
    job_id: String,
}

#[tauri::command]
pub fn read_clipboard_text(
    window: tauri::WebviewWindow,
) -> Result<String, crate::application::clipboard::ClipboardErrorCode> {
    if window.label() != "main" {
        return Err(crate::application::clipboard::ClipboardErrorCode::Unavailable);
    }
    crate::infrastructure::clipboard::read_clipboard_text()
}

use crate::{
    application::{
        download_job::{DownloadResult, StartedDownload},
        live_analysis::LiveAnalysisPreview,
        preview_selection::SelectionPreview,
    },
    domain::{
        download::{DestinationApproval, DownloadErrorCode},
        format_selection::SelectionIntent,
        managed_tools::ToolInventory,
        media::{AnalysisErrorCode, MediaInfo, Platform},
        runtime_status::RuntimeStatus,
        tool_provisioning::{ToolProvisioningErrorCode, ToolProvisioningStatus},
    },
    runtime::AppState,
};

#[tauri::command]
pub fn start_download(
    analysis_id: String,
    destination_id: String,
    intent: SelectionIntent,
    state: State<AppState>,
) -> Result<StartedDownload, DownloadErrorCode> {
    state.start_download(&analysis_id, &destination_id, intent)
}

#[tauri::command]
pub fn cancel_download(job_id: String, state: State<AppState>) -> Result<(), DownloadErrorCode> {
    state.cancel_download(&job_id)
}

#[tauri::command]
pub fn get_download_result(
    job_id: String,
    state: State<AppState>,
) -> Result<DownloadResult, DownloadErrorCode> {
    state.download_result(&job_id)
}

#[tauri::command]
pub fn open_completed_location(
    job_id: String,
    state: State<AppState>,
) -> Result<(), DownloadErrorCode> {
    state.open_completed_location(&job_id)
}

#[tauri::command]
pub async fn choose_destination(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<DestinationApproval>, DownloadErrorCode> {
    let destinations = state.destination_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = crate::infrastructure::destination::pick_native_destination(&app)?;
        destinations.approve_selected(selected)
    })
    .await
    .unwrap_or(Err(DownloadErrorCode::Unknown))
}

#[tauri::command]
pub fn get_tool_inventory(state: State<AppState>) -> ToolInventory {
    state.tool_inventory()
}

#[tauri::command]
pub fn get_runtime_status(state: State<AppState>) -> RuntimeStatus {
    RuntimeStatus::from_inventory(env!("CARGO_PKG_VERSION"), &state.tool_inventory())
}

#[tauri::command]
pub fn open_media_page(
    analysis_id: String,
    state: State<AppState>,
) -> Result<(), AnalysisErrorCode> {
    state.live_analysis_handle().open_media_page(&analysis_id)
}

#[tauri::command]
pub fn load_default_destination(
    state: State<AppState>,
) -> crate::application::destination_settings::DefaultDestinationStatus {
    state.destination_handle().load_default()
}

#[tauri::command]
pub fn approve_default_destination(
    state: State<AppState>,
) -> Result<DestinationApproval, DownloadErrorCode> {
    state.destination_handle().approve_default()
}

#[tauri::command]
pub async fn choose_default_destination(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<
    Option<crate::application::destination_settings::DefaultDestinationStatus>,
    DownloadErrorCode,
> {
    let destinations = state.destination_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = crate::infrastructure::destination::pick_native_destination(&app)?;
        destinations.choose_default(selected)
    })
    .await
    .unwrap_or(Err(DownloadErrorCode::Unknown))
}

#[tauri::command]
pub fn get_tool_provisioning_status(state: State<AppState>) -> ToolProvisioningStatus {
    state.tool_provisioning_status()
}

#[tauri::command]
pub async fn get_resolution_options(
    analysis_id: String,
    mode: crate::domain::format_selection::SelectionMode,
    compatibility_mode: Option<bool>,
    state: State<'_, AppState>,
) -> Result<crate::application::live_analysis::ResolutionOptions, AnalysisErrorCode> {
    let runtime = state.live_analysis_handle();
    tauri::async_runtime::spawn_blocking(move || {
        runtime.resolution_options(&analysis_id, mode, compatibility_mode.unwrap_or(false))
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::AnalysisUnavailable))
}

#[tauri::command]
pub async fn repair_managed_tools(
    state: State<'_, AppState>,
) -> Result<ToolProvisioningStatus, ToolProvisioningErrorCode> {
    let (runtime, gate) = state.repair_handles();
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = gate
            .lock()
            .map_err(|_| ToolProvisioningErrorCode::ActivationFailed)?;
        runtime.request_repair()
    })
    .await
    .unwrap_or(Err(ToolProvisioningErrorCode::ActivationFailed))
}

#[tauri::command]
pub fn preview_media_fixture(
    platform: Platform,
    state: State<AppState>,
) -> Result<MediaInfo, AnalysisErrorCode> {
    state.preview_media_fixture(platform)
}

#[tauri::command]
pub fn preview_media_selection_fixture(
    platform: Platform,
    intent: SelectionIntent,
    state: State<AppState>,
) -> Result<SelectionPreview, AnalysisErrorCode> {
    state.preview_media_selection_fixture(platform, intent)
}

#[tauri::command]
pub async fn analyze_media(
    url: String,
    intent: SelectionIntent,
    state: State<'_, AppState>,
) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
    let runtime = state.live_analysis_handle();
    tauri::async_runtime::spawn_blocking(move || runtime.analyze(&url, intent))
        .await
        .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}

#[tauri::command]
pub async fn preview_media_selection(
    analysis_id: String,
    intent: SelectionIntent,
    state: State<'_, AppState>,
) -> Result<LiveAnalysisPreview, AnalysisErrorCode> {
    let runtime = state.live_analysis_handle();
    tauri::async_runtime::spawn_blocking(move || runtime.preview(&analysis_id, intent))
        .await
        .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}

#[tauri::command]
pub async fn analyze_instagram(
    request: AnalyzeInstagramRequest,
    state: State<'_, AppState>,
) -> Result<crate::domain::instagram::InstagramAnalysisPreview, AnalysisErrorCode> {
    let runtime = state.instagram_analysis_handle();
    let readiness = state.tool_provisioning_status().analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.analyze(&request.url)
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}

#[tauri::command]
pub async fn get_instagram_preview(
    request: GetInstagramPreviewRequest,
    state: State<'_, AppState>,
) -> Result<crate::domain::instagram::InstagramPreviewPayload, AnalysisErrorCode> {
    let runtime = state.instagram_analysis_handle();
    let readiness = state.tool_provisioning_status().analysis_readiness();
    tauri::async_runtime::spawn_blocking(move || {
        readiness?;
        runtime.load_preview(&request.analysis_id, &request.item_id)
    })
    .await
    .unwrap_or(Err(AnalysisErrorCode::ProcessFailed))
}

#[tauri::command]
pub fn open_instagram_source(
    request: OpenInstagramSourceRequest,
    state: State<'_, AppState>,
) -> Result<(), AnalysisErrorCode> {
    state.open_instagram_source(&request.analysis_id)
}

#[tauri::command]
pub fn start_instagram_download(
    request: StartInstagramDownloadRequest,
    state: State<AppState>,
) -> Result<crate::application::instagram_download_job::StartedInstagramDownload, DownloadErrorCode>
{
    state.start_instagram_download(
        &request.analysis_id,
        &request.destination_id,
        request.intent,
    )
}

#[tauri::command]
pub fn cancel_instagram_download(
    request: InstagramDownloadJobRequest,
    state: State<AppState>,
) -> Result<(), DownloadErrorCode> {
    state.cancel_instagram_download(&request.job_id)
}

#[tauri::command]
pub fn get_instagram_download_result(
    request: InstagramDownloadJobRequest,
    state: State<AppState>,
) -> Result<crate::domain::instagram::InstagramDownloadResult, DownloadErrorCode> {
    state.instagram_download_result(&request.job_id)
}

#[tauri::command]
pub fn open_instagram_completed_location(
    request: InstagramDownloadJobRequest,
    state: State<AppState>,
) -> Result<(), DownloadErrorCode> {
    state.open_instagram_completed_location(&request.job_id)
}
