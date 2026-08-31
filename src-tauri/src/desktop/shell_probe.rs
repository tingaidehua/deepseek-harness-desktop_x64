//! 在 React 入口执行前记录 Desktop 壳层的真实加载链。

/// 该脚本由 WebView 文档创建阶段注入，不依赖 Vite 入口或 React。即使模块脚本因
/// 内嵌资源缺失而完全没有执行，仍能通过 Tauri IPC 把资源错误写入独立诊断文件。
pub const SHELL_PROBE_JS: &str = r#"
(function () {
  if (window.top !== window || window.__dsh_desktop_shell_probe__) return;
  window.__dsh_desktop_shell_probe__ = true;
  function report(stage, detail, attempt) {
    var internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') {
      if ((attempt || 0) < 40) setTimeout(function () { report(stage, detail, (attempt || 0) + 1); }, 50);
      return;
    }
    var root = document.getElementById('root');
    internals.invoke('report_shell_diagnostics', {
      payload: {
        stage: stage,
        href: String(location.href),
        resource: detail && detail.resource ? String(detail.resource) : '',
        message: detail && detail.message ? String(detail.message) : '',
        rootChildCount: root ? root.childElementCount : 0
      }
    }).catch(function () {});
  }
  function resourcePath(target) {
    var candidate = target && (target.src || target.href);
    if (!candidate) return '';
    try { return new URL(candidate, location.href).pathname; } catch (_) { return String(candidate); }
  }
  addEventListener('error', function (event) {
    var resource = resourcePath(event.target);
    report(resource ? 'resource-error' : 'script-error', {
      resource: resource,
      message: event.message || 'window error'
    });
  }, true);
  addEventListener('unhandledrejection', function (event) {
    var reason = event.reason;
    report('unhandled-rejection', {
      message: reason && reason.message ? reason.message : String(reason || 'unhandled rejection')
    });
  });
  report('document-created', {});
  addEventListener('DOMContentLoaded', function () { report('dom-content-loaded', {}); }, { once: true });
  [250, 1000, 5000].forEach(function (delay) {
    setTimeout(function () {
      var root = document.getElementById('root');
      report(root && root.childElementCount > 0 ? 'react-mounted' : 'react-missing', {});
    }, delay);
  });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_probe_does_not_depend_on_react_entry() {
        assert!(SHELL_PROBE_JS.contains("window.__TAURI_INTERNALS__"));
        assert!(SHELL_PROBE_JS.contains("resource-error"));
        assert!(SHELL_PROBE_JS.contains("react-missing"));
        assert!(SHELL_PROBE_JS.contains("window.top !== window"));
    }
}
