//! 预打包内核的多版本管理：列出、切换、下载历史版本、卸载。
//!
//! 磁盘布局：每个发行版位于 `dependencies/cores/<tag>` 不可变槽位，活动 tag
//! 只保存在设置中。旧版 `dependencies/dsh` 与 `dependencies/<tag>` 仅用于升级
//! 发现，不再参与目录互换。本地内核探测见 [`super::local`]。

use crate::config;
use crate::service::{download, fs_guard, workflow};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use super::local::{find_user_dsh_bin, local_core};
use super::source::{active_source, CoreSource, HarnessCore};

/// `dependencies` 目录（激活 `dsh` 与历史 `dsh-<tag>` 槽位的共同父级）。
fn dependencies_dir(app_handle: &AppHandle) -> PathBuf {
    config::get_dependencies_path(app_handle)
}

/// 历史版本槽位（新命名）：`dependencies/<tag>`。release tag 本身以 `dsh-` 开头
/// （如 `dsh-0.1.0-rc.8-32331963388`），因此槽位目录名即 tag，不再叠加前缀。
fn slot_dir(app_handle: &AppHandle, tag: &str) -> PathBuf {
    config::get_dsh_slot_install_path(app_handle, tag)
}

/// 定位已下载的槽位：优先新命名 `dependencies/<tag>`，兼容旧版遗留的双前缀
/// `dependencies/dsh-<tag>`（tag 以 `dsh-` 开头时旧命名会产生 `dsh-dsh-...`）。
fn existing_slot_dir(app_handle: &AppHandle, tag: &str) -> Option<PathBuf> {
    let deps = dependencies_dir(app_handle);
    let slots = config::get_dsh_core_slots_path(app_handle);
    let new = safe_slot_path(&slots, tag).ok()?;
    if new.is_dir() {
        return Some(new);
    }
    let old = safe_slot_path(&deps, tag).ok()?;
    if old.is_dir() {
        return Some(old);
    }
    let legacy = safe_slot_path(&deps, &format!("dsh-{tag}")).ok()?;
    legacy.is_dir().then_some(legacy)
}

/// 构造槽位路径，并拒绝越出 dependencies 根目录的既有路径或符号链接。
fn safe_slot_path(deps: &Path, tag: &str) -> Result<PathBuf, String> {
    fs_guard::validate_id(tag)?;
    let path = deps.join(tag);
    if path.exists() {
        fs_guard::ensure_within(&path, deps)?;
    }
    Ok(path)
}

/// 读取发行版目录 `package.json` 中 `@deepseek-ai/dsh` 依赖版本（历史槽位展示用）。
fn read_manifest_dsh_version(dir: &Path) -> Option<String> {
    let package_manifest = dir
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("package.json");
    if let Ok(content) = std::fs::read_to_string(package_manifest) {
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        if let Some(version) = value.get("version").and_then(serde_json::Value::as_str) {
            if semver::Version::parse(version).is_ok() {
                return Some(version.to_string());
            }
        }
    }
    let content = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let version = v
        .get("dependencies")?
        .get("@deepseek-ai/dsh")?
        .as_str()
        .map(|s| s.trim_start_matches(['^', '~', '=', '>', '<']).to_string())?;
    semver::Version::parse(&version).ok().map(|_| version)
}

/// 内核列表：本地内核 + deepseek-harness-pkg 各发布版本（按版本去重）。
///
/// 版本行数据源为 GitHub tags（`fetch_dsh_pkg_tags`，最新在前）。pkg 仓库会对同一
/// 版本打多个 tag（含测试打包），这里按版本去重——同一版本只保留**最后一个** tag。
/// 激活的预打包版本不置顶，而是作为普通版本行按自然顺序排列并标记 `active`。
/// tags 拉取失败（离线/限流）时降级为磁盘扫描，只列出本地、激活与已下载的历史版本。
pub async fn list(app_handle: &AppHandle) -> Vec<HarnessCore> {
    let source = active_source(app_handle);
    let local = local_core(app_handle);
    let local_bin = local
        .as_ref()
        .map(|c| c.bin.to_string_lossy().into_owned())
        .or_else(|| find_user_dsh_bin(app_handle).map(|b| b.to_string_lossy().into_owned()));

    let mut rows: Vec<HarnessCore> = vec![HarnessCore {
        id: "local".to_string(),
        source: CoreSource::Local,
        version: local
            .as_ref()
            .map(|c| c.version.clone())
            .unwrap_or_default(),
        tag: String::new(),
        path: local_bin.clone().unwrap_or_default(),
        dir: local
            .as_ref()
            .map(|c| c.package_dir.to_string_lossy().into_owned())
            .unwrap_or_default(),
        present: local.is_some(),
        active: source == CoreSource::Local,
        error: None,
    }];

    // 激活的预打包信息：tag（可空，旧安装无记录）+ 安装目录状态
    // `active-core` 是运行时选择的唯一权威 tag。`dsh_pkg_tag` 只记录最近一次发行版
    // 下载来源，旧安装或本地打包可能保留另一版本，不能据此重命名活动槽位。
    let active_tag = active_app_tag(
        config::get_active_dsh_slot(app_handle),
        config::get_dsh_pkg_tag(app_handle),
    );
    let active_dir = config::get_dsh_install_path(app_handle);
    let active_present = config::get_dsh_binary_path(app_handle).exists();
    // 激活内核按「版本」而非 tag 匹配版本行：pkg 仓库会对同一版本重打包/打
    // 测试 tag，版本行去重后保留的 tag 未必等于本机安装时的记录 tag。按 tag
    // 精确匹配会让激活版本行误标「未下载」并在列表底部多出一条重复激活行。
    let active_version = if source == CoreSource::App {
        active_app_version(&active_tag, config::get_dsh_version(app_handle))
    } else {
        None
    };
    // 已安装的预打包版本号（无论当前以哪种来源运行都存在）：用于保证预打包行
    // 始终如实呈现为"已安装"，即便本次以本地内核运行，也不会把它标成"未下载"。
    let installed_version = config::get_dsh_version(app_handle);

    // 版本行：GitHub tags（最新在前）→ 按版本去重，同版本只保留最后一个 tag
    let tags = match download::fetch_dsh_pkg_tags().await {
        Ok(tags) => tags,
        Err(e) => {
            log::warn!(
                "Failed to fetch dsh pkg tags ({}), showing only on-disk versions",
                e
            );
            Vec::new()
        }
    };
    let mut version_tags: Vec<(String, String)> = Vec::new(); // (version, tag)，保持首次出现顺序
    for (tag, _commit) in &tags {
        let Some(version) = download::parse_version_from_tag(tag) else {
            continue;
        };
        if semver::Version::parse(&version).is_err() {
            continue;
        }
        if let Some(entry) = version_tags.iter_mut().find(|(v, _)| v == &version) {
            // 同版本重复（测试打包）：保留最后一个 tag
            entry.1 = tag.clone();
        } else {
            version_tags.push((version, tag.clone()));
        }
    }

    // 激活行就地标记：按版本匹配激活内核（不置顶，作为普通版本行标 active）
    let mut active_rendered = false;
    for (version, tag) in &version_tags {
        let is_active = active_version.as_deref() == Some(version.as_str());
        // 已安装的预打包内核：即使本次以本地内核运行（source=Local）也要如实标为
        // "已安装"，避免本地内核出现后预打包被当作未下载/消失（issue #54）。
        let is_installed = installed_version.as_deref() == Some(version.as_str());
        if is_active || is_installed {
            active_rendered = true;
        }
        let slot = existing_slot_dir(app_handle, tag);
        let present = if is_active || is_installed {
            active_present
        } else {
            slot.is_some()
        };
        let (path, dir) = if is_active || is_installed {
            let s = active_dir.to_string_lossy().into_owned();
            (s.clone(), s)
        } else if let Some(slot) = slot {
            let s = slot.to_string_lossy().into_owned();
            (s.clone(), s)
        } else {
            (String::new(), String::new())
        };
        rows.push(HarnessCore {
            id: format!("app-{tag}"),
            source: CoreSource::App,
            version: version.clone(),
            tag: tag.clone(),
            path,
            dir,
            present,
            active: is_active,
            error: None,
        });
    }

    // 已安装的预打包版本未出现在版本列表（离线/限流/tag 被移除/旧版无 tag 记录）：
    // 纳入版本行之后，保持列表不置顶；无论当前是否以本地内核运行都要列出，
    // 避免"本地内核出现后预打包消失"。
    if !active_rendered && active_present {
        rows.push(HarnessCore {
            id: active_tag
                .as_ref()
                .map(|t| format!("app-{t}"))
                .unwrap_or_else(|| "app".to_string()),
            source: CoreSource::App,
            version: installed_version.clone().unwrap_or_default(),
            tag: active_tag.clone().unwrap_or_default(),
            path: active_dir.to_string_lossy().into_owned(),
            dir: active_dir.to_string_lossy().into_owned(),
            present: true,
            active: source == CoreSource::App,
            error: None,
        });
    }

    // 磁盘扫描：tags 拉取失败/限流，或存在已下载但不在 tags 列表的版本（被移除的
    // 测试打包）时，把已下载的 `dsh-*` 槽位补进列表；同样按版本去重。
    // 激活版本已由版本行（或底部激活行）呈现，先放入 seen 避免扫描再补一条重复行。
    let mut seen_versions: HashSet<String> = version_tags.iter().map(|(v, _)| v.clone()).collect();
    if let Some(v) = &active_version {
        seen_versions.insert(v.clone());
    }
    if let Some(v) = &installed_version {
        seen_versions.insert(v.clone());
    }
    let scan_roots = [
        (config::get_dsh_core_slots_path(app_handle), false),
        (dependencies_dir(app_handle), true),
    ];
    for (root, legacy_layout) in scan_roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `cores/<tag>` 直接使用目录名。旧布局的 `dsh-dsh-*` 剥一层
            // 前缀；固定回退目录 `dependencies/dsh` 与其他依赖目录跳过。
            let tag = if !legacy_layout {
                Some(name.clone())
            } else if let Some(rest) = name.strip_prefix("dsh-dsh-") {
                Some(format!("dsh-{rest}"))
            } else if let Some(rest) = name.strip_prefix("dsh-") {
                if rest.is_empty() {
                    None
                } else {
                    Some(name.clone())
                }
            } else {
                None
            };
            let Some(tag) = tag else { continue };
            if !entry.path().is_dir() {
                continue;
            }
            let version = read_manifest_dsh_version(&entry.path())
                .or_else(|| download::parse_version_from_tag(&tag))
                .unwrap_or_default();
            if version.is_empty() {
                continue;
            }
            let dir = entry.path();
            if !seen_versions.insert(version.clone()) {
                // 远端同版本 tag 与本地制品 tag 可以不同（本地 pkg / 测试打包）。
                // 版本行尚未下载时，用磁盘上的真实 tag 和目录接管该行，确保切换
                // 指向确实存在的不可变槽位，而不是显示一个无法激活的远端 tag。
                if let Some(row) = rows.iter_mut().find(|row| {
                    row.source == CoreSource::App && row.version == version && !row.present
                }) {
                    row.id = format!("app-{tag}");
                    row.tag = tag;
                    row.path = dir.to_string_lossy().into_owned();
                    row.dir = row.path.clone();
                    row.present = true;
                }
                continue;
            }
            rows.push(HarnessCore {
                id: format!("app-{tag}"),
                source: CoreSource::App,
                version,
                tag,
                path: dir.to_string_lossy().into_owned(),
                dir: dir.to_string_lossy().into_owned(),
                present: true,
                active: false,
                error: None,
            });
        }
    }

    rows
}

/// 切换活动内核（只持久化选择；服务重启由前端负责）。
///
/// `id` 取值：`local` | `app`（无 tag 记录的旧激活行）| `app-<tag>`。
pub async fn set_active(app_handle: &AppHandle, id: &str) -> Result<HarnessCore, String> {
    validate_active_target(app_handle, id)?;
    let previous_setting = config::get_store_dat_setting(app_handle);
    let previous_pointer = config::get_active_dsh_slot(app_handle);
    let was_running = crate::service::workflow::has_owned_process();
    if was_running {
        crate::service::workflow::stop(app_handle.clone()).await?;
    }

    let selection_result = if id == "local" {
        let mut setting = config::get_store_dat_setting(app_handle);
        setting.active_core = Some(CoreSource::Local.as_str().to_string());
        config::set_store_dat_setting(app_handle, setting)
    } else if id == "app" {
        let mut setting = config::get_store_dat_setting(app_handle);
        setting.active_core = Some(CoreSource::App.as_str().to_string());
        config::set_store_dat_setting(app_handle, setting)
    } else if let Some(tag) = id.strip_prefix("app-") {
        switch_app_version(app_handle, tag).await
    } else {
        unreachable!("target was validated before stopping the service")
    };
    if let Err(error) = selection_result {
        return Err(rollback_active_switch(
            app_handle,
            previous_pointer.as_deref(),
            previous_setting,
            was_running,
            error,
        )
        .await);
    }

    if let Err(error) = crate::service::plugin::rebind_for_active_core(app_handle).await {
        return Err(rollback_active_switch(
            app_handle,
            previous_pointer.as_deref(),
            previous_setting,
            was_running,
            format!("CORE_SWITCH_PLUGIN_REBIND: {error}"),
        )
        .await);
    }

    Ok(list(app_handle)
        .await
        .into_iter()
        .find(|c| c.active)
        .ok_or_else(|| "CORE_NOT_FOUND: active core disappeared after switch".to_string())?)
}

fn validate_active_target(app_handle: &AppHandle, id: &str) -> Result<(), String> {
    if id == "local" {
        return local_core(app_handle)
            .map(|_| ())
            .ok_or_else(|| "CORE_LOCAL_NOT_FOUND: no local core detected".to_string());
    }
    if id == "app" {
        return config::get_dsh_binary_path(app_handle)
            .is_file()
            .then_some(())
            .ok_or_else(|| "CORE_APP_NOT_FOUND: bundled core is not installed".to_string());
    }
    let tag = id
        .strip_prefix("app-")
        .ok_or_else(|| format!("CORE_INVALID_ID: {id}"))?;
    fs_guard::validate_id(tag)?;
    let target = existing_slot_dir(app_handle, tag)
        .ok_or_else(|| format!("CORE_VERSION_NOT_DOWNLOADED: {tag}"))?;
    target
        .join(config::DSH_ENTRY_RELATIVE)
        .is_file()
        .then_some(())
        .ok_or_else(|| {
            format!(
                "CORE_VERSION_INVALID: dsh entry is missing from {}",
                target.display()
            )
        })
}

async fn rollback_active_switch(
    app_handle: &AppHandle,
    previous_pointer: Option<&str>,
    previous_setting: config::Setting,
    was_running: bool,
    error: String,
) -> String {
    let pointer = config::restore_active_dsh_slot(app_handle, previous_pointer);
    let setting = config::set_store_dat_setting(app_handle, previous_setting);
    let plugins = if pointer.is_ok() && setting.is_ok() {
        crate::service::plugin::rebind_for_active_core(app_handle).await
    } else {
        Ok(())
    };
    let service = if was_running && pointer.is_ok() && setting.is_ok() && plugins.is_ok() {
        crate::service::workflow::start(app_handle.clone()).await
    } else {
        Ok(())
    };
    let rollback_errors = [pointer.err(), setting.err(), plugins.err(), service.err()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if rollback_errors.is_empty() {
        error
    } else {
        format!(
            "{error}; CORE_SWITCH_ROLLBACK_FAILED: {}",
            rollback_errors.join("; ")
        )
    }
}

/// 切换到指定 tag 的预打包版本（已下载的不可变槽位）。
///
/// 切换不移动或删除内核目录；目标入口校验通过后一次写入活动 tag。运行中的服务
/// 仍持有原槽位，直到调用方显式重启，因此写设置失败也不会破坏当前进程。
async fn switch_app_version(app_handle: &AppHandle, tag: &str) -> Result<(), String> {
    fs_guard::validate_id(tag)?;
    let discovered_dir = existing_slot_dir(app_handle, tag)
        .ok_or_else(|| format!("CORE_VERSION_NOT_DOWNLOADED: {tag}"))?;
    let target_dir = slot_dir(app_handle, tag);
    if discovered_dir != target_dir {
        std::fs::create_dir_all(config::get_dsh_core_slots_path(app_handle))
            .map_err(|error| format!("CORE_SLOT_DIR_CREATE: {error}"))?;
        download::rename_with_retry(&discovered_dir, &target_dir)
            .await
            .map_err(|error| format!("CORE_SLOT_MIGRATE: {error}"))?;
        log::info!(
            "Migrated inactive legacy core slot {} to {}",
            discovered_dir.display(),
            target_dir.display()
        );
    }
    let target_entry = target_dir.join(config::DSH_ENTRY_RELATIVE);
    if !target_entry.is_file() {
        return Err(format!(
            "CORE_VERSION_INVALID: dsh entry is missing from {}",
            target_dir.display()
        ));
    }
    // 记录切换：tag + commit（commit 从 tags 列表反查，失败保留原值）
    let commit = match download::fetch_dsh_pkg_tags().await {
        Ok(tags) => tags.into_iter().find(|(t, _)| t == tag).map(|(_, c)| c),
        Err(e) => {
            log::warn!("failed to resolve commit for tag {tag}: {e}");
            None
        }
    };
    let previous_pointer = config::get_active_dsh_slot(app_handle);
    config::set_active_dsh_slot(app_handle, tag)?;
    let mut setting = config::get_store_dat_setting(app_handle);
    setting.active_core = Some(CoreSource::App.as_str().to_string());
    setting.dsh_pkg_tag = Some(tag.to_string());
    if let Some(c) = commit {
        setting.dsh_pkg_commit = Some(c);
    }
    if let Err(error) = config::set_store_dat_setting(app_handle, setting) {
        let rollback = config::restore_active_dsh_slot(app_handle, previous_pointer.as_deref());
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!("{error}; CORE_SWITCH_ROLLBACK_FAILED: {rollback}")),
        };
    }
    Ok(())
}

/// 下载指定 tag 的预打包内核到不可变槽位 `dependencies/cores/<tag>`（不激活，切换由
/// `set_active` 完成）。幂等：已下载时直接返回该版本行。
pub async fn download_version(app_handle: &AppHandle, tag: &str) -> Result<HarnessCore, String> {
    // 路径安全：tag 直接进入 `dependencies/cores/<tag>`，需挡 `..`/分隔符
    fs_guard::validate_id(tag)?;
    let dest = slot_dir(app_handle, tag);
    if dest.exists() {
        return Ok(row_for_tag(app_handle, tag, &dest));
    }

    // 1. 拉该 tag 的资产地址 + 可信摘要（digest 缺失时安全中止，沿用
    //    DSH_INTEGRITY_UNAVAILABLE 设计：不下载无法验证完整性的内容）
    let info = download::fetch_dsh_pkg_asset(tag)
        .await
        .map_err(|e| format!("CORE_METADATA_FAILED: {e}"))?;
    let digest = info.digest.ok_or_else(|| {
        format!("CORE_INTEGRITY_UNAVAILABLE: trusted SHA-256 unavailable for {tag}, cannot download safely")
    })?;

    // 2. 下载 + 校验 + 原子解压到历史槽位（两阶段进度：下载 0-50，解压 50-100）
    //    下载默认走 GitHub 官方直连，失败自动切换 ghfast.top 镜像兜底。
    let window = app_handle
        .get_webview_window("main")
        .ok_or("WINDOW_NOT_FOUND: main window missing")?;
    let mut tracker = download::ProgressTracker::new(&window, 2);
    tracker.start_phase("download", &format!("正在下载内核版本 {tag}"));
    let urls = vec![
        info.asset_url.clone(),
        config::mirror_download_url(&info.asset_url),
    ];
    let buffer = download::download_file_from_sources(&tracker, urls)
        .await
        .map_err(|e| format!("CORE_DOWNLOAD_FAILED: {e}"))?;
    download::verify_sha256(&buffer, &digest).map_err(|e| format!("CORE_INTEGRITY_FAILED: {e}"))?;
    tracker.end_phase();
    let name = info
        .asset_url
        .rsplit('/')
        .next()
        .unwrap_or(&info.asset_url)
        .to_string();
    tracker.start_phase("extract", &format!("正在解压内核版本 {tag}"));
    download::ensure_extract(&tracker, name, buffer, dest.clone())
        .await
        .map_err(|e| format!("CORE_EXTRACT_FAILED: {e}"))?;
    tracker.end_phase();
    log::info!("Downloaded dsh core {tag} to {}", dest.display());

    Ok(row_for_tag(app_handle, tag, &dest))
}

/// 卸载已下载的历史版本（激活中的版本不可卸载）。
pub async fn remove_version(app_handle: &AppHandle, id: &str) -> Result<(), String> {
    let Some(tag) = id.strip_prefix("app-") else {
        return Err(format!("CORE_INVALID_ID: {id}"));
    };
    // 路径安全：tag 需通过字符集白名单（tag 形如 `dsh-0.1.0-rc.8-<commit>`），
    // 拒绝 `..`、分隔符等，防止 `remove_core("app-..")` 把目标推出依赖根目录。
    fs_guard::validate_id(tag)?;
    let cur_tag = active_app_tag(
        config::get_active_dsh_slot(app_handle),
        config::get_dsh_pkg_tag(app_handle),
    );
    if cur_tag.as_deref() == Some(tag) && active_source(app_handle) == CoreSource::App {
        return Err(format!(
            "CORE_ACTIVE_VERSION: cannot remove in-use version {tag}"
        ));
    }
    let dir = existing_slot_dir(app_handle, tag)
        .ok_or_else(|| format!("CORE_VERSION_NOT_FOUND: {tag}"))?;

    // 停止服务避免句柄锁定（被删目录可能是上一份激活副本，句柄未释放）
    if workflow::has_owned_process() {
        if let Err(e) = workflow::stop(app_handle.clone()).await {
            log::warn!("failed to stop harness before core removal: {e}");
        }
    }
    if !download::remove_dir_with_retry(&dir).await {
        return Err(format!(
            "CORE_REMOVE_FAILED: cannot remove {}",
            dir.display()
        ));
    }
    Ok(())
}

/// 解析激活预打包内核的版本号：优先记录 tag（`dsh-<version>-<commit>`），
/// 解析不出（无 tag 记录/格式不符）时用安装目录清单版本兜底。
fn active_app_tag(active_slot: Option<String>, recorded_release: Option<String>) -> Option<String> {
    active_slot.or(recorded_release)
}

fn active_app_version(
    active_tag: &Option<String>,
    manifest_version: Option<String>,
) -> Option<String> {
    active_tag
        .as_deref()
        .and_then(download::parse_version_from_tag)
        .and_then(|version| {
            let normalized = version.trim_start_matches('v').to_string();
            semver::Version::parse(&normalized).ok().map(|_| normalized)
        })
        .or(manifest_version)
}

/// 构造某个已下载 tag 的内核行（下载完成/已存在时返回）。
fn row_for_tag(app_handle: &AppHandle, tag: &str, dir: &Path) -> HarnessCore {
    let active = active_app_tag(
        config::get_active_dsh_slot(app_handle),
        config::get_dsh_pkg_tag(app_handle),
    )
    .as_deref()
        == Some(tag)
        && active_source(app_handle) == CoreSource::App;
    let dir_str = dir.to_string_lossy().into_owned();
    HarnessCore {
        id: format!("app-{tag}"),
        source: CoreSource::App,
        version: download::parse_version_from_tag(tag).unwrap_or_default(),
        tag: tag.to_string(),
        path: dir_str.clone(),
        dir: dir_str,
        present: true,
        active,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_paths_reject_traversal_and_accept_release_tag() {
        let root = std::env::temp_dir().join(format!("dsh-core-slots-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        assert!(safe_slot_path(&root, "foo/../../target").is_err());
        let valid = "dsh-0.1.0-rc.8-32331963388";
        let expected = root.join(valid);
        assert_eq!(safe_slot_path(&root, valid).unwrap(), expected);

        std::fs::create_dir_all(&expected).unwrap();
        assert_eq!(safe_slot_path(&root, valid).unwrap(), expected);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn slot_paths_reject_symlink_escape() {
        let root = std::env::temp_dir().join(format!("dsh-core-slots-link-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("dsh-core-slots-outside-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("dsh-evil")).unwrap();

        assert!(safe_slot_path(&root, "dsh-evil").is_err());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn list_dedupes_versions_keeping_last_tag() {
        // 模拟 pkg 仓库的测试打包：同一版本打多个 tag（最新在前），去重后每个版本
        // 只保留最后一个 tag，且顺序保持首次出现顺序（版本新→旧）。
        let tags = vec![
            ("dsh-0.1.1-rc.1-32342588166".to_string(), "c1".to_string()),
            ("dsh-0.1.0-rc.8-32331963388".to_string(), "c2".to_string()),
            // 同版本重复：后续 tag（测试打包）应被去重掉，保留最后一个
            ("dsh-0.1.0-rc.8-32342588166".to_string(), "c3".to_string()),
            ("dsh-0.1.0-rc.8-32342588167".to_string(), "c4".to_string()),
            ("dsh-0.1.0-rc.7-31773193667".to_string(), "c5".to_string()),
            ("dsh-0.1.0-rc.7-31773193668".to_string(), "c6".to_string()),
        ];
        let mut version_tags: Vec<(String, String)> = Vec::new();
        for (tag, _commit) in &tags {
            let Some(version) = download::parse_version_from_tag(tag) else {
                continue;
            };
            if let Some(entry) = version_tags.iter_mut().find(|(v, _)| v == &version) {
                entry.1 = tag.clone();
            } else {
                version_tags.push((version, tag.clone()));
            }
        }
        let versions: Vec<&str> = version_tags.iter().map(|(v, _)| v.as_str()).collect();
        let kept_tags: Vec<&str> = version_tags.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(versions, vec!["0.1.1-rc.1", "0.1.0-rc.8", "0.1.0-rc.7"]);
        // rc.8 / rc.7 都保留了最后一个 tag
        assert_eq!(kept_tags[1], "dsh-0.1.0-rc.8-32342588167");
        assert_eq!(kept_tags[2], "dsh-0.1.0-rc.7-31773193668");
    }

    #[test]
    fn active_version_falls_back_when_local_slot_tag_is_not_a_release_tag() {
        assert_eq!(
            active_app_version(
                &Some("dsh-v0.1.1-rc.2-local".to_string()),
                Some("0.1.1-rc.2".to_string()),
            ),
            Some("0.1.1-rc.2".to_string())
        );
    }

    #[test]
    fn active_slot_wins_over_stale_recorded_release_tag() {
        assert_eq!(
            active_app_tag(
                Some("dsh-0.1.2-alpha.1-source".to_string()),
                Some("dsh-0.1.1-rc.2-release".to_string()),
            ),
            Some("dsh-0.1.2-alpha.1-source".to_string())
        );
        assert_eq!(
            active_app_version(
                &active_app_tag(
                    Some("dsh-0.1.2-alpha.1-source".to_string()),
                    Some("dsh-0.1.1-rc.2-release".to_string()),
                ),
                Some("0.1.2-alpha.1".to_string()),
            ),
            Some("0.1.2-alpha.1".to_string())
        );
    }
}
