//! Desktop 管理插件的跨核心重绑定。
//!
//! 用户选择保存在应用设置中；profile 里的 `file:`/`link:` 只是当前核心协议世代
//! 的物理投影。核心切换后必须重新生成投影，再执行统一兼容性门禁。

use tauri::AppHandle;

use super::installed::is_installed;
use super::preset::provided_by_active_core;

/// 把内置插件与用户选择的社区预设重绑定到当前活动核心的精确制品集。
pub(crate) async fn rebind_for_active_core(app_handle: &AppHandle) -> Result<(), String> {
    let selected = crate::config::get_store_dat_setting(app_handle).managed_preset_plugins;

    // alpha.1 已原生提供该能力。移除旧核心留下的覆盖插件，但保留逻辑选择，切回
    // legacy 时会从对应离线产物重新安装。
    for id in &selected {
        if provided_by_active_core(app_handle, id) && is_installed(app_handle, id) {
            super::uninstall_recovery(app_handle, id)?;
        }
    }

    super::internal::ensure(app_handle).await?;
    if !selected.is_empty() {
        super::install::install(app_handle, &selected).await?;
    }
    super::compatibility::require_active_compatible(app_handle)
}
