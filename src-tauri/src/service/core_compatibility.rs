//! DSH 内核兼容记录。
//!
//! 每个受支持内核版本绑定一组可独立演进的协议能力。workflow 只负责进程生命周期，
//! 插件安装只消费制品集，React 只消费统一的 `RuntimeInfo.webview_url`。

use crate::config;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::AppHandle;

const TOKEN_COOKIE_OVERLAY: &str = "# Managed by Deepseek Harness Desktop.\n# This overlay adapts the official web profile without installing Desktop plugins.\n- id: web-runtime\n  config:\n    trustedHosts: !!js \"['dsh.tauri.localhost', ...ctx.webStartup.trustedHosts]\"\n";

/// DSH Web 服务的启动和导航协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebLaunchProtocol {
    /// Web 服务直接允许回环地址导航。
    DirectLoopbackV1,
    /// stdout 发布一次性 token，WebView 以认证 cookie 导航。
    TokenCookieV1,
}

/// 一个经过完整矩阵验证的内核版本及其协议能力。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompatibilityRecord {
    core_version: String,
    web_launch_protocol: String,
    client_abi: String,
    slot_protocol: String,
    workspace_navigation_protocol: String,
    workspace_archive_protocol: String,
    session_format: String,
    plugin_artifact_set: String,
    supports_no_open: bool,
    provides_win_terminal_inspector: bool,
}

fn supported_cores() -> &'static [CompatibilityRecord] {
    static RECORDS: OnceLock<Vec<CompatibilityRecord>> = OnceLock::new();
    RECORDS
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../resources/core-compatibility.json"))
                .expect("core-compatibility.json must contain valid compatibility records")
        })
        .as_slice()
}

/// 可序列化的内核能力摘要，供日志和自动化诊断使用。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreCapabilitySummary {
    pub core_version: String,
    pub web_launch_protocol: String,
    pub client_abi: String,
    pub slot_protocol: String,
    pub workspace_navigation_protocol: String,
    pub workspace_archive_protocol: String,
    pub session_format: String,
    pub plugin_artifact_set: String,
}

/// 针对一个已安装 DSH 版本解析出的精确兼容配置。
#[derive(Debug, Clone)]
pub struct CoreCompatibility {
    version: Version,
    record: &'static CompatibilityRecord,
}

impl CoreCompatibility {
    /// 仅接受经过完整验证的精确版本，不把未知版本推断为相邻协议。
    pub fn resolve(version: &str) -> Result<Self, String> {
        let parsed = Version::parse(version).map_err(|error| {
            format!("CORE_COMPATIBILITY_INVALID_VERSION: cannot parse {version:?}: {error}")
        })?;
        let record = supported_cores()
            .iter()
            .find(|record| record.core_version == version)
            .ok_or_else(|| {
                let supported = supported_cores()
                    .iter()
                    .map(|record| record.core_version.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "CORE_COMPATIBILITY_UNSUPPORTED_VERSION: {version} has no tested compatibility record; supported: {supported}"
                )
            })?;
        Ok(Self {
            version: parsed,
            record,
        })
    }

    /// 为当前激活内核解析兼容记录。
    pub fn active(app_handle: &AppHandle) -> Result<Self, String> {
        let version = crate::service::core::active_version(app_handle).ok_or_else(|| {
            let source = crate::service::core::active_source(app_handle);
            let slot = config::get_active_dsh_slot(app_handle);
            let install = config::get_dsh_install_path(app_handle);
            let entry = config::get_dsh_binary_path(app_handle);
            let package = install.join("node_modules/@deepseek-ai/dsh/package.json");
            log::error!(
                "CORE_COMPATIBILITY_VERSION_MISSING source={} active_slot={:?} install={} entry_exists={} package_exists={}",
                source.as_str(),
                slot,
                install.display(),
                entry.is_file(),
                package.is_file()
            );
            "CORE_COMPATIBILITY_VERSION_MISSING: active DSH version is unavailable".to_string()
        })?;
        Self::resolve(&version)
    }

    /// 当前兼容记录对应的已安装版本。
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Select the exact versioned plugin artifact directory.
    pub(crate) fn plugin_artifact_set(&self) -> &'static str {
        self.record.plugin_artifact_set.as_str()
    }

    /// 当前内核是否已经提供 Windows 终端检查能力。
    pub(crate) fn provides_win_terminal_inspector(&self) -> bool {
        self.record.provides_win_terminal_inspector
    }

    /// 返回可供诊断和测试断言的完整能力摘要。
    pub fn capability_summary(&self) -> CoreCapabilitySummary {
        CoreCapabilitySummary {
            core_version: self.version.to_string(),
            web_launch_protocol: self.web_launch_protocol_id().to_string(),
            client_abi: self.record.client_abi.clone(),
            slot_protocol: self.record.slot_protocol.clone(),
            workspace_navigation_protocol: self.record.workspace_navigation_protocol.clone(),
            workspace_archive_protocol: self.record.workspace_archive_protocol.clone(),
            session_format: self.record.session_format.clone(),
            plugin_artifact_set: self.record.plugin_artifact_set.clone(),
        }
    }

    /// 此协议是否以一次性认证 URL 作为 Loader 就绪信号。
    pub fn requires_authenticated_url(&self) -> bool {
        self.web_launch_protocol() == WebLaunchProtocol::TokenCookieV1
    }

    /// 写入应用私有的协议 overlay。官方 profile 的 `cordis.patch.yml` 不受影响。
    pub fn prepare_overlay(&self, app_handle: &AppHandle) -> Result<Option<PathBuf>, String> {
        let (name, content) = match self.web_launch_protocol() {
            WebLaunchProtocol::DirectLoopbackV1 => return Ok(None),
            WebLaunchProtocol::TokenCookieV1 => ("token-cookie-v1", TOKEN_COOKIE_OVERLAY),
        };
        let dir = config::get_base_dir(app_handle).join("dsh-compatibility");
        fs::create_dir_all(&dir).map_err(|error| format!("CORE_COMPATIBILITY_MKDIR: {error}"))?;
        let path = dir.join(format!("web-launch-{name}.cordis.yml"));
        write_if_changed(&path, content)?;
        Ok(Some(path))
    }

    /// 构造完整 Node argv（首项为 dsh bin），供各平台 spawn 实现共用。
    pub fn launch_args(
        &self,
        dsh_binary: &Path,
        profile: &str,
        port: u16,
        overlay: Option<&Path>,
    ) -> Vec<OsString> {
        let mut args = vec![
            dsh_binary.as_os_str().to_os_string(),
            OsString::from("--profile"),
            OsString::from(profile),
        ];
        if let Some(path) = overlay {
            args.push(OsString::from("--patch"));
            args.push(path.as_os_str().to_os_string());
        }
        args.extend([
            OsString::from("--host"),
            OsString::from("127.0.0.1"),
            OsString::from("--port"),
            OsString::from(port.to_string()),
        ]);
        if self.record.supports_no_open {
            args.push(OsString::from("--no-open"));
        }
        args
    }

    /// 统一生成 WebView 导航地址；认证协议未发布 token 时返回 None。
    pub fn webview_url(&self, port: u16) -> Option<String> {
        match self.web_launch_protocol() {
            WebLaunchProtocol::DirectLoopbackV1 => Some(config::get_dsh_service_url(port)),
            WebLaunchProtocol::TokenCookieV1 => {
                crate::service::workflow::utils::authenticated_webview_url(port)
            }
        }
    }

    fn web_launch_protocol_id(&self) -> &'static str {
        match self.web_launch_protocol() {
            WebLaunchProtocol::DirectLoopbackV1 => "direct-loopback-v1",
            WebLaunchProtocol::TokenCookieV1 => "token-cookie-v1",
        }
    }

    fn web_launch_protocol(&self) -> WebLaunchProtocol {
        match self.record.web_launch_protocol.as_str() {
            "direct-loopback-v1" => WebLaunchProtocol::DirectLoopbackV1,
            "token-cookie-v1" => WebLaunchProtocol::TokenCookieV1,
            protocol => {
                panic!("unsupported web launch protocol in compatibility manifest: {protocol}")
            }
        }
    }
}

fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if fs::read_to_string(path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    fs::write(path, content).map_err(|error| format!("CORE_COMPATIBILITY_WRITE: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_only_exact_supported_versions() {
        let rc2 = CoreCompatibility::resolve("0.1.1-rc.2").unwrap();
        assert_eq!(rc2.plugin_artifact_set(), "dsh-v0.1.1-rc.2");
        assert!(!rc2.requires_authenticated_url());

        let alpha = CoreCompatibility::resolve("0.1.2-alpha.1").unwrap();
        assert_eq!(alpha.plugin_artifact_set(), "dsh-v0.1.2-alpha.1");
        assert!(alpha.requires_authenticated_url());
        assert_eq!(alpha.capability_summary().client_abi, "split-client-v1");
        assert_eq!(
            alpha.capability_summary().workspace_navigation_protocol,
            "ui-workspace-v1"
        );
        assert_eq!(
            alpha.capability_summary().workspace_archive_protocol,
            "core-native-v1"
        );
    }

    #[test]
    fn rejects_unverified_neighbors_and_invalid_specs() {
        for version in ["0.1.0-rc.8", "0.1.1-rc.1", "0.1.2-alpha.2", "0.1.3"] {
            let error = CoreCompatibility::resolve(version).unwrap_err();
            assert!(error.contains("CORE_COMPATIBILITY_UNSUPPORTED_VERSION"));
        }
        assert!(CoreCompatibility::resolve("file:///dsh.tgz").is_err());
    }

    #[test]
    fn launch_args_are_complete_and_version_owned() {
        let compatibility = CoreCompatibility::resolve("0.1.2-alpha.1").unwrap();
        let overlay = Path::new("C:/compat/token-cookie.cordis.yml");
        let args = compatibility.launch_args(
            Path::new("C:/dsh/lib/bin.js"),
            "product",
            3081,
            Some(overlay),
        );
        let rendered: Vec<String> = args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "C:/dsh/lib/bin.js",
                "--profile",
                "product",
                "--patch",
                "C:/compat/token-cookie.cordis.yml",
                "--host",
                "127.0.0.1",
                "--port",
                "3081",
                "--no-open",
            ]
        );
    }

    #[test]
    fn token_cookie_overlay_extends_the_authoritative_runtime_trust_list() {
        assert!(TOKEN_COOKIE_OVERLAY.contains("- id: web-runtime"));
        assert!(TOKEN_COOKIE_OVERLAY.contains("...ctx.webStartup.trustedHosts"));
        assert!(!TOKEN_COOKIE_OVERLAY.contains("- id: connection"));
    }

    #[test]
    fn manifest_records_are_unique_and_complete() {
        let records = supported_cores();
        for (index, record) in records.iter().enumerate() {
            Version::parse(&record.core_version).expect("core version must be semver");
            assert!(record.plugin_artifact_set.starts_with("dsh-v"));
            assert!(!record.client_abi.is_empty());
            assert!(!record.slot_protocol.is_empty());
            assert!(!record.workspace_navigation_protocol.is_empty());
            assert!(!record.session_format.is_empty());
            let compatibility = CoreCompatibility::resolve(&record.core_version).unwrap();
            compatibility.web_launch_protocol();
            assert!(records[index + 1..]
                .iter()
                .all(|other| other.core_version != record.core_version
                    && other.plugin_artifact_set != record.plugin_artifact_set));
        }
    }
}
