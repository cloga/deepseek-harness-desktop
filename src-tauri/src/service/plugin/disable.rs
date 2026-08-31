//! 插件禁用/启用：从 profile 的 `dsh.profile.bundles` 移除（代码完全不加载），
//! 并写入 profile 的独立禁用清单。与卸载不同，禁用保留 node_modules 内的包体，
//! 启用时无需重新下载。
//!
//! 机制遵循 ADR-0001：禁用 = 从加载列表移除 + 记入独立禁用清单；启用 = 精确逆操作。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::service::fs_guard;
use crate::service::plugin::installed::profile_dir;
use crate::service::plugin::{process, watch};

/// 单条禁用记录（序列化为 camelCase 给前端/磁盘）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DisabledEntry {
    /// 禁用时间（unix 秒级时间戳字符串）。
    pub disabled_at: String,
    /// 禁用来源：当前均为 "user"（用户主动操作）。
    pub reason: String,
}

/// 禁用清单在 profile 目录下的路径。
fn disabled_path(profile: &Path) -> PathBuf {
    profile.join("disabled-plugins.json")
}

/// 读取禁用清单（缺失/损坏按空处理）。
pub(crate) fn load_disabled(profile: &Path) -> HashMap<String, DisabledEntry> {
    let Ok(content) = fs::read_to_string(disabled_path(profile)) else {
        return HashMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// 持久化禁用清单（pretty JSON + 尾部换行）。
fn save_disabled(profile: &Path, map: &HashMap<String, DisabledEntry>) -> Result<(), String> {
    let path = disabled_path(profile);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("DISABLED_DIR_CREATE_FAILED: {e}"))?;
    }
    let json =
        serde_json::to_string_pretty(map).map_err(|e| format!("DISABLED_RENDER_FAILED: {e}"))?;
    fs::write(&path, format!("{json}\n"))
        .map_err(|e| format!("DISABLED_WRITE_FAILED: {e}"))
}

/// 仅从 `dsh.profile.bundles` 移除指定插件（不动 `dependencies`）。
/// 返回是否实际移除了条目。
fn remove_from_bundles(manifest: &mut serde_json::Value, id: &str) -> bool {
    let Some(bundles) = manifest
        .get_mut("dsh")
        .and_then(|d| d.get_mut("profile"))
        .and_then(|p| p.get_mut("bundles"))
        .and_then(|b| b.as_array_mut())
    else {
        return false;
    };
    let before = bundles.len();
    bundles.retain(|b| b.as_str() != Some(id));
    bundles.len() != before
}

/// 把插件加回 `dsh.profile.bundles`（若已存在则不重复添加）。
/// 返回是否实际新增了条目。
fn add_to_bundles(manifest: &mut serde_json::Value, id: &str) -> bool {
    let Some(bundles) = manifest
        .get_mut("dsh")
        .and_then(|d| d.get_mut("profile"))
        .and_then(|p| p.get_mut("bundles"))
        .and_then(|b| b.as_array_mut())
    else {
        return false;
    };
    if bundles.iter().any(|b| b.as_str() == Some(id)) {
        return false;
    }
    bundles.push(serde_json::Value::String(id.to_string()));
    true
}

/// 是否为官方/核心包（`@deepseek-ai/` 前缀）。与 recovery 模块的保护名单一致。
fn is_core_package(id: &str) -> bool {
    id.starts_with("@deepseek-ai/")
}

/// 检查插件是否已安装（dependencies 中存在）。
fn is_in_dependencies(manifest: &serde_json::Value, id: &str) -> bool {
    manifest
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|d| d.contains_key(id))
        .unwrap_or(false)
}

fn now_seconds_string() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

/// 回滚禁用清单到操作前的状态。
///
/// 当 manifest 写入失败时，把已加入禁用清单的条目移除，使两个持久化文件
/// 恢复一致（插件仍在 bundles 中，禁用清单无记录），避免留下
/// 「已从 bundles 移除但未记入禁用清单」的不可恢复状态。
fn rollback_disable(profile: &Path, id: &str) {
    let mut map = load_disabled(profile);
    map.remove(id);
    // 回滚写入也失败时，只能静默——此时清单多一条目但插件仍在 bundles，
    // 下次禁用会覆盖该条目，不会阻塞用户操作。
    let _ = save_disabled(profile, &map);
}

/// 禁用插件的纯逻辑（不依赖 AppHandle，便于单元测试）。
///
/// 1. `fs_guard::validate_id(id)`（路径穿越防护）
/// 2. 拒绝官方/核心包
/// 3. 读取 profile package.json
/// 4. 校验插件已安装（dependencies 中存在）
/// 5. 先写入禁用清单（加入条目）
/// 6. 仅从 bundles 移除（不动 dependencies）并写回 manifest
///
/// 写入顺序保证：若步骤 6 失败，通过回滚步骤 5 使两文件恢复一致，
/// 避免插件从 bundles 移除却未记入禁用清单（否则启用会返回
/// `ENABLE_NOT_DISABLED`，用户无法通过正常流程恢复）。
pub(crate) fn disable_plugin_at(profile: &Path, id: &str) -> Result<(), String> {
    // 先做官方/核心包语义校验（优先级高于字符集校验）：官方包 id 含 `/`，
    // 必须先于 validate_id 拦截，否则会被字符集校验误判为 INVALID_ID。
    if is_core_package(id) {
        return Err(format!(
            "DISABLE_INTERNAL_PLUGIN: refusing to disable internal/official plugin {id}"
        ));
    }
    fs_guard::validate_id(id)?;
    let manifest_path = profile.join("package.json");
    let content = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("DISABLE_READ_MANIFEST: {e}"))?;
    let mut manifest: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("DISABLE_PARSE_MANIFEST: {e}"))?;
    if !is_in_dependencies(&manifest, id) {
        return Err(format!(
            "DISABLE_NOT_INSTALLED: plugin {id} is not installed"
        ));
    }
    // 先写禁用清单，确保 manifest 写入失败时可回滚该条目。
    let mut map = load_disabled(profile);
    map.insert(
        id.to_string(),
        DisabledEntry {
            disabled_at: now_seconds_string(),
            reason: "user".to_string(),
        },
    );
    save_disabled(profile, &map)?;
    remove_from_bundles(&mut manifest, id);
    let rendered = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("DISABLE_RENDER_MANIFEST: {e}"))?;
    if let Err(e) = fs::write(&manifest_path, format!("{rendered}\n")) {
        rollback_disable(profile, id);
        return Err(format!("DISABLE_WRITE_MANIFEST: {e}"));
    }
    Ok(())
}

/// 回滚启用前的禁用清单状态。
///
/// 当 manifest 写入失败时，把已移除的禁用条目加回，使两个持久化文件
/// 恢复一致（插件仍在禁用清单中，bundles 未记录），避免留下
/// 「已加回 bundles 但禁用清单仍缺条目」的不可恢复状态。
fn rollback_enable(profile: &Path, id: &str) {
    let mut map = load_disabled(profile);
    map.entry(id.to_string()).or_insert_with(|| DisabledEntry {
        disabled_at: now_seconds_string(),
        reason: "user".to_string(),
    });
    let _ = save_disabled(profile, &map);
}

/// 启用插件的纯逻辑（不依赖 AppHandle，便于单元测试）。
///
/// 1. `fs_guard::validate_id(id)`
/// 2. 读取 manifest
/// 3. 校验插件仍安装
/// 4. 校验插件在禁用清单中
/// 5. 先从禁用清单移除条目
/// 6. 加回 bundles 并写回 manifest
///
/// 写入顺序保证：若步骤 6 失败，通过回滚步骤 5 使两文件恢复一致，
/// 避免插件加回 bundles 后禁用清单缺条目（否则再次启用会返回
/// `ENABLE_NOT_DISABLED`，用户无法通过正常流程恢复）。
pub(crate) fn enable_plugin_at(profile: &Path, id: &str) -> Result<(), String> {
    fs_guard::validate_id(id)?;
    let manifest_path = profile.join("package.json");
    let content = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("ENABLE_READ_MANIFEST: {e}"))?;
    let mut manifest: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("ENABLE_PARSE_MANIFEST: {e}"))?;
    if !is_in_dependencies(&manifest, id) {
        return Err(format!(
            "ENABLE_NOT_INSTALLED: plugin {id} is not installed"
        ));
    }
    let mut map = load_disabled(profile);
    if !map.contains_key(id) {
        return Err(format!(
            "ENABLE_NOT_DISABLED: plugin {id} is not in the disabled list"
        ));
    }
    // 先从禁用清单移除，确保 manifest 写入失败时可回滚该条目。
    map.remove(id);
    save_disabled(profile, &map)?;
    add_to_bundles(&mut manifest, id);
    let rendered = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("ENABLE_RENDER_MANIFEST: {e}"))?;
    if let Err(e) = fs::write(&manifest_path, format!("{rendered}\n")) {
        rollback_enable(profile, id);
        return Err(format!("ENABLE_WRITE_MANIFEST: {e}"));
    }
    Ok(())
}

/// 禁用插件（AppHandle 入口）：获取 profile 目录 → 持有操作锁 → 执行纯逻辑 → 推送变更。
pub fn disable(app_handle: &AppHandle, id: &str) -> Result<(), String> {
    let profile = profile_dir(app_handle);
    let _guard = tauri::async_runtime::block_on(process::acquire_operation_lock());
    let result = disable_plugin_at(&profile, id);
    drop(_guard);
    watch::force_emit(app_handle);
    result
}

/// 启用插件（AppHandle 入口）：获取 profile 目录 → 持有操作锁 → 执行纯逻辑 → 推送变更。
pub fn enable(app_handle: &AppHandle, id: &str) -> Result<(), String> {
    let profile = profile_dir(app_handle);
    let _guard = tauri::async_runtime::block_on(process::acquire_operation_lock());
    let result = enable_plugin_at(&profile, id);
    drop(_guard);
    watch::force_emit(app_handle);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造临时 profile 目录并写入 package.json 清单。
    fn build_profile(test_name: &str, id: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-disable-test-{}-{}-{}",
            test_name,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let manifest = serde_json::json!({
            "name": "dsh-profile-web",
            "private": true,
            "dependencies": {
                "dsh-better-sidebar": "1.0.0",
                "dshmarket": "2.0.0",
                "@deepseek-ai/dsh-base": "1.0.0"
            },
            "dsh": { "profile": { "bundles": ["dsh-better-sidebar", "dshmarket", "@deepseek-ai/dsh-base"] } }
        });
        fs::write(
            dir.join("package.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        dir
    }

    fn read_manifest(profile: &Path) -> serde_json::Value {
        let content = fs::read_to_string(profile.join("package.json")).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    #[test]
    fn disable_removes_from_bundles_only() {
        let profile = build_profile("bundles-only", "a");
        disable_plugin_at(&profile, "dsh-better-sidebar").unwrap();

        let manifest = read_manifest(&profile);
        // dependencies 不变
        assert!(manifest["dependencies"]["dsh-better-sidebar"].is_string());
        assert!(manifest["dependencies"]["dshmarket"].is_string());
        // bundles 中已移除
        let bundles = manifest["dsh"]["profile"]["bundles"].as_array().unwrap();
        assert!(!bundles.iter().any(|b| b.as_str() == Some("dsh-better-sidebar")));
        assert!(bundles.iter().any(|b| b.as_str() == Some("dshmarket")));

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn disable_writes_disabled_json() {
        let profile = build_profile("disabled-json", "b");
        disable_plugin_at(&profile, "dsh-better-sidebar").unwrap();

        let map = load_disabled(&profile);
        let entry = map.get("dsh-better-sidebar").expect("disabled entry exists");
        assert_eq!(entry.reason, "user");
        assert!(!entry.disabled_at.is_empty());
        // 时间戳是纯数字字符串
        assert!(entry.disabled_at.parse::<u64>().is_ok());

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn enable_restores_to_bundles() {
        let profile = build_profile("enable-restore", "c");
        disable_plugin_at(&profile, "dsh-better-sidebar").unwrap();
        enable_plugin_at(&profile, "dsh-better-sidebar").unwrap();

        let manifest = read_manifest(&profile);
        let bundles = manifest["dsh"]["profile"]["bundles"].as_array().unwrap();
        assert!(bundles.iter().any(|b| b.as_str() == Some("dsh-better-sidebar")));

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn enable_removes_from_disabled_json() {
        let profile = build_profile("enable-rm-json", "d");
        disable_plugin_at(&profile, "dsh-better-sidebar").unwrap();
        enable_plugin_at(&profile, "dsh-better-sidebar").unwrap();

        let map = load_disabled(&profile);
        assert!(!map.contains_key("dsh-better-sidebar"));

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn disable_internal_plugin_is_rejected() {
        let profile = build_profile("internal-reject", "e");
        let err = disable_plugin_at(&profile, "@deepseek-ai/dsh-base").unwrap_err();
        assert!(err.contains("DISABLE_INTERNAL_PLUGIN"));

        // manifest 未被修改
        let manifest = read_manifest(&profile);
        let bundles = manifest["dsh"]["profile"]["bundles"].as_array().unwrap();
        assert_eq!(bundles.len(), 3);

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn disable_not_installed_is_rejected() {
        let profile = build_profile("not-installed", "f");
        let err = disable_plugin_at(&profile, "dsh-not-exist").unwrap_err();
        assert!(err.contains("DISABLE_NOT_INSTALLED"));

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn enable_not_disabled_is_rejected() {
        let profile = build_profile("not-disabled", "g");
        // 未禁用就启用 → 拒绝
        let err = enable_plugin_at(&profile, "dsh-better-sidebar").unwrap_err();
        assert!(err.contains("ENABLE_NOT_DISABLED"));

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn round_trip_preserves_other_bundles() {
        let profile = build_profile("round-trip", "h");
        disable_plugin_at(&profile, "dsh-better-sidebar").unwrap();
        enable_plugin_at(&profile, "dsh-better-sidebar").unwrap();

        let manifest = read_manifest(&profile);
        let bundles = manifest["dsh"]["profile"]["bundles"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b.as_str())
            .collect::<Vec<_>>();
        // 恢复原始顺序可能不同，但内容一致
        assert_eq!(bundles.len(), 3);
        assert!(bundles.contains(&"dsh-better-sidebar"));
        assert!(bundles.contains(&"dshmarket"));
        assert!(bundles.contains(&"@deepseek-ai/dsh-base"));
        // dependencies 完全不变
        assert!(manifest["dependencies"]["dsh-better-sidebar"].is_string());
        assert!(manifest["dependencies"]["dshmarket"].is_string());
        assert!(manifest["dependencies"]["@deepseek-ai/dsh-base"].is_string());

        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn disabled_list_persists_to_disk() {
        let profile = build_profile("persist", "i");
        disable_plugin_at(&profile, "dsh-better-sidebar").unwrap();

        // 重新从磁盘读取（不依赖内存）
        let map = load_disabled(&profile);
        assert!(map.contains_key("dsh-better-sidebar"));

        // 文件确实存在且可解析
        let path = disabled_path(&profile);
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("dsh-better-sidebar"));
        assert!(content.contains("\"disabledAt\""));
        assert!(content.contains("\"reason\""));

        let _ = fs::remove_dir_all(&profile);
    }
}
