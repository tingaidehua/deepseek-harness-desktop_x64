//! 浏览器/文件管理器唤起、日志捕获与健康检测。
//!
//! 对外部系统组件的交互：在系统浏览器打开链接、在文件管理器定位/打开目录与
//! 数据目录、复制服务地址到剪贴板；前端与后端日志的透传/读取/清空；以及通过
//! Rust 代理的服务健康检查与运行时环境诊断信息。

use crate::config;
use crate::logger;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

/// 健康检查（通过 Rust 代理，避免 WebView CORS 问题）
#[tauri::command]
pub async fn proxy_health_check(app_handle: AppHandle) -> Result<String, String> {
    let port = config::get_store_dat_setting(&app_handle).port;
    crate::service::workflow::proxy_health_check(&app_handle, port).await
}

/// 运行时/版本/诊断信息（侧边栏展示）
#[tauri::command]
pub async fn get_runtime_info(app_handle: AppHandle) -> Result<config::RuntimeInfo, String> {
    let port = config::get_store_dat_setting(&app_handle).port;
    let mut info = config::runtime_info(&app_handle, port);
    info.webview_url = crate::service::core_compatibility::CoreCompatibility::active(&app_handle)?
        .webview_url(port);
    Ok(info)
}

/// 在系统浏览器中打开 Harness 界面
#[tauri::command]
pub async fn open_in_browser(app_handle: AppHandle) -> Result<(), String> {
    let port = config::get_store_dat_setting(&app_handle).port;
    let url = crate::service::workflow::utils::authenticated_service_url(port)
        .unwrap_or_else(|| config::get_dsh_service_url(port));
    app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// 复制 Harness 服务地址到剪贴板
#[tauri::command]
pub async fn copy_service_url(app_handle: AppHandle) -> Result<(), String> {
    let url = config::get_dsh_service_url(config::get_store_dat_setting(&app_handle).port);
    app_handle
        .clipboard()
        .write_text(url)
        .map_err(|e| e.to_string())
}

/// 在系统文件管理器中定位指定文件（Session 日志下载完成后的"在文件夹中显示"）
#[tauri::command]
pub fn reveal_in_folder(app_handle: AppHandle, path: String) -> Result<(), String> {
    // 安全边界：只允许定位允许根目录（下载目录/数据目录/$DSH_HOME）内的文件，
    // 防止第三方插件通过 IPC 驱动宿主打开任意路径。
    let path = validated_allowed_path(&app_handle, Path::new(&path), PathKind::Any)
        .map_err(|error| format!("REVEAL_UNAVAILABLE: {error}"))?;
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| format!("REVEAL_FAILED: {e}"))
}

/// 在系统文件管理器中打开指定目录（内核版本「打开目录」按钮；目录用 open 而非
/// reveal——reveal 是定位父目录，open 是直接打开该目录本身）。
#[tauri::command]
pub fn open_dir(app_handle: AppHandle, path: String) -> Result<(), String> {
    // 安全边界同 reveal_in_folder：仅允许打开允许根目录内的目录
    let path = validated_allowed_path(&app_handle, Path::new(&path), PathKind::Directory)
        .map_err(|error| format!("OPEN_DIR_UNAVAILABLE: {error}"))?;
    open_existing_dir(&app_handle, &path)
}

/// 在系统文件管理器中打开数据目录（官方 $DSH_HOME，即 ~/.dsh）
#[tauri::command]
pub async fn reveal_data_dir(app_handle: AppHandle) -> Result<(), String> {
    let dsh_home = config::get_dsh_data_path(&app_handle);
    // 目录可能尚未创建（全新安装），先建好再打开，避免资源管理器报路径不存在
    std::fs::create_dir_all(&dsh_home).map_err(|e| e.to_string())?;

    let path = validated_allowed_path(&app_handle, &dsh_home, PathKind::Directory)
        .map_err(|error| format!("DATA_DIR_UNAVAILABLE: {error}"))?;
    open_existing_dir(&app_handle, &path)
}

#[derive(Clone, Copy)]
enum PathKind {
    Any,
    Directory,
}

/// 在交给系统组件前完成存在性、类型、安全根与规范路径检查。
fn validated_allowed_path(
    app_handle: &AppHandle,
    path: &Path,
    kind: PathKind,
) -> Result<PathBuf, String> {
    if !crate::bridge::guard::is_allowed_path(app_handle, path) {
        return Err("path does not exist or is outside an allowed directory".to_string());
    }
    canonical_existing_path(path, kind)
}

fn canonical_existing_path(path: &Path, kind: PathKind) -> Result<PathBuf, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("metadata: {error}"))?;
    if matches!(kind, PathKind::Directory) && !metadata.is_dir() {
        return Err("path is not a directory".to_string());
    }
    dunce::canonicalize(path).map_err(|error| format!("canonicalize: {error}"))
}

/// 打开已经验证的目录。Windows 使用禁止系统错误框的 ShellExecuteEx；失败由
/// Tauri command 返回给前端 toast，不能再弹出脱离 Desktop 生命周期的系统对话框。
fn open_existing_dir(_app_handle: &AppHandle, path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        return open_existing_dir_windows(path);
    }
    #[cfg(not(windows))]
    {
        _app_handle
            .opener()
            .open_path(path, None::<&str>)
            .map_err(|error| format!("OPEN_DIR_FAILED: {error}"))
    }
}

#[cfg(windows)]
fn open_existing_dir_windows(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        lpVerb: verb.as_ptr(),
        lpFile: target.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    let opened = unsafe { ShellExecuteExW(&mut info) };
    if opened == 0 {
        return Err(format!(
            "OPEN_DIR_FAILED: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// 前端日志透传：前端 `console.*` 劫持经此命令落盘到 `desktop.frontdesk.log`
/// （与持有后端 + `dsh` target 的 `desktop.log` 分离），见 `logger/mod.rs`。
#[tauri::command]
pub fn log_frontend(level: String, target: String, message: String) {
    let lvl = logger::FrontendLevel::from_str(&level);
    logger::log_frontend(lvl, &target, &message);
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendLogEntry {
    level: String,
    target: String,
    message: String,
}

/// 批量接收前端日志，限制单批条数和单条长度，避免异常页面制造无界 IPC 或写盘负载。
#[tauri::command]
pub fn log_frontend_batch(entries: Vec<FrontendLogEntry>) {
    const MAX_ENTRIES: usize = 256;
    const MAX_MESSAGE_BYTES: usize = 16 * 1024;
    const MAX_BATCH_BYTES: usize = 256 * 1024;
    let mut normalized = Vec::new();
    let mut total_bytes = 0usize;
    for entry in entries.into_iter().take(MAX_ENTRIES) {
        let message = tail_bytes(&entry.message, MAX_MESSAGE_BYTES).to_string();
        if total_bytes.saturating_add(message.len()) > MAX_BATCH_BYTES {
            break;
        }
        total_bytes += message.len();
        normalized.push((
            logger::FrontendLevel::from_str(&entry.level),
            entry.target.chars().take(80).collect::<String>(),
            message,
        ));
    }
    let borrowed = normalized
        .iter()
        .map(|(level, target, message)| (*level, target.as_str(), message.as_str()))
        .collect::<Vec<_>>();
    logger::log_frontend_batch(&borrowed);
}

/// 按字节上限取 `s` 的尾部，并在裁剪起点回退到 UTF-8 字符边界。
///
/// 日志必然包含中文/ANSI 等多字节字符，直接用
/// `&s[s.len() - max_bytes..]` 在起点落在字符中间时会 panic
/// （`byte index ... is not a char boundary`），此实现保证安全。
fn tail_bytes(s: &str, max_bytes: usize) -> &str {
    let start = s.len().saturating_sub(max_bytes);
    let mut i = start;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

/// 读取 dsh 服务日志
#[tauri::command]
pub async fn read_service_logs(
    app_handle: AppHandle,
    max_bytes: Option<usize>,
) -> Result<String, String> {
    let log_path = config::get_service_log_path(&app_handle);
    if !log_path.exists() {
        return Ok(String::new());
    }

    let content = std::fs::read_to_string(&log_path).map_err(|e| e.to_string())?;
    let max_bytes = max_bytes.unwrap_or(64 * 1024);
    if content.len() <= max_bytes {
        Ok(content)
    } else {
        Ok(tail_bytes(&content, max_bytes).to_string())
    }
}

/// 清空 dsh 服务日志
#[tauri::command]
pub async fn clear_service_logs(app_handle: AppHandle) -> Result<(), String> {
    let log_path = config::get_service_log_path(&app_handle);
    std::fs::write(&log_path, "").map_err(|e| e.to_string())
}

fn read_file_tail(path: &Path, max_bytes: usize, max_lines: usize) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let start = length.saturating_sub(max_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::with_capacity((length - start) as usize);
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let content = String::from_utf8_lossy(&bytes);
    let lines = content.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(max_lines)..].join("\n")
}

fn redact_diagnostic_text(input: &str, roots: &[(&Path, &str)]) -> String {
    let mut output = input.to_string();
    let mut replacements = roots
        .iter()
        .filter_map(|(path, replacement)| {
            let value = path.to_string_lossy().into_owned();
            (!value.is_empty()).then_some((value, *replacement))
        })
        .collect::<Vec<_>>();
    replacements.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
    for (path, replacement) in replacements {
        output = output.replace(&path, replacement);
        output = output.replace(&path.replace('\\', "/"), replacement);
    }
    let secret = regex::Regex::new(
        r"(?i)(authorization|cookie|api[_-]?key|access[_-]?token|control[_-]?token|token|password|secret)(\s*[:=]\s*)([^\s,;]+)",
    )
    .expect("static diagnostic secret regex");
    secret.replace_all(&output, "$1$2<REDACTED>").into_owned()
}

fn log_file_summary(path: &Path, max_backups: usize) -> String {
    let current = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let backups = (1..=max_backups)
        .filter(|index| PathBuf::from(format!("{}.{}", path.display(), index)).is_file())
        .count();
    format!("currentBytes={current}; maxBytes=5242880; backups={backups}/{max_backups}")
}

/// 生成适合报障粘贴的脱敏诊断包。只读取每个有界日志文件的尾部，并附带内核、
/// 插件、壳资源、React 挂载、frame 功能面和控制调用轨迹，页面黑屏时也可由控制面读取。
#[tauri::command]
pub async fn read_run_logs(app_handle: AppHandle) -> Result<String, String> {
    const MAX_LINES: usize = 100;
    const FRONTEND_MAX_LINES: usize = MAX_LINES / 2;
    const READ_BYTES: usize = 256 * 1024;

    let base = config::get_base_dir(&app_handle);
    let dsh_home = config::get_dsh_data_path(&app_handle);
    let service = config::get_service_log_path(&app_handle);
    let desktop = base.join("logs").join("desktop.log");
    let frontend = base.join("logs").join("desktop.frontdesk.log");
    let trace = crate::service::control::trace_path(&app_handle);
    let profile = crate::service::profile::active_profile(&app_handle);
    let dsh_version = config::get_dsh_version(&app_handle)
        .map(|v| format!("dsh: {v}\n"))
        .unwrap_or_default();
    let env_text = format!(
        "app: {}\n{}node: {}\nos: {} ({})\nprofile: <ACTIVE_PROFILE>\nlogPolicy: 5 MiB current + 3 backups per stream",
        app_handle.package_info().version,
        dsh_version,
        config::get_active_node_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );

    let snapshot = crate::diagnostics::active_snapshot(&app_handle)
        .and_then(|mut snapshot| {
            snapshot.core_path = "<ACTIVE_CORE>".to_string();
            snapshot.profile_path = "<ACTIVE_PROFILE_PATH>".to_string();
            serde_json::to_string_pretty(&snapshot)
                .map_err(|error| format!("DIAGNOSTICS_SERIALIZE: {error}"))
        })
        .unwrap_or_else(|error| format!("{{\"snapshotError\":{}}}", serde_json::json!(error)));
    let user_home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    let mut roots = vec![
        (base.as_path(), "<APP_DATA>"),
        (dsh_home.as_path(), "<DSH_HOME>"),
    ];
    if let Some(user_home) = &user_home {
        roots.push((user_home.as_path(), "<USER_HOME>"));
    }
    let service_text = read_file_tail(&service, READ_BYTES, MAX_LINES);
    let frontend_text = read_file_tail(&frontend, READ_BYTES, FRONTEND_MAX_LINES);
    let backend_text = read_file_tail(&desktop, READ_BYTES, MAX_LINES)
        .lines()
        .filter(|line| !is_frontend_log_line(line))
        .collect::<Vec<_>>()
        .join("\n");
    let trace_text = read_file_tail(&trace, READ_BYTES, MAX_LINES);
    let metadata = format!(
        "service: {}\nfrontend: {}\nbackend: {}\ncontrolTrace: {}",
        log_file_summary(&service, 3),
        log_file_summary(&frontend, 3),
        log_file_summary(&desktop, 3),
        log_file_summary(&trace, 1),
    );

    let report = redact_diagnostic_text(&format!(
        "### 环境信息\n\n{}\n\n### 诊断快照\n\n```json\n{}\n```\n\n### 日志容量\n\n{}\n\n### 控制调用轨迹（尾部）\n\n```jsonl\n{}\n```\n\n### 服务日志（尾部）\n\n```\n{}\n```\n\n### 前台日志（尾部）\n\n```\n{}\n```\n\n### 后台日志（尾部）\n\n```\n{}\n```",
        env_text,
        snapshot,
        metadata,
        trace_text.trim_end(),
        service_text.trim_end(),
        frontend_text.trim_end(),
        backend_text.trim_end()
    ), &roots);
    Ok(if profile.is_empty() {
        report
    } else {
        report.replace(&profile, "<ACTIVE_PROFILE>")
    })
}

/// 判断某行是否为前端日志（`target: "frontend"`）。
/// 日志行格式见 logger/mod.rs：`[ts] LEVEL target: message`（时间戳可能含空格）。
/// 前端行的 target 恒为 `frontend`，紧跟 LEVEL 之后；用「LEVEL + frontend:」定位，
/// 避免把消息正文里出现的 "frontend" 误判为前端日志。
fn is_frontend_log_line(line: &str) -> bool {
    const LEVELS: [&str; 5] = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
    let trimmed = line.trim_start();
    LEVELS
        .iter()
        .any(|lvl| trimmed.contains(&format!("{lvl} frontend:")))
}

/// 在系统浏览器中打开任意 http(s) 链接（更新说明 / 关于对话框仓库链接等）
#[tauri::command]
pub async fn open_external_url(app_handle: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("EXTERNAL_URL_INVALID: {url}"));
    }
    app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::is_frontend_log_line;
    use super::tail_bytes;
    use super::{canonical_existing_path, PathKind};
    use super::{read_file_tail, redact_diagnostic_text};
    use std::path::PathBuf;

    #[test]
    fn external_paths_must_exist_and_match_the_requested_kind() {
        let root =
            std::env::temp_dir().join(format!("dsh-desktop-open-path-{}", std::process::id()));
        let file = root.join("item.txt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&file, "fixture").unwrap();

        assert!(canonical_existing_path(&root, PathKind::Directory).is_ok());
        assert!(canonical_existing_path(&file, PathKind::Any).is_ok());
        assert!(canonical_existing_path(&file, PathKind::Directory).is_err());
        assert!(canonical_existing_path(&root.join("missing"), PathKind::Any).is_err());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostic_bundle_redacts_paths_and_secrets() {
        let root = PathBuf::from(r"C:\Users\person\AppData\Desktop");
        let home = PathBuf::from(r"C:\Users\person");
        let text = r"C:\Users\person\AppData\Desktop\logs C:\Users\person\bin token=abc Authorization:Bearer-123";
        let redacted = redact_diagnostic_text(
            text,
            &[
                (root.as_path(), "<APP_DATA>"),
                (home.as_path(), "<USER_HOME>"),
            ],
        );
        assert!(redacted.contains("<APP_DATA>\\logs"));
        assert!(redacted.contains("<USER_HOME>\\bin"));
        assert!(!redacted.contains("person"));
        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("Bearer-123"));
    }

    #[test]
    fn diagnostic_tail_is_bounded_by_lines() {
        let path = std::env::temp_dir().join(format!(
            "dsh-desktop-diagnostic-tail-{}.log",
            std::process::id()
        ));
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(read_file_tail(&path, 1024, 2), "three\nfour");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn frontend_line_detected() {
        // tracing 文件层（desktop.log）与前端独立文件（desktop.frontdesk.log）两种时间戳格式都应命中
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z INFO frontend: [tag] message"
        ));
        assert!(is_frontend_log_line(
            "[2024-06-01 12:00:00.123Z] INFO frontend: message"
        ));
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z WARN frontend: something"
        ));
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z ERROR frontend: boom"
        ));
    }

    #[test]
    fn backend_line_not_detected() {
        // 后端（dsh 等 target）不应误判为前端；消息正文里出现 "frontend" 也不应命中
        assert!(!is_frontend_log_line(
            "2024-06-01 12:00:00.123Z INFO dsh: starting server"
        ));
        assert!(!is_frontend_log_line(
            "[2024-06-01 12:00:00.123Z] INFO dsh: emit to frontend: 3"
        ));
        assert!(!is_frontend_log_line(
            "2024-06-01 12:00:00.123Z DEBUG reqwest: GET /ping"
        ));
    }

    #[test]
    fn frontend_level_padding_and_extra_spaces() {
        // 级别可能带前导空格（`{:>5}` 或 tracing 层多空格），frontend 目标仍应命中
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z  INFO frontend: padded"
        ));
    }

    #[test]
    fn tail_bytes_keeps_ascii_within_limit() {
        assert_eq!(tail_bytes("hello world", 5), "world");
        // 起点已落在字符边界时原样截取
        assert_eq!(tail_bytes("abc", 2), "bc");
    }

    #[test]
    fn tail_bytes_advances_to_char_boundary() {
        // 截取起点落在 3 字节中文中间 → 回退到字符边界，不 panic 且结果 ≤ max_bytes
        assert_eq!(tail_bytes("中a", 2), "a");
        // 4 字节 emoji 同理（非边界前缀字节会连续回退）
        assert_eq!(tail_bytes("😀x", 3), "x");
        // 多字节 + 超限，回退后长度仍不超过 max_bytes
        assert_eq!(tail_bytes("中文abc", 3), "abc");
    }

    #[test]
    fn tail_bytes_shorter_than_limit_returns_whole() {
        assert_eq!(tail_bytes("中文", 10), "中文");
        assert_eq!(tail_bytes("", 10), "");
    }
}
