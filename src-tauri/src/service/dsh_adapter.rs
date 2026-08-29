//! DSH 发行版适配层。
//!
//! 上游 CLI 参数、认证协议、WebView 地址和 Desktop overlay 都由这里按版本选择。
//! workflow 只负责进程生命周期，React 只消费统一的 `RuntimeInfo.webview_url`。

use crate::config;
use semver::Version;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const NO_OPEN_MIN_VERSION: &str = "0.1.0-rc.8";
const AUTHENTICATED_WEB_MIN_VERSION: &str = "0.1.2-alpha.1";
const AUTHENTICATED_WEB_MAX_VERSION: &str = "0.1.3";

const AUTHENTICATED_WEB_OVERLAY: &str = "# Managed by Deepseek Harness Desktop.\n# This overlay adapts the official web profile without installing Desktop plugins.\n- id: web-runtime\n  config:\n    trustedHosts: !!js \"['dsh.tauri.localhost', ...ctx.webStartup.trustedHosts]\"\n";

/// 一代受支持的 DSH Web 启动协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    /// 无一次性浏览器 token；直接加载回环服务地址。
    LegacyWeb,
    /// stdout 发布一次性 token，认证 cookie 使用 SameSite=Strict。
    AuthenticatedWebV1,
}

/// 针对一个已安装 DSH 版本解析出的集中适配结果。
#[derive(Debug, Clone)]
pub struct DshAdapter {
    version: Version,
    family: Family,
    no_open: bool,
}

impl DshAdapter {
    /// 按安装包版本选择适配器；未知未来版本失败，不静默套用旧协议。
    pub fn resolve(version: &str) -> Result<Self, String> {
        let parsed = Version::parse(version).map_err(|error| {
            format!("DSH_ADAPTER_INVALID_VERSION: cannot parse {version:?}: {error}")
        })?;
        let no_open_min = Version::parse(NO_OPEN_MIN_VERSION).expect("valid adapter version");
        let auth_min =
            Version::parse(AUTHENTICATED_WEB_MIN_VERSION).expect("valid adapter version");
        let auth_max =
            Version::parse(AUTHENTICATED_WEB_MAX_VERSION).expect("valid adapter version");

        let family = if parsed >= auth_min && parsed < auth_max {
            Family::AuthenticatedWebV1
        } else if parsed < auth_min {
            Family::LegacyWeb
        } else {
            return Err(format!(
                "DSH_ADAPTER_UNSUPPORTED_VERSION: {version} is outside the tested range (< {AUTHENTICATED_WEB_MAX_VERSION})"
            ));
        };
        let no_open = parsed >= no_open_min;
        Ok(Self {
            version: parsed,
            family,
            no_open,
        })
    }

    /// 为当前激活核心解析适配器。
    pub fn active(app_handle: &AppHandle) -> Result<Self, String> {
        let version = crate::service::core::active_version(app_handle).ok_or_else(|| {
            "DSH_ADAPTER_VERSION_MISSING: active DSH version is unavailable".to_string()
        })?;
        Self::resolve(&version)
    }

    /// 可写入诊断日志的稳定适配器 id。
    pub fn id(&self) -> &'static str {
        match self.family {
            Family::LegacyWeb => "legacy-web",
            Family::AuthenticatedWebV1 => "authenticated-web-v1",
        }
    }

    /// Select the matching versioned internal-plugin artifact directory.
    pub(crate) fn plugin_family(&self) -> &'static str {
        self.id()
    }

    /// 当前适配器对应的已安装版本。
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// 此协议是否以一次性认证 URL 作为 Loader 就绪信号。
    pub fn requires_authenticated_url(&self) -> bool {
        self.family == Family::AuthenticatedWebV1
    }

    /// 写入应用私有的版本 overlay。官方 profile 的 `cordis.patch.yml` 不受影响。
    pub fn prepare_overlay(&self, app_handle: &AppHandle) -> Result<Option<PathBuf>, String> {
        let content = match self.family {
            Family::LegacyWeb => return Ok(None),
            Family::AuthenticatedWebV1 => AUTHENTICATED_WEB_OVERLAY,
        };
        let dir = config::get_base_dir(app_handle).join("dsh-adapters");
        fs::create_dir_all(&dir).map_err(|error| format!("DSH_ADAPTER_MKDIR: {error}"))?;
        let path = dir.join(format!("{}.cordis.yml", self.id()));
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
        if self.no_open {
            args.push(OsString::from("--no-open"));
        }
        args
    }

    /// 统一生成 WebView 导航地址；认证协议未发布 token 时返回 None。
    pub fn webview_url(&self, port: u16) -> Option<String> {
        match self.family {
            Family::LegacyWeb => Some(config::get_dsh_service_url(port)),
            Family::AuthenticatedWebV1 => {
                crate::service::workflow::utils::authenticated_webview_url(port)
            }
        }
    }
}

fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if fs::read_to_string(path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    fs::write(path, content).map_err(|error| format!("DSH_ADAPTER_WRITE: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_explicit_protocol_families() {
        let legacy = DshAdapter::resolve("0.1.0-rc.7").unwrap();
        assert_eq!(legacy.id(), "legacy-web");
        assert!(!legacy.no_open);

        let no_open = DshAdapter::resolve("0.1.0-rc.8").unwrap();
        assert_eq!(no_open.id(), "legacy-web");
        assert!(no_open.no_open);

        let authenticated = DshAdapter::resolve("0.1.2-alpha.1").unwrap();
        assert_eq!(authenticated.id(), "authenticated-web-v1");
        assert!(authenticated.no_open);
        assert!(authenticated.requires_authenticated_url());
    }

    #[test]
    fn rejects_unknown_future_protocols() {
        let error = DshAdapter::resolve("0.1.3").unwrap_err();
        assert!(error.contains("DSH_ADAPTER_UNSUPPORTED_VERSION"));
        assert!(DshAdapter::resolve("file:///dsh.tgz").is_err());
    }

    #[test]
    fn launch_args_are_complete_and_version_owned() {
        let adapter = DshAdapter::resolve("0.1.2-alpha.1").unwrap();
        let overlay = Path::new("C:/adapter/auth.cordis.yml");
        let args = adapter.launch_args(
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
                "C:/adapter/auth.cordis.yml",
                "--host",
                "127.0.0.1",
                "--port",
                "3081",
                "--no-open",
            ]
        );
    }

    #[test]
    fn authenticated_overlay_extends_the_authoritative_runtime_trust_list() {
        assert!(AUTHENTICATED_WEB_OVERLAY.contains("- id: web-runtime"));
        assert!(AUTHENTICATED_WEB_OVERLAY.contains("...ctx.webStartup.trustedHosts"));
        assert!(!AUTHENTICATED_WEB_OVERLAY.contains("- id: connection"));
    }
}
