//! 内嵌 Harness 的认证请求探针。
//!
//! Cookie 存入 WebView 不等于 iframe 会实际发送，尤其 macOS WebKit 对 SameSite
//! 属性的桥接存在平台差异。本脚本只在 localhost 子 frame 中执行一次真实受保护
//! API 请求，把 HTTP 状态回报宿主；宿主收到 200 前不会提交 ready。

pub(crate) const AUTH_PROBE_JS: &str = r#"(function () {
  if (window.top === window) return;
  if (location.protocol !== 'http:' || location.hostname !== 'localhost') return;
  var body = JSON.stringify({
    type: 'client-request',
    rpcId: 'desktop-auth-probe',
    method: 'settings/describe',
    payload: { args: {} }
  });
  fetch('/api/settings/describe', {
    method: 'POST',
    credentials: 'include',
    headers: { 'content-type': 'application/json' },
    body: body
  }).then(function (response) {
    window.parent.postMessage({ type: 'dsh://auth-probe', status: response.status }, '*');
  }).catch(function () {
    window.parent.postMessage({ type: 'dsh://auth-probe', status: 0 }, '*');
  });
})();"#;

pub(crate) const AUTH_TOP_LEVEL_PROBE_JS: &str = r#"(function () {
  if (window.top !== window) return;
  if (location.protocol !== 'http:' || location.hostname !== 'localhost') return;
  if (location.search !== '' || sessionStorage.getItem('dsh-desktop-auth-probed') === '1') return;
  sessionStorage.setItem('dsh-desktop-auth-probed', '1');
  var body = JSON.stringify({
    type: 'client-request',
    rpcId: 'desktop-auth-probe',
    method: 'settings/describe',
    payload: { args: {} }
  });
  fetch('/api/settings/describe', {
    method: 'POST',
    credentials: 'include',
    headers: { 'content-type': 'application/json' },
    body: body
  }).then(function (response) {
    location.href = 'dsh-auth-probe://localhost/?status=' + response.status;
  }).catch(function () {
    location.href = 'dsh-auth-probe://localhost/?status=0';
  });
})();"#;

#[cfg(test)]
mod tests {
    use super::AUTH_PROBE_JS;
    use super::AUTH_TOP_LEVEL_PROBE_JS;

    #[test]
    fn probe_executes_protected_request_and_reports_status() {
        assert!(AUTH_PROBE_JS.contains("fetch('/api/settings/describe'"));
        assert!(AUTH_PROBE_JS.contains("credentials: 'include'"));
        assert!(AUTH_PROBE_JS.contains("type: 'dsh://auth-probe'"));
        assert!(AUTH_PROBE_JS.contains("location.hostname !== 'localhost'"));
        assert!(AUTH_TOP_LEVEL_PROBE_JS.contains("window.top !== window"));
        assert!(AUTH_TOP_LEVEL_PROBE_JS.contains("dsh-auth-probe://localhost/"));
    }
}
