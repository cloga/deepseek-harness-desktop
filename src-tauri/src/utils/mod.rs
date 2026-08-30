use std::path::PathBuf;

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

use crate::service::core::active_core_package_dir;

/// 对某个 dsh 包文件的一次性幂等补丁判定结果。
#[derive(Debug, PartialEq, Eq)]
pub enum PatchOutcome {
    /// 目标已含补丁标记（本补丁已生效，或上游官方已合并），无需再改。
    AlreadyPatched,
    /// 锚点缺失（上游布局变更），跳过并向调用方说明降级兜底。
    AnchorMissing,
    /// 已生成补丁后的完整内容。
    Patched(String),
}

/// 从统一的包内相对路径解析活动核心中的真实文件。
///
/// 本地核心可能采用 npm 扁平布局，兄弟包不在 dsh 自身的 `node_modules` 下，因此
/// 先拆出包名交给核心解析器，再拼接包内路径。显式本地核心失效时保持错误，不回退
/// 到预打包核心。
fn active_core_target(
    app_handle: &tauri::AppHandle,
    rel_path: &str,
) -> Result<Option<PathBuf>, String> {
    const PREFIX: &str = "node_modules/@deepseek-ai/";
    let rest = rel_path
        .strip_prefix(PREFIX)
        .ok_or_else(|| format!("DSH_PATCH_PATH_INVALID: unsupported relative path {rel_path}"))?;
    let (package, package_path) = rest
        .split_once('/')
        .ok_or_else(|| format!("DSH_PATCH_PATH_INVALID: missing package path in {rel_path}"))?;
    match active_core_package_dir(app_handle, package) {
        Ok(dir) => Ok(Some(dir.join(package_path))),
        Err(error) if error.starts_with("CORE_PACKAGE_NOT_FOUND:") => {
            log::info!("dsh patch package not found, skip: {error}");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// 写补丁前保留 `.dsh-backup`，再用同目录临时文件替换；失败时回滚原文件。
fn write_patch_with_backup(path: &std::path::Path, content: &str) -> Result<(), String> {
    let backup = path.with_extension("dsh-backup");
    std::fs::copy(path, &backup).map_err(|error| {
        format!(
            "DSH_PATCH_BACKUP: {} -> {} failed: {error}",
            path.display(),
            backup.display()
        )
    })?;

    let temp = path.with_extension("dsh-patch-tmp");
    if let Err(error) = std::fs::write(&temp, content) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("DSH_PATCH_WRITE: {} failed: {error}", temp.display()));
    }

    let replace = match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(path);
            std::fs::rename(&temp, path)
        }
    };
    if let Err(error) = replace {
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::copy(&backup, path);
        return Err(format!(
            "DSH_PATCH_RENAME: replace {} failed: {error}",
            path.display()
        ));
    }
    Ok(())
}

/// 对活动核心安装目录下的某个 dsh 包文件应用一次性幂等补丁。
///
/// - `rel_path`：相对活动核心安装目录的包内路径，例如
///   `node_modules/@deepseek-ai/dsh-client-ui-renderer/lib/client.js`（即 `patch_dsh("packagename/xxx/xxx.js", ..)` 里的包路径）。
/// - `patch`：纯函数式补丁判定，输入文件原文、返回 [`PatchOutcome`]；只做内容变换，
///   不触碰文件系统，便于单测。
///
/// 统一处理「定位文件 → 读取 → 打补丁 → 写回」与对应的日志。文件缺失、已打过、
/// 锚点变更均静默跳过并返回 Ok；只有真实读/写失败才返回 Err（不阻断启动的调用方
/// 据此仅告警）。目标解析始终限定在当前活动核心的包树内。
pub fn patch_dsh(
    app_handle: &tauri::AppHandle,
    rel_path: &str,
    patch: impl FnOnce(&str) -> PatchOutcome,
) -> Result<(), String> {
    let Some(target) = active_core_target(app_handle, rel_path)? else {
        return Ok(());
    };
    if !target.exists() {
        log::info!("dsh patch target not found, skip: {}", target.display());
        return Ok(());
    }
    let source = std::fs::read_to_string(&target)
        .map_err(|e| format!("DSH_PATCH_READ: {} failed: {e}", target.display()))?;
    match patch(&source) {
        PatchOutcome::AlreadyPatched => {
            log::info!("dsh patch already applied: {}", target.display());
        }
        PatchOutcome::AnchorMissing => {
            log::warn!("dsh patch anchor missing, skip: {}", target.display());
        }
        PatchOutcome::Patched(patched) => {
            write_patch_with_backup(&target, &patched)?;
            log::info!("dsh patch applied: {}", target.display());
        }
    }
    Ok(())
}

pub fn show_window<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// 显示主窗口：托盘「打开面板」、托盘左键点击、macOS Dock 图标点击共用。
/// 关闭按钮只隐藏窗口（见 builder 的 on_window_event），所以这里取到即可 show；
/// 若窗口确实不存在（非预期路径），仅记录日志，不重建。
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        show_window(&window);
    } else {
        log::warn!("[window] main window not found, skip show");
    }
}

pub fn app_icon_temp_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let icon = app.default_window_icon()?;
    let path = std::env::temp_dir().join(format!("dsh-notification-{}.png", std::process::id()));
    let rgba = icon.rgba().to_vec();
    let img = image::RgbaImage::from_raw(icon.width(), icon.height(), rgba)?;
    img.save(&path).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::write_patch_with_backup;

    #[test]
    fn patch_write_preserves_original_backup() {
        let root =
            std::env::temp_dir().join(format!("dsh-patch-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("client.js");
        std::fs::write(&path, "before").unwrap();

        write_patch_with_backup(&path, "after").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("dsh-backup")).unwrap(),
            "before"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
