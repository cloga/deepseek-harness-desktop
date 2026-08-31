//! 备份 / 还原 Tauri 命令。
//!
//! 薄封装：把 IPC 参数转成 `service::backup` 的调用，遵循 `bridge/profile.rs`
//! → `service::profile` 的分层模式。

use tauri::AppHandle;

use crate::service::backup;

/// 创建备份（`$DSH_HOME` → `$DSH_HOME/.backups/<timestamp>.tar.gz`）。
#[tauri::command]
pub fn backup_profile(
    app_handle: AppHandle,
    include_credentials: bool,
) -> Result<backup::BackupInfo, String> {
    backup::create_backup(
        &app_handle,
        backup::BackupOptions { include_credentials },
    )
}

/// 从指定备份还原。
///
/// `as_new` = true 时创建新档案目录；false 时覆盖当前 `$DSH_HOME`。
#[tauri::command]
pub fn restore_profile(
    app_handle: AppHandle,
    timestamp: String,
    as_new: bool,
) -> Result<(), String> {
    let mode = if as_new {
        backup::RestoreMode::AsNew
    } else {
        backup::RestoreMode::Overwrite
    };
    backup::restore_backup(&app_handle, &timestamp, mode)
}

/// 列出所有备份。
#[tauri::command]
pub fn list_backups(app_handle: AppHandle) -> Vec<backup::BackupInfo> {
    backup::list_backups(&app_handle)
}

/// 删除指定备份。
#[tauri::command]
pub fn delete_backup(app_handle: AppHandle, timestamp: String) -> Result<(), String> {
    backup::delete_backup(&app_handle, &timestamp)
}
