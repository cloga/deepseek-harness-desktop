use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

const DSH_MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const DSH_MAX_BACKUPS: usize = 3;
static DSH_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Default)]
struct HarnessLaunchState {
    next_generation: u64,
    owner: Option<HarnessLaunchOwner>,
}

struct HarnessLaunchOwner {
    generation: u64,
    pid: Option<u32>,
    url: Option<String>,
    authenticated_port: Option<u16>,
    webview_pending: bool,
    browser_pending: bool,
    in_flight: Option<HarnessLaunchConsumer>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HarnessLaunchConsumer {
    Webview,
    Browser,
}

pub struct HarnessLaunchClaim {
    generation: u64,
    pid: u32,
    url: String,
    consumer: HarnessLaunchConsumer,
}

#[derive(Clone, Copy, serde::Serialize)]
pub struct HarnessLaunchClaimId {
    pub generation: u64,
    pub pid: u32,
}

impl HarnessLaunchClaim {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn id(&self) -> HarnessLaunchClaimId {
        HarnessLaunchClaimId {
            generation: self.generation,
            pid: self.pid,
        }
    }
}

fn dsh_log_lock() -> &'static Mutex<()> {
    DSH_LOG_LOCK.get_or_init(|| Mutex::new(()))
}

fn harness_launch_state() -> &'static Mutex<HarnessLaunchState> {
    static HARNESS_LAUNCH_STATE: OnceLock<Mutex<HarnessLaunchState>> = OnceLock::new();
    HARNESS_LAUNCH_STATE.get_or_init(|| Mutex::new(HarnessLaunchState::default()))
}

/// 为一次宿主启动分配进程内单调递增的代号，阻止旧启动流程取走新 URL。
pub fn begin_harness_launch_generation() -> u64 {
    let mut state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state.next_generation = state.next_generation.wrapping_add(1).max(1);
    let generation = state.next_generation;
    state.owner = Some(HarnessLaunchOwner {
        generation,
        pid: None,
        url: None,
        authenticated_port: None,
        webview_pending: false,
        browser_pending: false,
        in_flight: None,
    });
    generation
}

/// 将启动代与真实子进程 PID 原子绑定，旧 reader 随后不能写入新进程状态。
pub fn bind_harness_launch_process(generation: u64, pid: u32) -> bool {
    let mut state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(owner) = state.owner.as_mut() else {
        return false;
    };
    if owner.generation != generation {
        return false;
    }
    owner.pid = Some(pid);
    true
}

/// 返回当前进程启动代；重复 launch 命中已持有进程时复用。
pub fn current_harness_launch_generation() -> Option<u64> {
    harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .owner
        .as_ref()
        .map(|owner| owner.generation)
}

/// 清除上一进程的认证 URL，防止核心重启后复用已经失效的 token。
pub fn clear_harness_launch_url() {
    harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .owner = None;
}

/// 仅当退出 PID 仍属于当前 URL owner 时清理，旧进程退出不能抹掉替代进程状态。
pub fn clear_harness_launch_url_if_pid(pid: u32) -> bool {
    let mut state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state.owner.as_ref().and_then(|owner| owner.pid) != Some(pid) {
        return false;
    }
    state.owner = None;
    true
}

/// 当前进程是否已为指定端口公布认证启动 URL。
pub fn has_authenticated_harness_launch(port: u16) -> bool {
    let state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state
        .owner
        .as_ref()
        .is_some_and(|owner| owner.authenticated_port == Some(port))
}

fn claim_harness_launch(
    port: u16,
    generation: Option<u64>,
    consumer: HarnessLaunchConsumer,
) -> Result<Option<HarnessLaunchClaim>, String> {
    let mut state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(owner) = state.owner.as_mut() else {
        return Ok(None);
    };
    if generation.is_some_and(|generation| owner.generation != generation) {
        return Err("HARNESS_AUTH_OWNER_MISMATCH: stale launch generation".into());
    }
    let Some(pid) = owner.pid else {
        return Err("HARNESS_AUTH_OWNER_UNBOUND: launch process is not bound".into());
    };
    let pending = match consumer {
        HarnessLaunchConsumer::Webview => owner.webview_pending,
        HarnessLaunchConsumer::Browser => owner.browser_pending,
    };
    if !pending {
        return Ok(None);
    }
    if owner.in_flight.is_some() {
        return Err("HARNESS_AUTH_HANDOFF_BUSY: another authentication delivery is active".into());
    }
    let Some(value) = owner.url.as_ref() else {
        return Ok(None);
    };
    let parsed = reqwest::Url::parse(value)
        .map_err(|_| "HARNESS_AUTH_URL_INVALID: captured launch URL is invalid".to_string())?;
    if parsed.host_str() != Some("127.0.0.1") || parsed.port_or_known_default() != Some(port) {
        return Err("HARNESS_AUTH_PORT_MISMATCH: launch URL does not match active port".into());
    }
    owner.in_flight = Some(consumer);
    Ok(Some(HarnessLaunchClaim {
        generation: owner.generation,
        pid,
        url: value.clone(),
        consumer,
    }))
}

pub fn harness_webview_handoff_completed(port: u16, generation: u64) -> bool {
    let state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state.owner.as_ref().is_some_and(|owner| {
        owner.generation == generation
            && owner.authenticated_port == Some(port)
            && !owner.webview_pending
            && owner.in_flight != Some(HarnessLaunchConsumer::Webview)
    })
}

pub fn claim_harness_webview_launch(
    port: u16,
    generation: u64,
) -> Result<Option<HarnessLaunchClaim>, String> {
    claim_harness_launch(port, Some(generation), HarnessLaunchConsumer::Webview)
}

pub fn claim_harness_browser_launch(port: u16) -> Result<Option<HarnessLaunchClaim>, String> {
    claim_harness_launch(port, None, HarnessLaunchConsumer::Browser)
}

/// 成功后才消费交付权；失败释放 claim，允许同一 generation/PID 安全重试。
fn finish_harness_launch(
    id: HarnessLaunchClaimId,
    consumer: HarnessLaunchConsumer,
    success: bool,
) -> bool {
    let mut state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(owner) = state.owner.as_mut() else {
        return false;
    };
    if owner.generation != id.generation
        || owner.pid != Some(id.pid)
        || owner.in_flight != Some(consumer)
    {
        return false;
    }
    owner.in_flight = None;
    if success {
        match consumer {
            HarnessLaunchConsumer::Webview => owner.webview_pending = false,
            HarnessLaunchConsumer::Browser => owner.browser_pending = false,
        }
        if !owner.webview_pending && !owner.browser_pending {
            owner.url = None;
        }
    }
    true
}

pub fn finish_harness_launch_claim(claim: &HarnessLaunchClaim, success: bool) -> bool {
    finish_harness_launch(claim.id(), claim.consumer, success)
}

pub fn finish_harness_webview_claim(id: HarnessLaunchClaimId, success: bool) -> bool {
    finish_harness_launch(id, HarnessLaunchConsumer::Webview, success)
}

/// 捕获 dsh 公布的一次性浏览器认证 URL，并返回适合写日志的脱敏文本。
fn capture_and_redact_launch_url(line: &str, generation: u64, pid: u32) -> String {
    let Some(candidate) = line
        .strip_prefix("dsh web: ")
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return line.to_string();
    };
    let Ok(parsed) = reqwest::Url::parse(candidate) else {
        return line.to_string();
    };
    let token_pairs = parsed
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    if !token_pairs
        .iter()
        .any(|(name, value)| name == "token" && !value.is_empty())
    {
        return line.to_string();
    }
    if parsed.scheme() != "http" || parsed.host_str() != Some("127.0.0.1") {
        return line.to_string();
    }
    let mut state = harness_launch_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(owner) = state.owner.as_mut() {
        if owner.generation == generation && owner.pid == Some(pid) {
            owner.url = Some(candidate.to_string());
            owner.authenticated_port = parsed.port_or_known_default();
            owner.webview_pending = true;
            owner.browser_pending = true;
            owner.in_flight = None;
        }
    }
    drop(state);
    let mut safe_url = parsed;
    safe_url
        .query_pairs_mut()
        .clear()
        .extend_pairs(token_pairs.iter().map(|(name, value)| {
            (
                name.as_str(),
                if name == "token" {
                    "<redacted>"
                } else {
                    value.as_str()
                },
            )
        }));
    line.replacen(candidate, safe_url.as_str(), 1)
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

/// 旧版客户端插件 bundle 探测地址。
///
/// SPA `/` 在 webServer 绑定后立刻 200，此时连接桥与 Loader 图往往还没就绪；
/// WebView 若在这个窗口加载，会永久停在官方 boot 页 “Loading plugins…”。
/// 旧版没有可读取的启动图时，保留这两个稳定入口作为兼容兜底。
pub(super) fn health_probe_plugin_urls(port: u16) -> Vec<String> {
    vec![
        format!("http://127.0.0.1:{port}/plugins/@deepseek-ai/dsh-client-ui-layout/client.js"),
        format!("http://127.0.0.1:{port}/plugins/@deepseek-ai/dsh-client-runtime/client.js"),
    ]
}

/// 从新版 Harness 首页的 `__DSH_BOOT__` 启动图提取客户端 bundle 地址。
///
/// alpha 版不再保证桌面端旧适配器中的功能包名称存在，首页注入的启动图才是
/// WebView 实际会加载的唯一模块清单。这里不猜测替代包名，而是复用该清单中的
/// `entries`，同时加入 HTML 中预加载的 modules/runtime 两个入口。
pub(super) fn client_urls_from_boot_html(port: u16, html: &str) -> Option<Vec<String>> {
    let marker = "globalThis[\"__DSH_BOOT__\"] = ";
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find("</script>")? + start;
    let json = html[start..end].trim().trim_end_matches(';').trim();
    let boot: serde_json::Value = serde_json::from_str(json).ok()?;
    let entries = boot.get("entries")?.as_array()?;
    let mut paths = Vec::new();

    // 预加载脚本不一定重复出现在 entries 的 url 中（例如老的 boot 页面），因此
    // 两类地址都收集后去重；只接受同源相对的插件 client.js 路径。
    for script in html.split("<script").skip(1) {
        let Some(src_start) = script.find("src=\"") else {
            continue;
        };
        let rest = &script[src_start + 5..];
        let Some(src_end) = rest.find('\"') else {
            continue;
        };
        let src = decode_html_attribute(&rest[..src_end]);
        if is_client_bundle_path(&src) {
            paths.push(src);
        }
    }
    for entry in entries {
        let Some(url) = entry.get("url").and_then(|v| v.as_str()) else {
            continue;
        };
        let url = decode_html_attribute(url);
        if is_client_bundle_path(&url) {
            paths.push(url);
        }
    }
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return None;
    }
    Some(
        paths
            .into_iter()
            .map(|path| format!("http://127.0.0.1:{port}{path}"))
            .collect(),
    )
}

/// 还原 boot HTML 属性/JSON 中的有限命名实体。
///
/// alpha 核心的 combo URL 使用 `&rev=...`，HTML 注入后会变成 `&amp;rev=...`；
/// 解析前还原它，否则 reqwest 会把实体名当成真实查询参数的一部分。
fn decode_html_attribute(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn is_client_bundle_path(path: &str) -> bool {
    // alpha combo 路由合法地使用 `/plugins/??<package>/client.js&rev=...`：
    // 第一个 `?` 是路由约定，第二个是 combo payload 的起始标记，不能把它拼成
    // `/plugins/??` 再交给 URL 解析器时丢掉一个问号。
    path.starts_with("/plugins/") && path.contains("client.js") && !path.starts_with("//")
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
    generation: u64,
    pid: u32,
) where
    R1: Read + Send + 'static,
    R2: Read + Send + 'static,
{
    // 在独立线程中读取 stdout
    if let Some(stdout) = stdout {
        let log_path = log_path.clone();
        thread::spawn(move || {
            drain_subprocess_output(
                BufReader::new(stdout),
                log_path,
                log::Level::Info,
                generation,
                pid,
            );
        });
    }

    // 在独立线程中读取 stderr
    if let Some(stderr) = stderr {
        thread::spawn(move || {
            drain_subprocess_output(
                BufReader::new(stderr),
                log_path,
                log::Level::Warn,
                generation,
                pid,
            );
        });
    }
}

/// 逐行读取子进程输出并写入日志。
///
/// 任何一行是非法 UTF-8 时**必须**用 lossy 替换继续读下去，不能中断：管道
/// 读端一旦被关闭，dsh 主进程下一次写 stderr 就会收到 EPIPE，Node 以退出码 1
/// 静默崩溃（插件子进程——python MCP 服务器等——在中文本地化 Windows 下按
/// ANSI 代码页输出 GBK 日志是常态），桌面端表现为 Harness 反复崩溃、WebView
/// 永久卡在 "Loading plugins"。同样的非法字节在日志里以 U+FFFD 呈现，不影响
/// 其余行的可读性。真正需要停手的只有 EOF 与管道自身的 I/O 错误。
fn drain_subprocess_output<R: Read + Send + 'static>(
    mut reader: BufReader<R>,
    log_path: PathBuf,
    level: log::Level,
    generation: u64,
    pid: u32,
) {
    loop {
        let mut bytes = Vec::new();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => break, // EOF
            Ok(_) => {
                // 去除行尾 \n / \r\n（与旧 lines() 行为一致）
                let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
                let line = String::from_utf8_lossy(bytes);
                let safe_line = capture_and_redact_launch_url(&line, generation, pid);
                match level {
                    log::Level::Warn => log::warn!(target: "dsh", "{}", safe_line),
                    _ => log::info!(target: "dsh", "{}", safe_line),
                }
                append_log(&log_path, &safe_line);
            }
            Err(e) => {
                log::error!("Failed to read dsh {}: {}", log_level_name(level), e);
                break;
            }
        }
    }
}

/// `log::Level` 的小写名称（内部日志用词与旧实现一致）。
fn log_level_name(level: log::Level) -> &'static str {
    match level {
        log::Level::Warn => "stderr",
        _ => "stdout",
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

    static LAUNCH_URL_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn launch_url_test_guard() -> std::sync::MutexGuard<'static, ()> {
        LAUNCH_URL_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn write(path: &PathBuf, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// 回归：非法 UTF-8 行（中文 Windows 下 python 插件输出 GBK 的典型形态）
    /// 绝不能让读取器中断——旧实现 break 会关闭管道读端，dsh 下次写 stderr
    /// 收到 EPIPE 以退出码 1 崩溃，表现为 Harness 崩溃循环 + WebView 卡在
    /// "Loading plugins"。
    #[test]
    fn drain_keeps_reading_after_invalid_utf8_lines() {
        let dir = std::env::temp_dir().join(format!("dsh_drain_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("out.log");

        // "因为测试" 的 GBK 编码（与系统 ANSI 代码页 936 输出一致）
        let mut data = b"first line\n".to_vec();
        data.extend_from_slice(b"\xd2\xf2\xce\xaa\xb2\xe2\xca\xd4\n");
        data.extend_from_slice(b"second line\n");

        drain_subprocess_output(
            std::io::BufReader::new(std::io::Cursor::new(data)),
            log.clone(),
            log::Level::Warn,
            0,
            0,
        );

        let content = fs::read_to_string(&log).unwrap();
        assert!(
            content.contains("first line"),
            "first line must be logged, got: {content:?}"
        );
        assert!(
            content.contains("second line"),
            "reader must NOT stop at invalid UTF-8 (old code broke and closed the pipe); got: {content:?}"
        );
        // 非法字节以 U+FFFD 呈现，行内容不丢失
        assert!(content.contains('\u{FFFD}'));
        let _ = fs::remove_dir_all(&dir);
    }

    /// `log::Level` 与日志名称的映射。
    #[test]
    fn log_level_name_maps_streams() {
        assert_eq!(log_level_name(log::Level::Warn), "stderr");
        assert_eq!(log_level_name(log::Level::Info), "stdout");
        assert_eq!(log_level_name(log::Level::Debug), "stdout");
    }

    /// 行尾处理与旧 `lines()` 行为一致：\n 与 \r\n 均被剥离。
    #[test]
    fn drain_strips_line_endings() {
        let dir = std::env::temp_dir().join(format!("dsh_drain_crlf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("out.log");

        drain_subprocess_output(
            std::io::BufReader::new(std::io::Cursor::new(b"a\r\nb\n".to_vec())),
            log.clone(),
            log::Level::Info,
            0,
            0,
        );

        let content = fs::read_to_string(&log).unwrap();
        assert_eq!(content, "a\nb\n");
        let _ = fs::remove_dir_all(&dir);
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
    fn boot_html_uses_declared_client_graph() {
        let html = r#"<script src="/plugins/@deepseek-ai/dsh-client-modules/client.js?rev=one"></script><script>globalThis["__DSH_BOOT__"] = {"rev":"graph","entries":[{"id":"@deepseek-ai/dsh-client-ui-layout","url":"/plugins/@deepseek-ai/dsh-client-ui-layout/client.js?rev=two","rev":"two"}]}</script>"#;
        let urls = client_urls_from_boot_html(3099, html).expect("boot graph");
        assert_eq!(urls.len(), 2);
        assert!(urls
            .iter()
            .any(|url| url.contains("dsh-client-modules/client.js")));
        assert!(urls
            .iter()
            .any(|url| url.contains("dsh-client-ui-layout/client.js")));
        assert!(urls
            .iter()
            .all(|url| url.starts_with("http://127.0.0.1:3099/plugins/")));
    }

    #[test]
    fn boot_html_rejects_missing_or_external_graph_entries() {
        assert!(client_urls_from_boot_html(3099, "<html></html>").is_none());
        let html = r#"<script>globalThis[\"__DSH_BOOT__\"] = {\"entries\":[{\"url\":\"https://example.test/client.js\"}]}</script>"#;
        assert!(client_urls_from_boot_html(3099, html).is_none());
    }

    /// alpha combo bundle 的 script src 位于 HTML 属性时，`&rev=` 会编码为
    /// `&amp;rev=`；探测 URL 必须先还原实体，否则服务端收到错误资源地址并返回 404。
    #[test]
    fn boot_html_decodes_combo_bundle_attribute_url() {
        let html = r#"<script src="/plugins/??@deepseek-ai/dsh-client-modules/client.js&amp;rev=cddf5581d5d5"></script><script>globalThis["__DSH_BOOT__"] = {"entries":[]}</script>"#;
        let urls = client_urls_from_boot_html(3081, html).expect("boot graph");
        assert_eq!(urls, vec!["http://127.0.0.1:3081/plugins/??@deepseek-ai/dsh-client-modules/client.js&rev=cddf5581d5d5"]);
    }

    #[test]
    fn health_probe_plugin_urls_target_client_bundles_not_spa_root() {
        let urls = health_probe_plugin_urls(3080);
        assert!(urls.iter().all(|u| u.contains("/plugins/")));
        assert!(urls
            .iter()
            .all(|u| !u.ends_with("3080/") && !u.ends_with("://127.0.0.1:3080")));
        assert!(urls
            .iter()
            .any(|u| u.contains("dsh-client-ui-layout/client.js")));
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

    #[test]
    fn authenticated_launch_url_is_claimed_and_finalized_once_per_consumer() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        let generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(generation, 42));
        let safe = capture_and_redact_launch_url(
            "dsh web: http://127.0.0.1:3083/?token=secret-value",
            generation,
            42,
        );
        assert_eq!(safe, "dsh web: http://127.0.0.1:3083/?token=%3Credacted%3E");
        assert!(has_authenticated_harness_launch(3083));
        let webview = claim_harness_webview_launch(3083, generation)
            .unwrap()
            .expect("webview claim");
        assert_eq!(webview.url(), "http://127.0.0.1:3083/?token=secret-value");
        assert!(finish_harness_launch_claim(&webview, true));
        assert!(claim_harness_webview_launch(3083, generation)
            .unwrap()
            .is_none());
        assert!(harness_webview_handoff_completed(3083, generation));
        let browser = claim_harness_browser_launch(3083)
            .unwrap()
            .expect("browser claim");
        assert!(finish_harness_launch_claim(&browser, true));
        assert!(claim_harness_browser_launch(3083).unwrap().is_none());
        assert!(!has_authenticated_harness_launch(3080));
        clear_harness_launch_url();
    }

    #[test]
    fn wrong_port_or_generation_does_not_consume_launch_url() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        let generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(generation, 43));
        capture_and_redact_launch_url(
            "dsh web: http://127.0.0.1:3083/?token=secret",
            generation,
            43,
        );
        assert!(claim_harness_webview_launch(3084, generation).is_err());
        assert!(claim_harness_webview_launch(3083, generation.saturating_sub(1)).is_err());
        assert!(claim_harness_webview_launch(3083, generation)
            .unwrap()
            .is_some());
    }

    #[test]
    fn failed_delivery_releases_claim_for_retry() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        let generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(generation, 44));
        capture_and_redact_launch_url(
            "dsh web: http://127.0.0.1:3083/?token=retry",
            generation,
            44,
        );
        let first = claim_harness_webview_launch(3083, generation)
            .unwrap()
            .expect("first claim");
        assert!(finish_harness_launch_claim(&first, false));
        let retry = claim_harness_webview_launch(3083, generation)
            .unwrap()
            .expect("retry claim");
        assert_eq!(retry.url(), first.url());
        assert!(finish_harness_launch_claim(&retry, true));
    }

    #[test]
    fn stale_pid_claim_cannot_finalize_replacement_owner() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        let old_generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(old_generation, 45));
        capture_and_redact_launch_url(
            "dsh web: http://127.0.0.1:3083/?token=old",
            old_generation,
            45,
        );
        let stale = claim_harness_webview_launch(3083, old_generation)
            .unwrap()
            .expect("stale claim");
        let replacement_generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(replacement_generation, 46));
        assert!(!finish_harness_launch_claim(&stale, true));
        assert_eq!(
            current_harness_launch_generation(),
            Some(replacement_generation)
        );
    }

    #[test]
    fn launch_generation_is_owned_by_the_rust_process() {
        let first = begin_harness_launch_generation();
        let second = begin_harness_launch_generation();
        assert!(second > first);
    }

    #[test]
    fn redacts_encoded_and_repeated_tokens() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        let safe = capture_and_redact_launch_url(
            "dsh web: http://127.0.0.1:3083/?token=secret%2Fvalue&keep=a%2Fb&token=second",
            0,
            0,
        );
        assert!(!safe.contains("secret"));
        assert!(!safe.contains("second"));
        assert!(safe.contains("keep=a%2Fb"));
        assert_eq!(safe.matches("token=%3Credacted%3E").count(), 2);
    }

    #[test]
    fn non_loopback_or_tokenless_urls_are_not_captured() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        for line in [
            "dsh web: http://example.com:3083/?token=secret",
            "dsh web: https://127.0.0.1:443/?token=secret",
            "dsh web: http://127.0.0.1:3083/",
        ] {
            assert_eq!(capture_and_redact_launch_url(line, 0, 0), line);
        }
        assert!(!has_authenticated_harness_launch(443));
    }

    #[test]
    fn old_reader_cannot_refill_replacement_owner() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        let old_generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(old_generation, 51));
        let new_generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(new_generation, 52));
        capture_and_redact_launch_url(
            "dsh web: http://127.0.0.1:3083/?token=old",
            old_generation,
            51,
        );
        assert!(!has_authenticated_harness_launch(3083));
    }

    #[test]
    fn exit_cleanup_only_matches_current_pid() {
        let _guard = launch_url_test_guard();
        clear_harness_launch_url();
        let generation = begin_harness_launch_generation();
        assert!(bind_harness_launch_process(generation, 62));
        assert!(!clear_harness_launch_url_if_pid(61));
        assert_eq!(current_harness_launch_generation(), Some(generation));
        assert!(clear_harness_launch_url_if_pid(62));
        assert_eq!(current_harness_launch_generation(), None);
    }
}
