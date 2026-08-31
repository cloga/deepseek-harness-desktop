//! 历史鉴权绕过补丁的升级检测与安全处置。

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use super::{active_source, local_core_package_dir, CoreSource};
use crate::config;

const HISTORICAL_AUTH_BYPASS_MARKER: &str = "dsh-tauri-desktop: alpha embedded auth bypass";
const INSTALLED_CONNECTION_ENTRY: &str =
    "node_modules/@deepseek-ai/dsh-client-connection/lib/index.js";
const SOURCE_CONNECTION_ENTRY: &str = "packages/client/connection/lib/index.js";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegrityDisposition {
    Clean,
    ReplaceManaged,
    RejectLocal,
}

fn contains_historical_bypass(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    std::fs::read_to_string(path)
        .map(|source| source.contains(HISTORICAL_AUTH_BYPASS_MARKER))
        .unwrap_or(true)
}

fn local_connection_entries(app_handle: &AppHandle) -> Vec<PathBuf> {
    let Some(package_dir) = local_core_package_dir(app_handle) else {
        return Vec::new();
    };
    let mut entries = vec![package_dir.join(INSTALLED_CONNECTION_ENTRY)];
    if let Some(source_entry) = package_dir
        .ancestors()
        .map(|ancestor| ancestor.join(SOURCE_CONNECTION_ENTRY))
        .find(|candidate| candidate.is_file())
    {
        entries.push(source_entry);
    }
    entries
}

fn integrity_disposition(source: CoreSource, entries: &[PathBuf]) -> IntegrityDisposition {
    if !entries
        .iter()
        .any(|entry| contains_historical_bypass(entry))
    {
        return IntegrityDisposition::Clean;
    }
    match source {
        CoreSource::App => IntegrityDisposition::ReplaceManaged,
        CoreSource::Local => IntegrityDisposition::RejectLocal,
    }
}

fn active_integrity_disposition(app_handle: &AppHandle) -> IntegrityDisposition {
    match active_source(app_handle) {
        CoreSource::App => {
            let entry = config::get_dsh_install_path(app_handle).join(INSTALLED_CONNECTION_ENTRY);
            integrity_disposition(CoreSource::App, &[entry])
        }
        CoreSource::Local => {
            integrity_disposition(CoreSource::Local, &local_connection_entries(app_handle))
        }
    }
}

/// 已污染的托管核心必须重新走可信下载；本地核心只能由用户包管理器或构建流程恢复。
pub fn ensure_active_core_safe(app_handle: &AppHandle) -> Result<(), String> {
    match active_integrity_disposition(app_handle) {
        IntegrityDisposition::Clean => Ok(()),
        IntegrityDisposition::ReplaceManaged => Err(
            "HARNESS_CORE_REPAIR_REQUIRED: managed core contains a historical authentication bypass; reinstalling the verified release is required"
                .into(),
        ),
        IntegrityDisposition::RejectLocal => Err(
            "HARNESS_LOCAL_CORE_REPAIR_REQUIRED: local core contains a historical authentication bypass; reinstall the package or rebuild the linked checkout"
                .into(),
        ),
    }
}

/// 安装器据此把历史污染的托管核心视为未安装，复用摘要校验与原子替换流程。
pub fn app_core_requires_repair(app_handle: &AppHandle) -> bool {
    let entry = config::get_dsh_install_path(app_handle).join(INSTALLED_CONNECTION_ENTRY);
    contains_historical_bypass(&entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_marker_requires_replacement_and_trusted_fixture_restores_auth() {
        let dir = std::env::temp_dir().join(format!("dsh-auth-integrity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("index.js");
        std::fs::write(
            &entry,
            "return void 0; /* dsh-tauri-desktop: alpha embedded auth bypass */",
        )
        .unwrap();
        assert!(contains_historical_bypass(&entry));
        assert_eq!(
            integrity_disposition(CoreSource::App, std::slice::from_ref(&entry)),
            IntegrityDisposition::ReplaceManaged
        );
        assert_eq!(
            integrity_disposition(CoreSource::Local, std::slice::from_ref(&entry)),
            IntegrityDisposition::RejectLocal
        );

        std::fs::write(
            &entry,
            "return this.browserAuth.isAuthenticated(request) ? void 0 : 401;",
        )
        .unwrap();
        assert!(!contains_historical_bypass(&entry));
        assert_eq!(
            integrity_disposition(CoreSource::App, std::slice::from_ref(&entry)),
            IntegrityDisposition::Clean
        );
        assert!(std::fs::read_to_string(&entry).unwrap().contains(": 401"));
        std::fs::remove_dir_all(dir).ok();
    }
}
