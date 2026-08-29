//! Machine-readable diagnostics shared by Tauri IPC and the command-line probe.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreIdentity {
    pub version: Option<String>,
    pub entry_exists: bool,
    pub shipped_dsh_package_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebviewRouteDiagnostic {
    pub state: String,
    pub core_compatibility: crate::service::core_compatibility::CoreCapabilitySummary,
    pub target_origin: String,
    pub reported_origin: Option<String>,
    pub loader_present: bool,
    pub title: String,
    pub observed_at_ms: u128,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceCheck {
    pub id: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceDiagnostic {
    pub state: String,
    pub core_compatibility: crate::service::core_compatibility::CoreCapabilitySummary,
    pub origin: String,
    pub loader_present: bool,
    pub transport_owns_host: bool,
    pub checks: Vec<SurfaceCheck>,
    pub failures: Vec<String>,
    pub observed_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSnapshot {
    pub active_core_tag: Option<String>,
    pub recorded_release_tag: Option<String>,
    pub recorded_source_commit: Option<String>,
    pub core_path: String,
    pub profile_path: String,
    pub core: CoreIdentity,
    pub core_compatibility: Option<crate::service::core_compatibility::CoreCapabilitySummary>,
    pub plugin_compatibility: crate::service::plugin::compatibility::CompatibilityReport,
    pub webview_route: Option<WebviewRouteDiagnostic>,
    pub surface: Option<SurfaceDiagnostic>,
}

fn webview_diagnostic_path(app_data: &Path) -> PathBuf {
    app_data.join("diagnostics/webview-route.json")
}

fn surface_diagnostic_path(app_data: &Path) -> PathBuf {
    app_data.join("diagnostics/webview-surface.json")
}

fn read_surface_diagnostic(app_data: &Path) -> Option<SurfaceDiagnostic> {
    std::fs::read_to_string(surface_diagnostic_path(app_data))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn read_webview_route(app_data: &Path) -> Option<WebviewRouteDiagnostic> {
    std::fs::read_to_string(webview_diagnostic_path(app_data))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn redacted_origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port_or_known_default()?;
    Some(format!("{}://{host}:{port}", parsed.scheme()))
}

/// Record a redacted end-to-end frame observation for CLI diagnostics.
pub fn record_webview_route(
    app_handle: &AppHandle,
    state: &str,
    reported_url: &str,
    loader_present: bool,
    title: &str,
) -> Result<WebviewRouteDiagnostic, String> {
    if !matches!(state, "ready" | "timeout" | "error") {
        return Err(format!("WEBVIEW_DIAGNOSTIC_INVALID_STATE: {state}"));
    }
    let port = crate::config::get_store_dat_setting(app_handle).port;
    let compatibility = crate::service::core_compatibility::CoreCompatibility::active(app_handle)?;
    let expected = compatibility
        .webview_url(port)
        .ok_or_else(|| "WEBVIEW_DIAGNOSTIC_TARGET_UNAVAILABLE".to_string())?;
    let target_origin = redacted_origin(&expected)
        .ok_or_else(|| "WEBVIEW_DIAGNOSTIC_TARGET_INVALID".to_string())?;
    let reported_origin = redacted_origin(reported_url);
    if state == "ready" && (!loader_present || reported_origin.as_deref() != Some(&target_origin)) {
        return Err("WEBVIEW_DIAGNOSTIC_FALSE_READY".to_string());
    }
    let observation = WebviewRouteDiagnostic {
        state: state.to_string(),
        core_compatibility: compatibility.capability_summary(),
        target_origin,
        reported_origin,
        loader_present,
        title: title.chars().take(160).collect(),
        observed_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    };
    let path = webview_diagnostic_path(&crate::config::get_base_dir(app_handle));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("WEBVIEW_DIAGNOSTIC_DIR: {error}"))?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&observation)
            .map_err(|error| format!("WEBVIEW_DIAGNOSTIC_SERIALIZE: {error}"))?,
    )
    .map_err(|error| format!("WEBVIEW_DIAGNOSTIC_WRITE: {error}"))?;
    Ok(observation)
}

/// 记录真实 DSH 子页面执行的功能面断言，供命令行和设置页读取。
pub fn record_surface_diagnostic(
    app_handle: &AppHandle,
    reported_origin: &str,
    loader_present: bool,
    transport_owns_host: bool,
    checks: Vec<SurfaceCheck>,
    failures: Vec<String>,
) -> Result<SurfaceDiagnostic, String> {
    let port = crate::config::get_store_dat_setting(app_handle).port;
    let compatibility = crate::service::core_compatibility::CoreCompatibility::active(app_handle)?;
    let expected = compatibility
        .webview_url(port)
        .ok_or_else(|| "SURFACE_DIAGNOSTIC_TARGET_UNAVAILABLE".to_string())?;
    let expected_origin = redacted_origin(&expected)
        .ok_or_else(|| "SURFACE_DIAGNOSTIC_TARGET_INVALID".to_string())?;
    let origin = redacted_origin(reported_origin)
        .ok_or_else(|| "SURFACE_DIAGNOSTIC_ORIGIN_INVALID".to_string())?;
    if origin != expected_origin || !loader_present {
        return Err("SURFACE_DIAGNOSTIC_FALSE_REPORT".to_string());
    }
    if checks.len() > 64 || failures.len() > 64 {
        return Err("SURFACE_DIAGNOSTIC_LIMIT".to_string());
    }
    let checks = checks
        .into_iter()
        .map(|check| SurfaceCheck {
            id: check.id.chars().take(120).collect(),
            ok: check.ok,
            detail: check.detail.chars().take(240).collect(),
        })
        .collect::<Vec<_>>();
    let failures = failures
        .into_iter()
        .map(|failure| failure.chars().take(240).collect())
        .collect::<Vec<_>>();
    let failed = checks.iter().any(|check| !check.ok) || !failures.is_empty();
    let report = SurfaceDiagnostic {
        state: if failed { "degraded" } else { "ready" }.to_string(),
        core_compatibility: compatibility.capability_summary(),
        origin,
        loader_present,
        transport_owns_host,
        checks,
        failures,
        observed_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    };
    let path = surface_diagnostic_path(&crate::config::get_base_dir(app_handle));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("SURFACE_DIAGNOSTIC_DIR: {error}"))?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&report)
            .map_err(|error| format!("SURFACE_DIAGNOSTIC_SERIALIZE: {error}"))?,
    )
    .map_err(|error| format!("SURFACE_DIAGNOSTIC_WRITE: {error}"))?;
    Ok(report)
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag != "."
        && tag != ".."
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn active_core_from_app_data(app_data: &Path) -> (Option<String>, PathBuf) {
    let dependencies = app_data.join("dependencies");
    let tag = std::fs::read_to_string(dependencies.join("active-core"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| valid_tag(value));
    if let Some(tag) = &tag {
        let slot = dependencies.join("cores").join(tag);
        if slot.join(crate::config::DSH_ENTRY_RELATIVE).is_file() {
            return (Some(tag.clone()), slot);
        }
    }
    (None, dependencies.join("dsh"))
}

fn recorded_provenance(app_data: &Path) -> (Option<String>, Option<String>) {
    let value = std::fs::read_to_string(app_data.join(".store.dat"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    let setting = value.as_ref().and_then(|value| value.get("setting"));
    let tag = setting
        .and_then(|value| value.get("dsh_pkg_tag"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let commit = setting
        .and_then(|value| value.get("dsh_pkg_commit"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    (tag, commit)
}

fn read_core_identity(core: &Path) -> CoreIdentity {
    let manifest = core.join("node_modules/@deepseek-ai/dsh/package.json");
    let version = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| value.get("version")?.as_str().map(str::to_string));
    let scope = core.join("node_modules/@deepseek-ai");
    let shipped_dsh_package_count = std::fs::read_dir(scope)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("dsh"))
        .count();
    CoreIdentity {
        version,
        entry_exists: core.join(crate::config::DSH_ENTRY_RELATIVE).is_file(),
        shipped_dsh_package_count,
    }
}

fn core_compatibility(
    core: &CoreIdentity,
) -> Option<crate::service::core_compatibility::CoreCapabilitySummary> {
    core.version
        .as_deref()
        .and_then(|version| {
            crate::service::core_compatibility::CoreCompatibility::resolve(version).ok()
        })
        .map(|compatibility| compatibility.capability_summary())
}

/// Build an offline snapshot without creating a WebView or starting DSH.
pub fn snapshot_from_roots(
    app_data: &Path,
    dsh_home: &Path,
    profile: &str,
) -> Result<DiagnosticsSnapshot, String> {
    if !valid_tag(profile) {
        return Err(format!("DIAGNOSTICS_INVALID_PROFILE: {profile}"));
    }
    let (active_core_tag, core_path) = active_core_from_app_data(app_data);
    let (recorded_release_tag, recorded_source_commit) = recorded_provenance(app_data);
    let profile_path = dsh_home.join("profiles").join(profile);
    let plugin_compatibility =
        crate::service::plugin::compatibility::inspect(&core_path, &profile_path)?;
    let core = read_core_identity(&core_path);
    Ok(DiagnosticsSnapshot {
        active_core_tag,
        recorded_release_tag,
        recorded_source_commit,
        core_path: core_path.to_string_lossy().into_owned(),
        profile_path: profile_path.to_string_lossy().into_owned(),
        core_compatibility: core_compatibility(&core),
        core,
        plugin_compatibility,
        webview_route: read_webview_route(app_data),
        surface: read_surface_diagnostic(app_data),
    })
}

/// Build a snapshot for an explicit core and profile without changing selection.
pub fn snapshot_for_paths(
    core_path: &Path,
    profile_path: &Path,
) -> Result<DiagnosticsSnapshot, String> {
    let core = read_core_identity(core_path);
    Ok(DiagnosticsSnapshot {
        active_core_tag: None,
        recorded_release_tag: None,
        recorded_source_commit: None,
        core_path: core_path.to_string_lossy().into_owned(),
        profile_path: profile_path.to_string_lossy().into_owned(),
        core_compatibility: core_compatibility(&core),
        core,
        plugin_compatibility: crate::service::plugin::compatibility::inspect(
            core_path,
            profile_path,
        )?,
        webview_route: None,
        surface: None,
    })
}

/// Build a snapshot for the running Desktop configuration.
pub fn active_snapshot(app_handle: &AppHandle) -> Result<DiagnosticsSnapshot, String> {
    let core_path = crate::service::core::active_core_dir(app_handle);
    let profile_path = crate::service::profile::profile_dir_of(
        app_handle,
        &crate::service::profile::active_profile(app_handle),
    );
    let setting = crate::config::get_store_dat_setting(app_handle);
    let core = read_core_identity(&core_path);
    Ok(DiagnosticsSnapshot {
        active_core_tag: crate::config::get_active_dsh_slot(app_handle),
        recorded_release_tag: setting.dsh_pkg_tag,
        recorded_source_commit: setting.dsh_pkg_commit,
        core_path: core_path.to_string_lossy().into_owned(),
        profile_path: profile_path.to_string_lossy().into_owned(),
        core_compatibility: core_compatibility(&core),
        core,
        plugin_compatibility: crate::service::plugin::compatibility::inspect(
            &core_path,
            &profile_path,
        )?,
        webview_route: read_webview_route(&crate::config::get_base_dir(app_handle)),
        surface: read_surface_diagnostic(&crate::config::get_base_dir(app_handle)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_missing_pointer_falls_back_to_legacy_core() {
        let root =
            std::env::temp_dir().join(format!("dsh-desktop-diagnostics-{}", std::process::id()));
        std::fs::create_dir_all(root.join("dependencies")).unwrap();
        std::fs::write(root.join("dependencies/active-core"), "../bad\n").unwrap();
        let (tag, core) = active_core_from_app_data(&root);
        assert_eq!(tag, None);
        assert_eq!(core, root.join("dependencies/dsh"));
    }
}
