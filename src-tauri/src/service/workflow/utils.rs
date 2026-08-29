use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::sync::{Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

const DSH_MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const DSH_MAX_BACKUPS: usize = 3;
static DSH_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static DSH_AUTHENTICATED_URL: OnceLock<RwLock<Option<String>>> = OnceLock::new();
fn dsh_log_lock() -> &'static Mutex<()> {
    DSH_LOG_LOCK.get_or_init(|| Mutex::new(()))
}

fn authenticated_url_slot() -> &'static RwLock<Option<String>> {
    DSH_AUTHENTICATED_URL.get_or_init(|| RwLock::new(None))
}

/// 清除上一进程的一次性浏览器认证地址。
pub(super) fn clear_authenticated_service_url() {
    *authenticated_url_slot()
        .write()
        .unwrap_or_else(|error| error.into_inner()) = None;
}

/// 返回当前 DSH 进程为指定端口发布的一次性浏览器认证地址。
pub(crate) fn authenticated_service_url(port: u16) -> Option<String> {
    let url = authenticated_url_slot()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()?;
    let parsed = reqwest::Url::parse(&url).ok()?;
    (parsed.port_or_known_default() == Some(port)).then_some(url)
}

/// 接收独立 Harness 控制插件在恢复外壳时重新签发的浏览器认证地址。
pub(crate) fn restore_authenticated_service_url(candidate: &str, port: u16) -> Result<(), String> {
    let parsed = reqwest::Url::parse(candidate)
        .map_err(|error| format!("HARNESS_RECOVERY_URL_PARSE: {error}"))?;
    let loopback = parsed.host_str().is_some_and(|host| {
        host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
    });
    let has_token = parsed
        .query_pairs()
        .any(|(key, value)| key == "token" && !value.is_empty());
    if !loopback || parsed.port_or_known_default() != Some(port) || !has_token {
        return Err(
            "HARNESS_RECOVERY_URL_INVALID: URL must contain a token for the owned loopback port"
                .to_string(),
        );
    }
    *authenticated_url_slot()
        .write()
        .unwrap_or_else(|error| error.into_inner()) = Some(candidate.to_string());
    Ok(())
}

/// 返回供 Tauri iframe 使用的同站点认证地址。
///
/// 官方 DSH 认证 cookie 是 `HttpOnly; SameSite=Strict`。将回环 IP 仅在 WebView
/// 导航地址中改为 `dsh.tauri.localhost`，可使它与壳的 `http://tauri.localhost`
/// 同站，又避免 Tauri 把 iframe 误判为自身资源；系统浏览器与健康检查仍使用
/// 原始 `127.0.0.1` 地址。
pub(crate) fn authenticated_webview_url(port: u16) -> Option<String> {
    let url = authenticated_service_url(port)?;
    let mut parsed = reqwest::Url::parse(&url).ok()?;
    parsed.set_host(Some("dsh.tauri.localhost")).ok()?;
    Some(parsed.to_string())
}

/// 捕获新版 DSH 的浏览器认证地址，并返回可安全持久化的脱敏日志行。
fn capture_authenticated_service_url(line: &str) -> String {
    let Some(candidate) = line
        .strip_prefix("dsh web: ")
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return line.to_string();
    };
    let Ok(mut url) = reqwest::Url::parse(candidate) else {
        return line.to_string();
    };
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
    });
    let has_token = url
        .query_pairs()
        .any(|(key, value)| key == "token" && !value.is_empty());
    if !loopback || !has_token {
        return line.to_string();
    }
    *authenticated_url_slot()
        .write()
        .unwrap_or_else(|error| error.into_inner()) = Some(candidate.to_string());
    url.set_query(None);
    format!(
        "dsh web: {} (authenticated URL captured by Desktop)",
        url.as_str()
    )
}

/// 构造仅用于回环地址探测的 HTTP 客户端。
///
/// 生命周期探测访问的是本机 dsh，不能继承 `HTTP_PROXY` / `ALL_PROXY`：部分代理
/// 不尊重回环地址直连，或应用进程没有 `NO_PROXY`，会把健康检查转发到外部代理，
/// 造成端口已经监听但持续误报未就绪。
pub(super) fn loopback_http_client(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .build()
}

/// 从启动页提取带当前 revision 的客户端插件 bundle 探测地址。
///
/// SPA `/` 在 webServer 绑定后立刻 200，此时连接桥与 Loader 图往往还没就绪；
/// WebView 若在这个窗口加载，会永久停在官方 boot 页 “Loading plugins…”。新版
/// DSH 只接受启动页声明的 revision URL，无 revision 的固定插件地址会返回 404。
/// 必须读取当前启动页并取得真实 JS bundle，才视为可挂载 iframe。
pub(super) fn health_probe_plugin_urls(port: u16, boot_html: &str) -> Vec<String> {
    const PROBE_PACKAGES: [&str; 2] = [
        "@deepseek-ai/dsh-client-modules",
        "@deepseek-ai/dsh-client-ui-layout",
    ];

    PROBE_PACKAGES
        .iter()
        .filter_map(|package| {
            // RC 使用单包路由 `/plugins/<pkg>/client.js`，alpha.1 使用可合并路由
            // `/plugins/??<pkg>/client.js`。两者都从启动页读取真实 revision，避免
            // 猜测固定 URL；未来协议只需在核心兼容记录的测试夹具中增加声明形式。
            let merged = format!("/plugins/??{package}/client.js");
            let single = format!("/plugins/{package}/client.js");
            let start = boot_html
                .find(&merged)
                .or_else(|| boot_html.find(&single))?;
            let tail = &boot_html[start..];
            let end = tail.find('"')?;
            let path = tail[..end].replace("&amp;", "&");
            Some(format!("http://127.0.0.1:{port}{path}"))
        })
        .collect()
}

/// 判断健康检查响应是不是可用的插件 bundle。
///
/// 未知 `/plugins/...` 路径会被 SPA fallback 成 `index.html`（仍是 200），
/// 绝不能当成插件已就绪。
pub(super) fn looks_like_plugin_bundle(ok_status: bool, body: &str) -> bool {
    if !ok_status {
        return false;
    }
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("<!doctype") || lower.starts_with("<html") {
        return false;
    }
    true
}

/// 检查 Harness 是否真正在运行（探测指定端口，随配置端口联动）
pub async fn is_dsh_running(port: u16) -> bool {
    let client = loopback_http_client(Duration::from_secs(2)).ok(); // 将 Result 转为 Option

    // 如果 client 创建失败，直接返回 false
    let client = match client {
        Some(c) => c,
        None => return false,
    };

    let url = format!("{}/", crate::config::get_dsh_service_url(port));

    // 发送请求并判断是否就绪
    let check_status = async {
        let resp = client.get(&url).send().await.ok()?;
        if resp.status() != reqwest::StatusCode::OK {
            return None;
        }
        Some(true)
    };

    check_status.await.unwrap_or(false)
}

/// 检查指定端口是否被占用（通过尝试连接来判断）
pub fn is_port_in_use(port: u16) -> bool {
    // 以实际绑定结果判断，能够识别“已绑定但尚未 listen”的占用状态。
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpListener::bind(addr).is_err()
}

/// 在独立线程中读取子进程的输出，同时写入日志文件
///
/// # 参数
/// - `stdout`: 子进程的标准输出
/// - `stderr`: 子进程的标准错误输出
/// - `log_path`: 前端日志面板读取的日志文件
pub fn spawn_output_readers<R1, R2>(
    stdout: Option<R1>,
    stderr: Option<R2>,
    log_path: PathBuf,
    spawned_at: Instant,
) where
    R1: Read + Send + 'static,
    R2: Read + Send + 'static,
{
    let readiness_logged = Arc::new(AtomicBool::new(false));
    // 在独立线程中读取 stdout
    if let Some(stdout) = stdout {
        let log_path = log_path.clone();
        let readiness_logged = readiness_logged.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let safe_line = capture_authenticated_service_url(&line);
                        if safe_line.contains("authenticated URL captured by Desktop")
                            && !readiness_logged.swap(true, Ordering::SeqCst)
                        {
                            log::info!(
                                "STARTUP_PHASE dsh_authenticated_url duration_ms={}",
                                spawned_at.elapsed().as_millis()
                            );
                        }
                        log::info!(target: "dsh", "{}", safe_line);
                        append_log(&log_path, &safe_line);
                    }
                    Err(e) => {
                        log::error!("Failed to read dsh stdout: {}", e);
                        break;
                    }
                }
            }
        });
    }

    // 在独立线程中读取 stderr
    if let Some(stderr) = stderr {
        let readiness_logged = readiness_logged.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let safe_line = capture_authenticated_service_url(&line);
                        if safe_line.contains("authenticated URL captured by Desktop")
                            && !readiness_logged.swap(true, Ordering::SeqCst)
                        {
                            log::info!(
                                "STARTUP_PHASE dsh_authenticated_url duration_ms={}",
                                spawned_at.elapsed().as_millis()
                            );
                        }
                        log::warn!(target: "dsh", "{}", safe_line);
                        append_log(&log_path, &safe_line);
                    }
                    Err(e) => {
                        log::error!("Failed to read dsh stderr: {}", e);
                        break;
                    }
                }
            }
        });
    }
}

fn append_log(log_path: &PathBuf, line: &str) {
    // 与 `logger` 的 `desktop.log` / `desktop.frontdesk.log` 保持一致：5MiB × 3 轮转
    let _guard = dsh_log_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(file, "{}", line);
        let _ = file.flush();
    }
    // 超阈值则按大小轮转（与启动次轮转 `rotate_service_log` 互补，避免单次运行无限增长）
    if let Ok(meta) = std::fs::metadata(log_path) {
        if meta.len() > DSH_MAX_LOG_BYTES {
            let _ = std::fs::remove_file(indexed_log_path(log_path, DSH_MAX_BACKUPS));
            for i in (1..DSH_MAX_BACKUPS).rev() {
                let from = indexed_log_path(log_path, i);
                let to = indexed_log_path(log_path, i + 1);
                if from.exists() {
                    let _ = std::fs::remove_file(&to);
                    let _ = std::fs::rename(&from, &to);
                }
            }
            if log_path.exists() {
                let _ = std::fs::rename(log_path, indexed_log_path(log_path, 1));
            }
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(log_path);
        }
    }
}

/// 轮转日志文件名：`dsh-web.log`（index 0）、`dsh-web.log.1`、`dsh-web.log.2`……
fn indexed_log_path(log_path: &PathBuf, index: usize) -> PathBuf {
    if index == 0 {
        return log_path.clone();
    }
    let mut name = log_path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}", index));
    log_path.with_file_name(name)
}

/// 每次启动服务前轮转日志，只保留最近 `keep` 次启动产生的日志文件。
///
/// 把当前 `dsh-web.log` 依次后退为 `.1`、`.2`……，超过保留上限的最老文件
/// 直接删除，再以空文件重新记录本次启动日志。这样磁盘上始终只保留最近
/// `keep` 次 dsh 启动的日志，避免单文件随多次启动无限增长。
pub fn rotate_service_log(log_path: &PathBuf, keep: usize) {
    if keep == 0 {
        let _ = std::fs::remove_file(log_path);
        return;
    }
    // 1) 删除超过保留上限的最老文件（它会被顶上来的文件覆盖且无处安放）
    let _ = std::fs::remove_file(&indexed_log_path(log_path, keep - 1));
    // 2) 从次老到次新依次后移，为本次启动腾出位置
    for i in (1..keep).rev() {
        let from = indexed_log_path(log_path, i);
        let to = indexed_log_path(log_path, i + 1);
        if from.exists() {
            let _ = std::fs::remove_file(&to);
            let _ = std::fs::rename(&from, &to);
        }
    }
    // 3) 当前日志后移为 `.1`，重新开始本次记录
    if log_path.exists() {
        let _ = std::fs::rename(log_path, indexed_log_path(log_path, 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &PathBuf, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn captures_browser_auth_url_without_persisting_token_in_log_line() {
        clear_authenticated_service_url();
        let safe =
            capture_authenticated_service_url("dsh web: http://127.0.0.1:4567/?token=secret-value");
        assert_eq!(
            safe,
            "dsh web: http://127.0.0.1:4567/ (authenticated URL captured by Desktop)"
        );
        assert_eq!(
            authenticated_service_url(4567).as_deref(),
            Some("http://127.0.0.1:4567/?token=secret-value")
        );
        assert_eq!(authenticated_service_url(4568), None);
        assert_eq!(
            authenticated_webview_url(4567).as_deref(),
            Some("http://dsh.tauri.localhost:4567/?token=secret-value")
        );
        clear_authenticated_service_url();
    }

    #[test]
    fn recovery_url_requires_token_and_exact_loopback_port() {
        clear_authenticated_service_url();
        restore_authenticated_service_url("http://127.0.0.1:4567/?token=reissued", 4567)
            .expect("accept recovery URL");
        assert_eq!(
            authenticated_service_url(4567).as_deref(),
            Some("http://127.0.0.1:4567/?token=reissued")
        );
        assert!(
            restore_authenticated_service_url("http://example.com:4567/?token=x", 4567).is_err()
        );
        assert!(restore_authenticated_service_url("http://127.0.0.1:4568/?token=x", 4567).is_err());
        assert!(restore_authenticated_service_url("http://127.0.0.1:4567/", 4567).is_err());
        clear_authenticated_service_url();
    }

    /// 模拟连续 5 次启动，验证磁盘上始终只保留最近 `keep` 份日志，
    /// 且每次启动都会新建当前日志文件。
    #[test]
    fn rotate_keeps_only_last_three_starts() {
        let dir = std::env::temp_dir().join(format!("dsh_rotate_test_{}", std::process::id()));
        let log = dir.join("dsh-web.log");
        let _ = fs::remove_dir_all(&dir);

        for i in 0..5 {
            // 每次启动前，当前日志写入上一批内容后轮转（与 sponsor 流程一致）
            write(&log, &format!("start {i} content\n"));
            rotate_service_log(&log, 3);
            // 轮转后当前文件应为空（尚未写入本次内容）
            assert_eq!(fs::read_to_string(&log).unwrap_or_default(), "");
            // 只允许保留 .0/.1/.2 三份
            assert!(!dir.join("dsh-web.log.3").exists());
            assert!(!dir.join("dsh-web.log.4").exists());
        }

        // 最后一次循环后：当前为空、.1 = start 4、.2 = start 3
        assert_eq!(fs::read_to_string(&log).unwrap_or_default(), "");
        assert!(fs::read_to_string(&dir.join("dsh-web.log.1"))
            .unwrap()
            .contains("start 4"));
        assert!(fs::read_to_string(&dir.join("dsh-web.log.2"))
            .unwrap()
            .contains("start 3"));
        assert!(!dir.join("dsh-web.log.3").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_probe_plugin_urls_use_revisions_declared_by_boot_html() {
        let html = r#"<link href="/plugins/??one/client.js,@deepseek-ai/dsh-client-ui-layout/client.js&amp;rev=combined"><script src="/plugins/??@deepseek-ai/dsh-client-modules/client.js&amp;rev=modules"></script><script>globalThis.__DSH_BOOT__={"entries":[{"url":"/plugins/??@deepseek-ai/dsh-client-ui-layout/client.js&rev=layout"}]}</script>"#;
        let urls = health_probe_plugin_urls(3080, html);
        assert_eq!(
            urls,
            vec![
                "http://127.0.0.1:3080/plugins/??@deepseek-ai/dsh-client-modules/client.js&rev=modules",
                "http://127.0.0.1:3080/plugins/??@deepseek-ai/dsh-client-ui-layout/client.js&rev=layout",
            ]
        );
        assert!(urls.iter().all(|u| u.contains("/plugins/")));
        assert!(urls.iter().all(|u| u.contains("&rev=")));
    }

    #[test]
    fn health_probe_plugin_urls_reject_boot_html_without_probe_bundles() {
        assert!(health_probe_plugin_urls(3080, "<!doctype html><html></html>").is_empty());
    }

    #[test]
    fn health_probe_plugin_urls_accept_legacy_single_package_routes() {
        let html = r#"<script src="/plugins/@deepseek-ai/dsh-client-modules/client.js?rev=modules"></script><script>globalThis.__DSH_BOOT__={"entries":[{"url":"/plugins/@deepseek-ai/dsh-client-ui-layout/client.js?rev=layout"}]}</script>"#;
        assert_eq!(
            health_probe_plugin_urls(3080, html),
            vec![
                "http://127.0.0.1:3080/plugins/@deepseek-ai/dsh-client-modules/client.js?rev=modules",
                "http://127.0.0.1:3080/plugins/@deepseek-ai/dsh-client-ui-layout/client.js?rev=layout",
            ]
        );
    }

    #[test]
    fn spa_html_fallback_is_not_a_plugin_bundle() {
        assert!(!looks_like_plugin_bundle(
            true,
            "<!doctype html><html lang=\"en\"><body>HARNESS Loading plugins...</body></html>"
        ));
        assert!(!looks_like_plugin_bundle(
            true,
            "<html><head></head></html>"
        ));
        assert!(!looks_like_plugin_bundle(true, "   "));
        assert!(!looks_like_plugin_bundle(
            false,
            "window.__ModuleLoader__={}"
        ));
        assert!(looks_like_plugin_bundle(
            true,
            "window.__ModuleLoader__.load({id:\"@deepseek-ai/dsh-client-ui-layout\"})"
        ));
    }

    /// keep=0 时把当前日志也删掉。
    #[test]
    fn rotate_with_keep_zero_removes_all() {
        let dir = std::env::temp_dir().join(format!("dsh_rotate_zero_{}", std::process::id()));
        let log = dir.join("dsh-web.log");
        let _ = fs::remove_dir_all(&dir);
        write(&log, "x");
        write(&dir.join("dsh-web.log.1"), "x");
        rotate_service_log(&log, 0);
        assert!(!log.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
