//! Desktop 管理插件的跨内核重绑定。
//!
//! 用户选择保存在应用设置中；profile 里的 `file:`/`link:` 只是当前内核协议世代
//! 的物理投影。内核切换后必须重新生成投影，再执行统一兼容性门禁。

use std::collections::HashMap;
use std::time::Instant;
use tauri::AppHandle;

use super::installed::{installed_name, is_installed, profile_dir, ProfilePackageJson};
use super::preset::{
    load_presets, preset_plugin_artifact_dir, provided_by_active_core, stage_preset_plugin,
};

/// 把内置插件与用户选择的社区预设重绑定到当前活动内核的精确制品集。
pub(crate) async fn rebind_for_active_core(app_handle: &AppHandle) -> Result<(), String> {
    let started = Instant::now();
    let selected = crate::config::get_store_dat_setting(app_handle).managed_preset_plugins;

    // alpha.1 已原生提供该能力。移除旧内核留下的覆盖插件，但保留逻辑选择，切回
    // legacy 时会从对应离线产物重新安装。
    for id in &selected {
        if provided_by_active_core(app_handle, id) && is_installed(app_handle, id) {
            super::uninstall_recovery(app_handle, id)?;
        }
    }

    super::internal::ensure(app_handle).await?;

    // 用户选择是逻辑状态，profile 依赖是当前内核制品集的物理投影。只有投影缺失
    // 或仍指向另一制品集时才执行 dsh/pnpm；热启动保持完整性核对但不重复安装。
    let profile = profile_dir(app_handle);
    let dependencies = std::fs::read_to_string(profile.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<ProfilePackageJson>(&raw).ok())
        .map(|manifest| manifest.dependencies)
        .unwrap_or_default();
    let presets = load_presets(app_handle);
    let preset_map: HashMap<&str, _> = presets
        .iter()
        .map(|preset| (preset.id.as_str(), preset))
        .collect();
    let mut install = Vec::new();
    for id in &selected {
        if provided_by_active_core(app_handle, id) {
            continue;
        }
        let Some(preset) = preset_map.get(id.as_str()) else {
            install.push(id.clone());
            continue;
        };
        let entry_ready = profile
            .join("node_modules")
            .join(installed_name(preset))
            .join("package.json")
            .is_file();
        let projection_ready = match preset_plugin_artifact_dir(app_handle, id) {
            Some(source) => {
                super::compatibility::require_packaged_plugin_compatible(
                    &crate::service::core::active_core_dir(app_handle),
                    &source,
                )?;
                let expected = stage_preset_plugin(&profile, id, &source)?;
                dependencies
                    .get(installed_name(preset))
                    .is_some_and(|actual| super::internal::dep_matches_spec(actual, &expected))
            }
            None => is_installed(app_handle, id),
        };
        if !entry_ready || !projection_ready {
            install.push(id.clone());
        }
    }
    if !install.is_empty() {
        log::info!("MANAGED_PLUGIN_REBIND: installing projections for {install:?}");
        super::install::install(app_handle, &install).await?;
    } else {
        log::info!(
            "MANAGED_PLUGIN_CACHE_HIT: {} selected plugin(s) already match the active artifact set",
            selected.len()
        );
    }
    super::compatibility::require_active_compatible(app_handle).map(|()| {
        log::info!(
            "STARTUP_PHASE managed_plugins duration_ms={} selected={} installed={}",
            started.elapsed().as_millis(),
            selected.len(),
            install.len()
        )
    })
}
