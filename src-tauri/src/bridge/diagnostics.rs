//! Read-only machine diagnostics commands.

use crate::diagnostics::DiagnosticsSnapshot;
use tauri::AppHandle;

#[tauri::command]
pub fn get_diagnostics_snapshot(app_handle: AppHandle) -> Result<DiagnosticsSnapshot, String> {
    crate::diagnostics::active_snapshot(&app_handle)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellDiagnosticPayload {
    stage: String,
    href: String,
    resource: String,
    message: String,
    root_child_count: usize,
}

/// 持久化不依赖 React 入口的顶层壳加载事件。
#[tauri::command]
pub fn report_shell_diagnostics(
    app_handle: AppHandle,
    payload: ShellDiagnosticPayload,
) -> Result<crate::diagnostics::ShellRuntimeDiagnostic, String> {
    crate::diagnostics::record_shell_runtime_event(
        &app_handle,
        &payload.stage,
        &payload.href,
        &payload.resource,
        &payload.message,
        payload.root_child_count,
    )
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameReadinessPayload {
    state: String,
    href: String,
    title: String,
    loader_present: bool,
}

/// Persist a redacted observation emitted by the real embedded DSH frame.
#[tauri::command]
pub fn report_frame_readiness(
    app_handle: AppHandle,
    payload: FrameReadinessPayload,
) -> Result<crate::diagnostics::WebviewRouteDiagnostic, String> {
    crate::diagnostics::record_webview_route(
        &app_handle,
        &payload.state,
        &payload.href,
        payload.loader_present,
        &payload.title,
    )
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceDiagnosticPayload {
    origin: String,
    loader_present: bool,
    transport_owns_host: bool,
    checks: Vec<crate::diagnostics::SurfaceCheck>,
    failures: Vec<String>,
}

/// 持久化真实子页面执行的功能面探针结果。
#[tauri::command]
pub fn report_surface_diagnostics(
    app_handle: AppHandle,
    payload: SurfaceDiagnosticPayload,
) -> Result<crate::diagnostics::SurfaceDiagnostic, String> {
    crate::diagnostics::record_surface_diagnostic(
        &app_handle,
        &payload.origin,
        payload.loader_present,
        payload.transport_owns_host,
        payload.checks,
        payload.failures,
    )
}
