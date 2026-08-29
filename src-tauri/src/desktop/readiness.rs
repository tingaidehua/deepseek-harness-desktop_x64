//! Reports whether the embedded DSH frame reached its real application document.

/// Runs inside every child frame and emits a readiness message only after the DSH module loader
/// and a rendered document are both present. Browser-generated network error pages never satisfy
/// this condition, so the shell can distinguish an HTTP listener from a usable embedded page.
pub const FRAME_READINESS_JS: &str = r#"
(function () {
  if (window.top === window || window.__dsh_frame_readiness_probe__) return;
  window.__dsh_frame_readiness_probe__ = true;
  var attempts = 0;
  function report(state) {
    window.parent.postMessage({
      source: 'dsh-frame-readiness',
      type: 'dsh://frame-readiness',
      state: state,
      href: String(window.location.href),
      title: String(document.title || ''),
      loaderPresent: typeof window.__ModuleLoader__ === 'object' && window.__ModuleLoader__ !== null
    }, '*');
  }
  function inspect() {
    attempts += 1;
    var rendered = !!(document.body && document.body.childElementCount > 0);
    var loader = typeof window.__ModuleLoader__ === 'object' && window.__ModuleLoader__ !== null;
    if (rendered && loader) {
      report('ready');
      return;
    }
    if (attempts >= 120) {
      report('timeout');
      return;
    }
    setTimeout(inspect, 250);
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', inspect, { once: true });
  } else {
    inspect();
  }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_probe_requires_loader_and_child_frame() {
        assert!(FRAME_READINESS_JS.contains("window.top === window"));
        assert!(FRAME_READINESS_JS.contains("window.__ModuleLoader__"));
        assert!(FRAME_READINESS_JS.contains("dsh://frame-readiness"));
    }
}
