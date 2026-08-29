//! 面向自动化代理的桌面端本地控制面。
//!
//! 控制面监听随机回环端口，以带随机令牌的逐行 JSON 协议暴露只读诊断、日志、
//! 压测与明确列出的恢复操作。它不经过 WebView，因此页面空白、插件装载失败时仍
//! 可用；DSH 内置客户端运行在独立 Node 进程中，Desktop 崩溃后仍可读取落盘信息
//! 并重新拉起应用。

use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const PROTOCOL: &str = "dsh-desktop-control-jsonl-v1";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_TRACE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRequest {
    token: String,
    operation: String,
    #[serde(default)]
    args: Value,
    trace_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlResponse {
    trace_id: String,
    ok: bool,
    result: Option<Value>,
    error: Option<String>,
    duration_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EndpointRecord {
    protocol: &'static str,
    host: &'static str,
    port: u16,
    token: String,
    pid: u32,
    executable: String,
    trace_file: String,
    started_at_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationDescription {
    id: &'static str,
    mutating: bool,
    description: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceRecord<'a> {
    trace_id: &'a str,
    operation: &'a str,
    pid: u32,
    ok: bool,
    duration_ms: u128,
    error: Option<&'a str>,
    finished_at_ms: u128,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// 控制面连接记录路径；注入 DSH 子进程供内置插件读取。
pub fn endpoint_path(app_handle: &AppHandle) -> PathBuf {
    crate::config::get_base_dir(app_handle)
        .join("control")
        .join("endpoint.json")
}

/// 结构化控制轨迹路径；Desktop 不可用时插件仍可直接读取。
pub fn trace_path(app_handle: &AppHandle) -> PathBuf {
    crate::config::get_base_dir(app_handle)
        .join("logs")
        .join("desktop-control-trace.jsonl")
}

fn operation_catalog() -> [OperationDescription; 9] {
    [
        OperationDescription {
            id: "control.catalog",
            mutating: false,
            description: "列出协议和可调用操作",
        },
        OperationDescription {
            id: "diagnostics.snapshot",
            mutating: false,
            description: "读取核心、插件和真实页面功能面快照",
        },
        OperationDescription {
            id: "runtime.info",
            mutating: false,
            description: "读取 Desktop、DSH、Node 和数据目录信息",
        },
        OperationDescription {
            id: "runtime.health",
            mutating: false,
            description: "检查当前 DSH 服务健康状态",
        },
        OperationDescription {
            id: "core.list",
            mutating: false,
            description: "列出可用核心及当前选择",
        },
        OperationDescription {
            id: "profile.list",
            mutating: false,
            description: "列出档案及当前选择",
        },
        OperationDescription {
            id: "logs.bundle",
            mutating: false,
            description: "读取服务、前台和后台日志尾部",
        },
        OperationDescription {
            id: "trace.read",
            mutating: false,
            description: "读取控制面结构化调用轨迹",
        },
        OperationDescription {
            id: "stress.snapshot",
            mutating: false,
            description: "并发执行诊断快照并返回延迟与失败统计",
        },
    ]
}

fn generated_token() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("CONTROL_DIR_CREATE: {error}"))?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(|error| format!("CONTROL_FILE_WRITE: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("CONTROL_FILE_PERMISSIONS: {error}"))?;
    }
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| format!("CONTROL_FILE_REMOVE: {error}"))?;
    }
    std::fs::rename(&temporary, path).map_err(|error| format!("CONTROL_FILE_REPLACE: {error}"))
}

fn read_tail(path: &Path, max_bytes: usize) -> Result<String, String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let bytes = std::fs::read(path).map_err(|error| format!("CONTROL_TRACE_READ: {error}"))?;
    let start = bytes.len().saturating_sub(max_bytes);
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

fn append_trace(app_handle: &AppHandle, record: &TraceRecord<'_>) {
    let path = trace_path(app_handle);
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            log::warn!("CONTROL_TRACE_DIR_CREATE: {error}");
            return;
        }
    }
    if path
        .metadata()
        .map(|metadata| metadata.len() > MAX_TRACE_BYTES)
        .unwrap_or(false)
    {
        let rotated = path.with_extension("jsonl.previous");
        let _ = std::fs::remove_file(&rotated);
        if let Err(error) = std::fs::rename(&path, rotated) {
            log::warn!("CONTROL_TRACE_ROTATE: {error}");
        }
    }
    let Ok(mut line) = serde_json::to_vec(record) else {
        return;
    };
    line.push(b'\n');
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            if let Err(error) = file.write_all(&line) {
                log::warn!("CONTROL_TRACE_WRITE: {error}");
            }
        }
        Err(error) => log::warn!("CONTROL_TRACE_OPEN: {error}"),
    }
}

fn arg_usize(args: &Value, key: &str, default: usize, maximum: usize) -> Result<usize, String> {
    let value = args
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or(default as u64);
    if value == 0 || value > maximum as u64 {
        return Err(format!(
            "CONTROL_INVALID_ARGUMENT: {key} must be between 1 and {maximum}"
        ));
    }
    Ok(value as usize)
}

async fn stress_snapshot(app_handle: AppHandle, args: &Value) -> Result<Value, String> {
    let iterations = arg_usize(args, "iterations", 20, 1_000)?;
    let concurrency = arg_usize(args, "concurrency", 4, 64)?.min(iterations);
    let mut pending = FuturesUnordered::new();
    let mut next = 0usize;
    let mut durations = Vec::with_capacity(iterations);
    let mut failures = Vec::new();

    while next < iterations || !pending.is_empty() {
        while next < iterations && pending.len() < concurrency {
            let app = app_handle.clone();
            let index = next;
            pending.push(tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result = crate::diagnostics::active_snapshot(&app);
                (index, started.elapsed().as_micros() as u64, result)
            }));
            next += 1;
        }
        if let Some(joined) = pending.next().await {
            match joined {
                Ok((index, duration, result)) => {
                    durations.push(duration);
                    if let Err(error) = result {
                        failures.push(json!({ "iteration": index, "error": error }));
                    }
                }
                Err(error) => {
                    durations.push(0);
                    failures.push(json!({ "iteration": null, "error": format!("CONTROL_STRESS_JOIN: {error}") }));
                }
            }
        }
    }
    durations.sort_unstable();
    let percentile = |percent: usize| -> u64 {
        let index = ((durations.len() - 1) * percent / 100).min(durations.len() - 1);
        durations[index]
    };
    Ok(json!({
        "iterations": iterations,
        "concurrency": concurrency,
        "successes": iterations - failures.len(),
        "failures": failures,
        "latencyMicros": {
            "p50": percentile(50),
            "p95": percentile(95),
            "max": durations.last().copied().unwrap_or_default()
        }
    }))
}

async fn dispatch(app_handle: AppHandle, operation: &str, args: &Value) -> Result<Value, String> {
    match operation {
        "control.catalog" => Ok(json!({ "protocol": PROTOCOL, "operations": operation_catalog() })),
        "diagnostics.snapshot" => {
            serde_json::to_value(crate::diagnostics::active_snapshot(&app_handle)?)
                .map_err(|error| format!("CONTROL_SERIALIZE: {error}"))
        }
        "runtime.info" => {
            let port = crate::config::get_store_dat_setting(&app_handle).port;
            serde_json::to_value(crate::config::runtime_info(&app_handle, port))
                .map_err(|error| format!("CONTROL_SERIALIZE: {error}"))
        }
        "runtime.health" => {
            let port = crate::config::get_store_dat_setting(&app_handle).port;
            Ok(
                json!({ "message": crate::service::workflow::proxy_health_check(&app_handle, port).await? }),
            )
        }
        "core.list" => serde_json::to_value(crate::service::core::list(&app_handle).await)
            .map_err(|error| format!("CONTROL_SERIALIZE: {error}")),
        "profile.list" => serde_json::to_value(crate::service::profile::list(&app_handle))
            .map_err(|error| format!("CONTROL_SERIALIZE: {error}")),
        "logs.bundle" => Ok(json!({ "text": crate::bridge::read_run_logs(app_handle).await? })),
        "trace.read" => {
            let max_bytes = arg_usize(args, "maxBytes", 128 * 1024, 1024 * 1024)?;
            Ok(json!({ "text": read_tail(&trace_path(&app_handle), max_bytes)? }))
        }
        "stress.snapshot" => stress_snapshot(app_handle, args).await,
        _ => Err(format!("CONTROL_UNKNOWN_OPERATION: {operation}")),
    }
}

async fn handle_connection(app_handle: AppHandle, token: String, stream: TcpStream) {
    let (reader, mut writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut reader = reader.take((MAX_REQUEST_BYTES + 1) as u64);
    let mut line = String::new();
    let read = reader.read_line(&mut line).await;
    let response = match read {
        Ok(size) if size > MAX_REQUEST_BYTES => ControlResponse {
            trace_id: format!("control-{}", now_ms()),
            ok: false,
            result: None,
            error: Some("CONTROL_REQUEST_TOO_LARGE".to_string()),
            duration_ms: 0,
        },
        Ok(0) => return,
        Err(error) => ControlResponse {
            trace_id: format!("control-{}", now_ms()),
            ok: false,
            result: None,
            error: Some(format!("CONTROL_REQUEST_READ: {error}")),
            duration_ms: 0,
        },
        Ok(_) => match serde_json::from_str::<ControlRequest>(&line) {
            Err(error) => ControlResponse {
                trace_id: format!("control-{}", now_ms()),
                ok: false,
                result: None,
                error: Some(format!("CONTROL_REQUEST_PARSE: {error}")),
                duration_ms: 0,
            },
            Ok(request) => {
                let trace_id = request
                    .trace_id
                    .unwrap_or_else(|| format!("control-{}", now_ms()));
                let started = Instant::now();
                let result = if request.token == token {
                    dispatch(app_handle.clone(), &request.operation, &request.args).await
                } else {
                    Err("CONTROL_UNAUTHORIZED".to_string())
                };
                let duration_ms = started.elapsed().as_millis();
                let response = match result {
                    Ok(value) => ControlResponse {
                        trace_id,
                        ok: true,
                        result: Some(value),
                        error: None,
                        duration_ms,
                    },
                    Err(error) => ControlResponse {
                        trace_id,
                        ok: false,
                        result: None,
                        error: Some(error),
                        duration_ms,
                    },
                };
                append_trace(
                    &app_handle,
                    &TraceRecord {
                        trace_id: &response.trace_id,
                        operation: &request.operation,
                        pid: std::process::id(),
                        ok: response.ok,
                        duration_ms,
                        error: response.error.as_deref(),
                        finished_at_ms: now_ms(),
                    },
                );
                response
            }
        },
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        bytes.push(b'\n');
        let _ = writer.write_all(&bytes).await;
    }
}

async fn serve(app_handle: AppHandle, endpoint: PathBuf) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("CONTROL_BIND: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("CONTROL_ADDRESS: {error}"))?
        .port();
    let token = generated_token();
    let record = EndpointRecord {
        protocol: PROTOCOL,
        host: "127.0.0.1",
        port,
        token: token.clone(),
        pid: std::process::id(),
        executable: std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        trace_file: trace_path(&app_handle).to_string_lossy().into_owned(),
        started_at_ms: now_ms(),
    };
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| format!("CONTROL_ENDPOINT_SERIALIZE: {error}"))?;
    write_private_file(&endpoint, &bytes)?;
    log::info!("Desktop control plane listening on 127.0.0.1:{port}");
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app = app_handle.clone();
                let token = token.clone();
                tauri::async_runtime::spawn(handle_connection(app, token, stream));
            }
            Err(error) => log::error!("CONTROL_ACCEPT: {error}"),
        }
    }
}

/// 注册随机回环端口控制面，并返回将要发布的连接记录路径。
pub fn start(app_handle: AppHandle) -> Result<PathBuf, String> {
    let endpoint = endpoint_path(&app_handle);
    if let Some(parent) = endpoint.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("CONTROL_DIR_CREATE: {error}"))?;
    }
    let returned = endpoint.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = serve(app_handle, endpoint).await {
            log::error!("Desktop control plane stopped: {error}");
        }
    });
    Ok(returned)
}

#[cfg(test)]
mod tests {
    use super::{arg_usize, generated_token, operation_catalog, PROTOCOL};
    use serde_json::json;

    #[test]
    fn catalog_has_unique_versioned_operations() {
        let catalog = operation_catalog();
        let mut ids = catalog
            .iter()
            .map(|operation| operation.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), catalog.len());
        assert!(PROTOCOL.ends_with("-v1"));
    }

    #[test]
    fn numeric_arguments_are_bounded() {
        assert_eq!(arg_usize(&json!({}), "iterations", 20, 1000).unwrap(), 20);
        assert!(arg_usize(&json!({ "iterations": 0 }), "iterations", 20, 1000).is_err());
        assert!(arg_usize(&json!({ "iterations": 1001 }), "iterations", 20, 1000).is_err());
    }

    #[test]
    fn authentication_token_uses_256_bits_of_os_randomness() {
        let token = generated_token();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
