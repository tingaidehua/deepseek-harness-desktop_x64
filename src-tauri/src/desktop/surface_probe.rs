//! 在真实 DSH 子页面中执行可重复的功能面探针。

/// 探针验证插件的延迟资源、Desktop 对本地 Host 的所有权声明，以及页面可见故障。
/// 结果经父窗口转交给 Rust，命令行诊断无需操作界面即可读取。
pub const SURFACE_PROBE_JS: &str = r#"
(function () {
  if (window.top === window || window.__dsh_surface_probe__) return;
  window.__dsh_surface_probe__ = true;
  var failures = [];
  var checks = [];
  var contracts = globalThis.__DSH_DESKTOP_SURFACE_CONTRACTS__ || { resources: [], visibleFailurePatterns: [] };
  var knownFailures = contracts.visibleFailurePatterns;
  function remember(value) {
    var text = String(value || '').trim();
    if (!text || failures.indexOf(text) >= 0) return;
    failures.push(text.slice(0, 240));
  }
  addEventListener('error', function (event) {
    var target = event.target;
    if (target && target !== window && target.src) remember('resource failed: ' + new URL(target.src, location.href).pathname);
    else remember(event.message || 'window error');
  }, true);
  addEventListener('unhandledrejection', function (event) {
    remember(event.reason && event.reason.message ? event.reason.message : event.reason);
  });
  async function probe(id, path) {
    try {
      var response = await fetch(path, { credentials: 'same-origin', cache: 'no-store' });
      checks.push({ id: id, ok: response.ok, detail: path + ' -> ' + response.status });
    } catch (error) {
      checks.push({ id: id, ok: false, detail: path + ' -> ' + String(error) });
    }
  }
  async function run() {
    var loader = window.__ModuleLoader__;
    if (!loader || loader.mode !== 'live') {
      setTimeout(run, 250);
      return;
    }
    var body = String(document.body && document.body.innerText || '').toLowerCase();
    knownFailures.forEach(function (needle) {
      if (body.indexOf(needle) >= 0) remember('visible failure: ' + needle);
    });
    await Promise.all(contracts.resources.map(function (entry) { return probe(entry.id, entry.path); }));
    var transport = globalThis.__DSH_TRANSPORT__;
    window.parent.postMessage({
      source: 'dsh-surface-diagnostics',
      type: 'dsh://surface-diagnostics',
      origin: location.origin,
      loaderPresent: true,
      transportOwnsHost: transport && transport.ownsHost === true,
      checks: checks,
      failures: failures
    }, '*');
  }
  setTimeout(run, 0);
})();
"#;

/// 把同一份功能面清单注入浏览器，运行时探针与命令行遍历共用其断言定义。
pub const SURFACE_CONTRACTS_JS: &str = concat!(
    "globalThis.__DSH_DESKTOP_SURFACE_CONTRACTS__ = ",
    include_str!("../../resources/surface-contracts.json"),
    ";"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_covers_privileged_state_and_lazy_plugin_resources() {
        assert!(SURFACE_PROBE_JS.contains("transportOwnsHost"));
        assert!(SURFACE_CONTRACTS_JS.contains("/sidebar/bundle/terminal.js"));
        assert!(SURFACE_CONTRACTS_JS.contains("settings are unavailable in this browser"));
        assert!(SURFACE_PROBE_JS.contains("dsh://surface-diagnostics"));
    }
}
