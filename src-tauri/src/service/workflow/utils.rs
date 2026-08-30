use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

const DSH_MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const DSH_MAX_BACKUPS: usize = 3;
static DSH_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static HARNESS_LAUNCH_URL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static PENDING_HARNESS_LAUNCH_URL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn dsh_log_lock() -> &'static Mutex<()> {
    DSH_LOG_LOCK.get_or_init(|| Mutex::new(()))
}

fn harness_launch_url_slot() -> &'static Mutex<Option<String>> {
    HARNESS_LAUNCH_URL.get_or_init(|| Mutex::new(None))
}

fn pending_harness_launch_url_slot() -> &'static Mutex<Option<String>> {
    PENDING_HARNESS_LAUNCH_URL.get_or_init(|| Mutex::new(None))
}

/// 清除上一进程的认证 URL，防止核心重启后复用已经失效的 token。
pub fn clear_harness_launch_url() {
    *harness_launch_url_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    *pending_harness_launch_url_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
}

/// 返回当前进程为指定端口公布的认证 URL。
pub fn harness_launch_url(port: u16) -> Option<String> {
    let value = harness_launch_url_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()?;
    let parsed = reqwest::Url::parse(&value).ok()?;
    (parsed.host_str() == Some("127.0.0.1") && parsed.port_or_known_default() == Some(port))
        .then_some(value)
}

/// 一次性交给尚未挂载的宿主 iframe；取走后远端页面无法再通过 IPC 获取 token。
pub fn take_harness_launch_url(port: u16) -> Option<String> {
    let value = pending_harness_launch_url_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()?;
    let parsed = reqwest::Url::parse(&value).ok()?;
    (parsed.host_str() == Some("127.0.0.1") && parsed.port_or_known_default() == Some(port))
        .then_some(value)
}

/// 捕获 dsh 公布的一次性浏览器认证 URL，并返回适合写日志的脱敏文本。
fn capture_and_redact_launch_url(line: &str) -> String {
    let Some(candidate) = line
        .strip_prefix("dsh web: ")
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return line.to_string();
    };
    let Ok(parsed) = reqwest::Url::parse(candidate) else {
        return line.to_string();
    };
    let token = parsed
        .query_pairs()
        .find_map(|(name, value)| (name == "token").then(|| value.into_owned()));
    let Some(token) = token.filter(|value| !value.is_empty()) else {
        return line.to_string();
    };
    if parsed.scheme() != "http" || parsed.host_str() != Some("127.0.0.1") {
        return line.to_string();
    }
    *harness_launch_url_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(candidate.to_string());
    *pending_harness_launch_url_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(candidate.to_string());
    line.replace(&token, "<redacted>")
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

/// 客户端插件 bundle 探测地址。
///
/// SPA `/` 在 webServer 绑定后立刻 200，此时连接桥与 Loader 图往往还没就绪；
/// WebView 若在这个窗口加载，会永久停在官方 boot 页 “Loading plugins…”。
/// 必须等到真实 JS bundle（而非 HTML fallback）可取，才视为可挂载 iframe。
pub(super) fn health_probe_plugin_urls(port: u16) -> Vec<String> {
    vec![
        format!(
            "http://127.0.0.1:{port}/plugins/@deepseek-ai/dsh-client-ui-layout/client.js"
        ),
        format!(
            "http://127.0.0.1:{port}/plugins/@deepseek-ai/dsh-client-runtime/client.js"
        ),
    ]
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

    if let Some(url) = harness_launch_url(port) {
        return client
            .get(url)
            .send()
            .await
            .map(|response| {
                response.status().is_success()
                    || response.status().is_redirection()
                    || response.status() == reqwest::StatusCode::UNAUTHORIZED
            })
            .unwrap_or(false);
    }

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
pub fn spawn_output_readers<R1, R2>(stdout: Option<R1>, stderr: Option<R2>, log_path: PathBuf)
where
    R1: Read + Send + 'static,
    R2: Read + Send + 'static,
{
    // 在独立线程中读取 stdout
    if let Some(stdout) = stdout {
        let log_path = log_path.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let safe_line = capture_and_redact_launch_url(&line);
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
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let safe_line = capture_and_redact_launch_url(&line);
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
            let _ = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(log_path);
        }
    }
}

/// 轮转日志文件名：`dsh-web.log`（index 0）、`dsh-web.log.1`、`dsh-web.log.2`……
fn indexed_log_path(log_path: &PathBuf, index: usize) -> PathBuf {
    if index == 0 {
        return log_path.clone();
    }
    let mut name = log_path
        .file_name()
        .unwrap_or_default()
        .to_os_string();
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

    /// 模拟连续 5 次启动，验证磁盘上始终只保留最近 `keep` 份日志，
    /// 且每次启动都会新建当前日志文件。
    #[test]
    fn rotate_keeps_only_last_three_starts() {
        let dir = std::env::temp_dir().join(format!(
            "dsh_rotate_test_{}",
            std::process::id()
        ));
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
    fn health_probe_plugin_urls_target_client_bundles_not_spa_root() {
        let urls = health_probe_plugin_urls(3080);
        assert!(urls.iter().all(|u| u.contains("/plugins/")));
        assert!(urls
            .iter()
            .all(|u| !u.ends_with("3080/") && !u.ends_with("://127.0.0.1:3080")));
        assert!(
            urls.iter()
                .any(|u| u.contains("dsh-client-ui-layout/client.js"))
        );
    }

    #[test]
    fn spa_html_fallback_is_not_a_plugin_bundle() {
        assert!(!looks_like_plugin_bundle(
            true,
            "<!doctype html><html lang=\"en\"><body>HARNESS Loading plugins...</body></html>"
        ));
        assert!(!looks_like_plugin_bundle(true, "<html><head></head></html>"));
        assert!(!looks_like_plugin_bundle(true, "   "));
        assert!(!looks_like_plugin_bundle(false, "window.__ModuleLoader__={}"));
        assert!(looks_like_plugin_bundle(
            true,
            "window.__ModuleLoader__.load({id:\"@deepseek-ai/dsh-client-ui-layout\"})"
        ));
    }

    #[test]
    fn authenticated_launch_url_is_captured_and_redacted() {
        clear_harness_launch_url();
        let safe = capture_and_redact_launch_url(
            "dsh web: http://127.0.0.1:3083/?token=secret-value",
        );
        assert_eq!(
            safe,
            "dsh web: http://127.0.0.1:3083/?token=<redacted>"
        );
        assert_eq!(
            harness_launch_url(3083).as_deref(),
            Some("http://127.0.0.1:3083/?token=secret-value")
        );
        assert_eq!(
            take_harness_launch_url(3083).as_deref(),
            Some("http://127.0.0.1:3083/?token=secret-value")
        );
        assert_eq!(take_harness_launch_url(3083), None);
        assert_eq!(harness_launch_url(3080), None);
        clear_harness_launch_url();
    }

    #[test]
    fn unrelated_urls_are_not_treated_as_harness_credentials() {
        clear_harness_launch_url();
        let line = "dsh web: https://example.com/?token=secret-value";
        assert_eq!(capture_and_redact_launch_url(line), line);
        assert_eq!(harness_launch_url(443), None);
    }

    /// keep=0 时把当前日志也删掉。
    #[test]
    fn rotate_with_keep_zero_removes_all() {
        let dir = std::env::temp_dir().join(format!(
            "dsh_rotate_zero_{}",
            std::process::id()
        ));
        let log = dir.join("dsh-web.log");
        let _ = fs::remove_dir_all(&dir);
        write(&log, "x");
        write(&dir.join("dsh-web.log.1"), "x");
        rotate_service_log(&log, 0);
        assert!(!log.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
