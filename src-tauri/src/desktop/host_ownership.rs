//! 在客户端插件启动前声明由 Desktop 持有的认证 DSH Host。

/// token-cookie-v1 协议使用同站 `.localhost` 别名，使官方 SameSite=Strict Cookie 能在 Tauri 中使用。
/// Desktop 持有只监听回环地址的 DSH 子进程，因此客户端可开放与 `127/8` 页面相同的 Host 设置面。
pub const HOST_OWNERSHIP_JS: &str = r#"
(function () {
  if (location.hostname !== 'dsh.tauri.localhost') return;
  var current = globalThis.__DSH_TRANSPORT__ || {};
  globalThis.__DSH_TRANSPORT__ = Object.assign({}, current, { ownsHost: true });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_is_limited_to_the_authenticated_desktop_authority() {
        assert!(HOST_OWNERSHIP_JS.contains("location.hostname !== 'dsh.tauri.localhost'"));
        assert!(HOST_OWNERSHIP_JS.contains("ownsHost: true"));
    }
}
