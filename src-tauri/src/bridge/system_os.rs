//! 浏览器/文件管理器唤起、日志捕获与健康检测。
//!
//! 对外部系统组件的交互：在系统浏览器打开链接、在文件管理器定位/打开目录与
//! 数据目录、复制服务地址到剪贴板；前端与后端日志的透传/读取/清空；以及通过
//! Rust 代理的服务健康检查与运行时环境诊断信息。

use crate::config;
use crate::logger;
use crate::service::core;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

/// 健康检查（通过 Rust 代理，避免 WebView CORS 问题）
#[tauri::command]
pub async fn proxy_health_check(app_handle: AppHandle) -> Result<String, String> {
    let port = config::get_store_dat_setting(&app_handle).port;
    crate::service::workflow::proxy_health_check(port).await
}

/// 运行时/版本/诊断信息（侧边栏展示）
#[tauri::command]
pub async fn get_runtime_info(app_handle: AppHandle) -> Result<config::RuntimeInfo, String> {
    let port = config::get_store_dat_setting(&app_handle).port;
    let mut info = config::runtime_info(&app_handle, port);
    info.dsh_version = core::active_version(&app_handle).or(info.dsh_version);
    info.core_source = core::active_source(&app_handle).as_str().to_string();
    info.core_path = Some(
        core::active_dsh_binary(&app_handle)
            .to_string_lossy()
            .into_owned(),
    );
    Ok(info)
}

/// 为 WebView 创建原生认证导航，token 不跨 IPC。
#[derive(Serialize)]
pub struct HarnessWebviewPreparation {
    url: String,
    verify_auth: bool,
    claim: Option<crate::service::workflow::utils::HarnessLaunchClaimId>,
    probe_origin: Option<String>,
}

#[derive(Clone, Serialize)]
struct HarnessAuthProbePayload {
    origin: String,
    status: u16,
}

#[tauri::command]
pub async fn prepare_harness_webview(
    app_handle: AppHandle,
    generation: u64,
) -> Result<HarnessWebviewPreparation, String> {
    let port = config::get_store_dat_setting(&app_handle).port;
    let service_url = config::get_dsh_service_url(port);
    let Some(claim) =
        crate::service::workflow::utils::claim_harness_webview_launch(port, generation)?
    else {
        let reuse_authenticated_webview =
            crate::service::workflow::utils::harness_webview_handoff_completed(port, generation);
        return Ok(HarnessWebviewPreparation {
            url: service_url,
            verify_auth: reuse_authenticated_webview,
            claim: None,
            probe_origin: reuse_authenticated_webview.then(|| config::get_dsh_service_url(port)),
        });
    };
    match prepare_claimed_harness_webview(&claim).await {
        Ok(preparation) => Ok(preparation),
        Err(error) => {
            crate::service::workflow::utils::finish_harness_launch_claim(&claim, false);
            Err(error)
        }
    }
}

async fn prepare_claimed_harness_webview(
    claim: &crate::service::workflow::utils::HarnessLaunchClaim,
) -> Result<HarnessWebviewPreparation, String> {
    let (launch_url, iframe_url) = prepare_harness_launch_urls(claim.url())?;
    let probe_origin = reqwest::Url::parse(&iframe_url)
        .map_err(|_| "HARNESS_AUTH_URL_INVALID: clean iframe URL is invalid".to_string())?
        .origin()
        .ascii_serialization();
    let relay_url = start_harness_auth_relay(launch_url, claim.id()).await?;
    Ok(HarnessWebviewPreparation {
        url: relay_url,
        verify_auth: true,
        claim: Some(claim.id()),
        probe_origin: Some(probe_origin),
    })
}

/// 校验并保留受信的字面 127.0.0.1 启动地址，避免 localhost 落到竞争的 IPv6 监听器。
fn prepare_harness_launch_urls(launch_url: &str) -> Result<(String, String), String> {
    let mut exchange_url = reqwest::Url::parse(launch_url)
        .map_err(|error| format!("HARNESS_AUTH_URL_INVALID: {error}"))?;
    let has_token = exchange_url
        .query_pairs()
        .any(|(name, value)| name == "token" && !value.is_empty());
    if exchange_url.scheme() != "http"
        || exchange_url.host_str() != Some("127.0.0.1")
        || !exchange_url.username().is_empty()
        || exchange_url.password().is_some()
        || !has_token
    {
        return Err("HARNESS_AUTH_URL_REJECTED: expected 127.0.0.1 loopback URL".into());
    }
    let mut clean_url = exchange_url.clone();
    clean_url.set_query(None);
    clean_url.set_fragment(None);
    Ok((exchange_url.to_string(), clean_url.to_string()))
}

/// 让真实 WebView frame 自己跟随官方 token 交换，relay URL 本身不含秘密。
async fn start_harness_auth_relay(
    launch_url: String,
    claim: crate::service::workflow::utils::HarnessLaunchClaimId,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|_| "HARNESS_AUTH_RELAY_BIND_FAILED: loopback bind failed".to_string())?;
    let port = listener
        .local_addr()
        .map_err(|_| "HARNESS_AUTH_RELAY_ADDRESS_FAILED: loopback address unavailable".to_string())?
        .port();
    let path = format!("/dsh-auth/{}/{}", claim.generation, claim.pid);
    let expected_path = path.clone();
    tauri::async_runtime::spawn(async move {
        let accepted =
            tokio::time::timeout(std::time::Duration::from_secs(20), listener.accept()).await;
        let Ok(Ok((mut stream, _))) = accepted else {
            return;
        };
        let mut request = [0_u8; 2048];
        let Ok(size) = stream.read(&mut request).await else {
            return;
        };
        let request = String::from_utf8_lossy(&request[..size]);
        let request_path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|target| target.split('?').next());
        let response = if request_path == Some(expected_path.as_str()) {
            format!(
                "HTTP/1.1 303 See Other\r\nLocation: {launch_url}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
        } else {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        };
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    });
    Ok(format!("http://127.0.0.1:{port}{path}"))
}

#[tauri::command]
pub async fn mount_harness_webview(
    app_handle: AppHandle,
    url: String,
    probe_origin: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    if ![x, y, width, height].iter().all(|value| value.is_finite()) {
        return Err("HARNESS_WEBVIEW_BOUNDS_INVALID: child WebView bounds are invalid".into());
    }
    let relay_url = reqwest::Url::parse(&url)
        .map_err(|_| "HARNESS_WEBVIEW_URL_INVALID: relay URL is invalid".to_string())?;
    let expected_origin = reqwest::Url::parse(&probe_origin)
        .map_err(|_| "HARNESS_WEBVIEW_ORIGIN_INVALID: probe origin is invalid".to_string())?;
    if expected_origin.scheme() != "http"
        || expected_origin.host_str() != Some("127.0.0.1")
        || expected_origin.origin().ascii_serialization() != probe_origin
    {
        return Err("HARNESS_WEBVIEW_ORIGIN_REJECTED: expected 127.0.0.1 origin".into());
    }
    let is_relay = relay_url.path().starts_with("/dsh-auth/");
    let is_clean_core = relay_url.origin() == expected_origin.origin()
        && relay_url.path() == "/"
        && relay_url.query_pairs().all(|(name, _)| name == "t");
    if relay_url.scheme() != "http"
        || relay_url.host_str() != Some("127.0.0.1")
        || (!is_relay && !is_clean_core)
    {
        return Err("HARNESS_WEBVIEW_URL_REJECTED: expected native relay or clean core URL".into());
    }
    let relay_port = relay_url.port();
    let harness_port = expected_origin.port();

    if let Some(existing) = app_handle.get_webview("harness") {
        existing
            .close()
            .map_err(|_| "HARNESS_WEBVIEW_CLOSE_FAILED: child WebView close failed".to_string())?;
    }
    let window = app_handle
        .get_window("main")
        .ok_or_else(|| "HARNESS_WEBVIEW_WINDOW_MISSING: main window not found".to_string())?;
    let event_app = app_handle.clone();
    let event_origin = probe_origin.clone();
    let destination = url
        .parse()
        .map_err(|_| "HARNESS_WEBVIEW_URL_INVALID: relay URL is invalid".to_string())?;
    let builder = tauri::webview::WebviewBuilder::new(
        "harness",
        WebviewUrl::External(
            "about:blank"
                .parse()
                .expect("about:blank must remain a valid URL"),
        ),
    )
    .initialization_script(crate::desktop::auth_probe::AUTH_TOP_LEVEL_PROBE_JS)
    .on_navigation(move |target| {
        if target.scheme() == "dsh-auth-probe" {
            if target.host_str() != Some("127.0.0.1") {
                return false;
            }
            let status = target
                .query_pairs()
                .find_map(|(name, value)| (name == "status").then(|| value.parse::<u16>().ok()))
                .flatten()
                .unwrap_or(0);
            let _ = event_app.emit(
                "harness-auth-probe",
                HarnessAuthProbePayload {
                    origin: event_origin.clone(),
                    status,
                },
            );
            return false;
        }
        target.as_str() == "about:blank"
            || target.scheme() == "http"
                && target.host_str() == Some("127.0.0.1")
                && (target.port() == relay_port || target.port() == harness_port)
    });
    let webview = window
        .add_child(
            builder,
            tauri::LogicalPosition::new(-1.0, -1.0),
            tauri::LogicalSize::new(1.0, 1.0),
        )
        .map_err(|_| "HARNESS_WEBVIEW_CREATE_FAILED: child WebView creation failed".to_string())?;
    webview
        .navigate(destination)
        .map_err(|_| "HARNESS_WEBVIEW_NAVIGATE_FAILED: child WebView navigation failed".to_string())
}

#[tauri::command]
pub fn set_harness_webview_bounds(
    app_handle: AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let Some(webview) = app_handle.get_webview("harness") else {
        return Ok(());
    };
    webview
        .set_bounds(tauri::Rect {
            position: tauri::Position::Logical(tauri::LogicalPosition::new(x, y)),
            size: tauri::Size::Logical(tauri::LogicalSize::new(width.max(1.0), height.max(1.0))),
        })
        .map_err(|_| "HARNESS_WEBVIEW_BOUNDS_FAILED: child WebView resize failed".to_string())
}

#[tauri::command]
pub fn close_harness_webview(app_handle: AppHandle) -> Result<(), String> {
    if let Some(webview) = app_handle.get_webview("harness") {
        webview
            .close()
            .map_err(|_| "HARNESS_WEBVIEW_CLOSE_FAILED: child WebView close failed".to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn reload_harness_webview(app_handle: AppHandle) -> Result<(), String> {
    app_handle
        .get_webview("harness")
        .ok_or_else(|| "HARNESS_WEBVIEW_MISSING: child WebView not found".to_string())?
        .reload()
        .map_err(|_| "HARNESS_WEBVIEW_RELOAD_FAILED: child WebView reload failed".to_string())
}

#[tauri::command]
pub fn finish_harness_webview_auth(generation: u64, pid: u32, success: bool) -> Result<(), String> {
    let id = crate::service::workflow::utils::HarnessLaunchClaimId { generation, pid };
    crate::service::workflow::utils::finish_harness_webview_claim(id, success)
        .then_some(())
        .ok_or_else(|| "HARNESS_AUTH_OWNER_CHANGED: WebView claim no longer owns launch".into())
}

/// 在系统浏览器中打开 Harness 界面
#[tauri::command]
pub async fn open_in_browser(app_handle: AppHandle) -> Result<(), String> {
    let port = config::get_store_dat_setting(&app_handle).port;
    let claim = crate::service::workflow::utils::claim_harness_browser_launch(port)?;
    let url = claim
        .as_ref()
        .map(|claim| claim.url().to_string())
        .unwrap_or_else(|| config::get_dsh_service_url(port));
    let result = app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "HARNESS_BROWSER_OPEN_FAILED: system browser launch failed".to_string());
    if let Some(claim) = claim {
        if !crate::service::workflow::utils::finish_harness_launch_claim(&claim, result.is_ok()) {
            return Err(
                "HARNESS_AUTH_OWNER_CHANGED: launch owner changed during browser open".into(),
            );
        }
    }
    result
}

/// 复制 Harness 服务地址到剪贴板
#[tauri::command]
pub async fn copy_service_url(app_handle: AppHandle) -> Result<(), String> {
    let url = config::get_dsh_service_url(config::get_store_dat_setting(&app_handle).port);
    app_handle
        .clipboard()
        .write_text(url)
        .map_err(|e| e.to_string())
}

/// 在系统文件管理器中定位指定文件（Session 日志下载完成后的"在文件夹中显示"）
#[tauri::command]
pub fn reveal_in_folder(app_handle: AppHandle, path: String) -> Result<(), String> {
    // 安全边界：只允许定位允许根目录（下载目录/数据目录/$DSH_HOME）内的文件，
    // 防止第三方插件通过 IPC 驱动宿主打开任意路径。
    if !crate::bridge::guard::is_allowed_path(&app_handle, std::path::Path::new(&path)) {
        return Err(format!("REVEAL_PATH_REJECTED: {path}"));
    }
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| format!("REVEAL_FAILED: {e}"))
}

/// 在系统文件管理器中打开指定目录（核心版本「打开目录」按钮；目录用 open 而非
/// reveal——reveal 是定位父目录，open 是直接打开该目录本身）。
#[tauri::command]
pub fn open_dir(app_handle: AppHandle, path: String) -> Result<(), String> {
    // 安全边界同 reveal_in_folder：仅允许打开允许根目录内的目录
    if !crate::bridge::guard::is_allowed_path(&app_handle, std::path::Path::new(&path)) {
        return Err(format!("OPEN_DIR_REJECTED: {path}"));
    }
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| format!("OPEN_DIR_FAILED: {e}"))
}

/// 在系统文件管理器中打开数据目录（官方 $DSH_HOME，即 ~/.dsh）
#[tauri::command]
pub async fn reveal_data_dir(app_handle: AppHandle) -> Result<(), String> {
    let dsh_home = config::get_dsh_data_path(&app_handle);
    // 目录可能尚未创建（全新安装），先建好再打开，避免资源管理器报路径不存在
    std::fs::create_dir_all(&dsh_home).map_err(|e| e.to_string())?;

    if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(&dsh_home)
            .spawn()
            .map_err(|e| e.to_string())?;
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(&dsh_home)
            .spawn()
            .map_err(|e| e.to_string())?;
    } else {
        std::process::Command::new("xdg-open")
            .arg(&dsh_home)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 前端日志透传：前端 `console.*` 劫持经此命令落盘到 `desktop.frontdesk.log`
/// （与持有后端 + `dsh` target 的 `desktop.log` 分离），见 `logger/mod.rs`。
#[tauri::command]
pub fn log_frontend(level: String, target: String, message: String) {
    let lvl = logger::FrontendLevel::from_str(&level);
    logger::log_frontend(lvl, &target, &message);
}

/// 按字节上限取 `s` 的尾部，并在裁剪起点回退到 UTF-8 字符边界。
///
/// 日志必然包含中文/ANSI 等多字节字符，直接用
/// `&s[s.len() - max_bytes..]` 在起点落在字符中间时会 panic
/// （`byte index ... is not a char boundary`），此实现保证安全。
fn tail_bytes(s: &str, max_bytes: usize) -> &str {
    let start = s.len().saturating_sub(max_bytes);
    let mut i = start;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

/// 读取 dsh 服务日志
#[tauri::command]
pub async fn read_service_logs(
    app_handle: AppHandle,
    max_bytes: Option<usize>,
) -> Result<String, String> {
    let log_path = config::get_service_log_path(&app_handle);
    if !log_path.exists() {
        return Ok(String::new());
    }

    let content = std::fs::read_to_string(&log_path).map_err(|e| e.to_string())?;
    let max_bytes = max_bytes.unwrap_or(64 * 1024);
    if content.len() <= max_bytes {
        Ok(content)
    } else {
        Ok(tail_bytes(&content, max_bytes).to_string())
    }
}

/// 清空 dsh 服务日志
#[tauri::command]
pub async fn clear_service_logs(app_handle: AppHandle) -> Result<(), String> {
    let log_path = config::get_service_log_path(&app_handle);
    std::fs::write(&log_path, "").map_err(|e| e.to_string())
}

/// 读取运行日志（DSH 服务日志 + 桌面端 Rust 运行日志），格式化为便于
/// 反馈/报障复制的纯文本块：`### 环境信息`、`### 服务日志`、`### 前台日志`
/// 与 `### 后台日志` 四段。
///
/// 服务日志来自 `logs/dsh-web.log`（debug 构建为 `logs/dsh-web.dev.log`）；
/// 运行日志来自 `logs/desktop.log`（桌面端自身 `logger::init` 每次启动落盘，
/// 见 logger/mod.rs）。前端 `console.*` 已在 logger 的文件层按 `target: "frontend"`
/// 跳过、不会写入 `desktop.log`（见 logger/mod.rs），因此「后台日志」取的是纯后端
/// `log::*`；仅对旧版本已落盘、尚未轮转掉的 `frontend:` 行做一次兜底剔除。据此把
/// 「运行日志」拆成：
/// - `### 前台日志`：取自前端独立文件 `logs/desktop.frontdesk.log`
///   （`logger::init` 单独落盘，见 logger/mod.rs），仅含前端 `console.*`；
/// - `### 后台日志`：取自 `logs/desktop.log`，剔除残余 `target: "frontend"` 行，
///   仅保留后端 `log::*`。
/// 每段取末尾最多 `MAX_LINES` 行（前端日志量大，仅取一半 `FRONTEND_MAX_LINES`），
/// 避免粘贴内容超出 GitHub issue 长度上限。
#[tauri::command]
pub async fn read_run_logs(app_handle: AppHandle) -> Result<String, String> {
    const MAX_LINES: usize = 100;
    // 前端日志量大，复制的行数减半（避免粘贴内容过长）；后端仍取满 MAX_LINES
    const FRONTEND_MAX_LINES: usize = MAX_LINES / 2;

    let base = config::get_base_dir(&app_handle);
    let service = config::get_service_log_path(&app_handle);
    let desktop = base.join("logs").join("desktop.log");
    let frontend = base.join("logs").join("desktop.frontdesk.log");

    let read_tail = |path: &std::path::Path, max_lines: usize| -> String {
        if !path.exists() {
            return String::new();
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    };

    // 后端尾行：先剔除 `target: "frontend"` 的行，再取末尾，避免前端日志把后端日志挤没
    let read_backend_tail = |path: &std::path::Path, max_lines: usize| -> String {
        if !path.exists() {
            return String::new();
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<&str> = content
            .lines()
            .filter(|line| !is_frontend_log_line(line))
            .collect();
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    };

    // 环境信息：桌面端应用版本、dsh 发行版本、Node 版本与系统平台/架构，便于报障时快速定位环境差异
    let dsh_version = core::active_version(&app_handle)
        .or_else(|| config::get_dsh_version(&app_handle))
        .map(|v| format!("dsh: {v}\n"))
        .unwrap_or_default();
    let env_text = format!(
        "app: {}\n{}node: {}\nos: {} ({})",
        app_handle.package_info().version,
        dsh_version,
        config::get_active_node_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );

    let service_text = read_tail(&service, MAX_LINES);
    let frontend_text = read_tail(&frontend, FRONTEND_MAX_LINES);
    let backend_text = read_backend_tail(&desktop, MAX_LINES);

    Ok(format!(
        "### 环境信息\n\n{}\n\n### 服务日志\n\n```\n{}\n```\n\n### 前台日志\n\n```\n{}\n```\n\n### 后台日志\n\n```\n{}\n```",
        env_text,
        service_text.trim_end(),
        frontend_text.trim_end(),
        backend_text.trim_end()
    ))
}

/// 判断某行是否为前端日志（`target: "frontend"`）。
/// 日志行格式见 logger/mod.rs：`[ts] LEVEL target: message`（时间戳可能含空格）。
/// 前端行的 target 恒为 `frontend`，紧跟 LEVEL 之后；用「LEVEL + frontend:」定位，
/// 避免把消息正文里出现的 "frontend" 误判为前端日志。
fn is_frontend_log_line(line: &str) -> bool {
    const LEVELS: [&str; 5] = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
    let trimmed = line.trim_start();
    LEVELS
        .iter()
        .any(|lvl| trimmed.contains(&format!("{lvl} frontend:")))
}

/// 在系统浏览器中打开任意 http(s) 链接（更新说明 / 关于对话框仓库链接等）
#[tauri::command]
pub async fn open_external_url(app_handle: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("EXTERNAL_URL_INVALID: {url}"));
    }
    app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::is_frontend_log_line;
    use super::prepare_harness_launch_urls;
    use super::start_harness_auth_relay;
    use super::tail_bytes;
    use crate::service::workflow::utils::HarnessLaunchClaimId;

    #[test]
    fn frontend_line_detected() {
        // tracing 文件层（desktop.log）与前端独立文件（desktop.frontdesk.log）两种时间戳格式都应命中
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z INFO frontend: [tag] message"
        ));
        assert!(is_frontend_log_line(
            "[2024-06-01 12:00:00.123Z] INFO frontend: message"
        ));
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z WARN frontend: something"
        ));
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z ERROR frontend: boom"
        ));
    }

    #[test]
    fn backend_line_not_detected() {
        // 后端（dsh 等 target）不应误判为前端；消息正文里出现 "frontend" 也不应命中
        assert!(!is_frontend_log_line(
            "2024-06-01 12:00:00.123Z INFO dsh: starting server"
        ));
        assert!(!is_frontend_log_line(
            "[2024-06-01 12:00:00.123Z] INFO dsh: emit to frontend: 3"
        ));
        assert!(!is_frontend_log_line(
            "2024-06-01 12:00:00.123Z DEBUG reqwest: GET /ping"
        ));
    }

    #[test]
    fn frontend_level_padding_and_extra_spaces() {
        // 级别可能带前导空格（`{:>5}` 或 tracing 层多空格），frontend 目标仍应命中
        assert!(is_frontend_log_line(
            "2024-06-01 12:00:00.123Z  INFO frontend: padded"
        ));
    }

    #[tokio::test]
    async fn auth_relay_keeps_token_out_of_the_frontend_url() {
        let (launch_url, iframe_url) =
            prepare_harness_launch_urls("http://127.0.0.1:31415/?token=one-shot")
                .expect("prepare launch URLs");
        assert_eq!(launch_url, "http://127.0.0.1:31415/?token=one-shot");
        assert_eq!(iframe_url, "http://127.0.0.1:31415/");

        let relay = start_harness_auth_relay(
            launch_url.clone(),
            HarnessLaunchClaimId {
                generation: 9,
                pid: 42,
            },
        )
        .await
        .expect("start relay");
        let relay_response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("relay client")
            .get(format!("{relay}?t=123"))
            .send()
            .await
            .expect("relay response");
        assert_eq!(relay_response.status(), reqwest::StatusCode::SEE_OTHER);
        assert_eq!(
            relay_response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(launch_url.as_str())
        );
        assert!(!relay.contains("one-shot"));
    }

    #[test]
    fn invalid_launch_url_error_never_contains_token_or_url() {
        let launch_url = "https://example.invalid/?token=SENTINEL_SECRET_TOKEN";
        let error = prepare_harness_launch_urls(launch_url).expect_err("remote URL must fail");
        assert!(!error.contains("SENTINEL_SECRET_TOKEN"));
        assert!(!error.contains(launch_url));
        assert_eq!(
            error,
            "HARNESS_AUTH_URL_REJECTED: expected 127.0.0.1 loopback URL"
        );
    }

    #[test]
    fn tail_bytes_keeps_ascii_within_limit() {
        assert_eq!(tail_bytes("hello world", 5), "world");
        // 起点已落在字符边界时原样截取
        assert_eq!(tail_bytes("abc", 2), "bc");
    }

    #[test]
    fn tail_bytes_advances_to_char_boundary() {
        // 截取起点落在 3 字节中文中间 → 回退到字符边界，不 panic 且结果 ≤ max_bytes
        assert_eq!(tail_bytes("中a", 2), "a");
        // 4 字节 emoji 同理（非边界前缀字节会连续回退）
        assert_eq!(tail_bytes("😀x", 3), "x");
        // 多字节 + 超限，回退后长度仍不超过 max_bytes
        assert_eq!(tail_bytes("中文abc", 3), "abc");
    }

    #[test]
    fn tail_bytes_shorter_than_limit_returns_whole() {
        assert_eq!(tail_bytes("中文", 10), "中文");
        assert_eq!(tail_bytes("", 10), "");
    }
}
