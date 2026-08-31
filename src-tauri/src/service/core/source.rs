//! 核心来源判定与「当前活动核心」入口选择。
//!
//! 承载 [`CoreSource`] / [`HarnessCore`] 两个公开类型，以及
//! [`active_source`] / [`active_dsh_binary`] / [`active_version`] 三个供服务启动
//! 与插件操作统一取用的入口。本地核心探测见 [`super::local`]。

use crate::config;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use super::local::local_core;
use crate::service::download::parse_version_from_tag;

/// 从 pnpm monorepo 的已知 workspace 层级中按 manifest 名定位包。
///
/// `pnpm link --global` 会把 `@deepseek-ai/dsh` 解析到 `apps/cli`，其兄弟包并不在
/// `node_modules`，而在仓库的 `packages/*/*`。这里只在包含 `pnpm-workspace.yaml`
/// 的祖先内扫描固定深度，避免把普通安装目录扩大成递归搜索。
fn resolve_workspace_package_dir(root: &Path, package: &str) -> Option<PathBuf> {
    let workspace = root
        .ancestors()
        .find(|ancestor| ancestor.join("pnpm-workspace.yaml").is_file())?;
    let expected_name = format!("@deepseek-ai/{package}");
    for relative_root in ["packages", "apps", "vendor"] {
        let Ok(first_level) = std::fs::read_dir(workspace.join(relative_root)) else {
            continue;
        };
        for first in first_level.flatten() {
            let first_path = first.path();
            if package_manifest_name(&first_path).as_deref() == Some(expected_name.as_str()) {
                return Some(first_path);
            }
            let Ok(second_level) = std::fs::read_dir(&first_path) else {
                continue;
            };
            for second in second_level.flatten() {
                let second_path = second.path();
                if package_manifest_name(&second_path).as_deref() == Some(expected_name.as_str()) {
                    return Some(second_path);
                }
            }
        }
    }
    None
}

/// 读取候选 workspace 包的 npm 名称；非目录或无效 manifest 均视为不匹配。
fn package_manifest_name(dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let manifest = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    manifest.get("name")?.as_str().map(String::from)
}

/// 核心来源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreSource {
    /// 用户通过 CLI 安装的本地核心
    Local,
    /// 桌面端预打包核心
    App,
}

impl CoreSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CoreSource::Local => "local",
            CoreSource::App => "app",
        }
    }

    pub fn parse(source: &str) -> Option<CoreSource> {
        match source {
            "local" => Some(CoreSource::Local),
            "app" => Some(CoreSource::App),
            _ => None,
        }
    }
}

/// 核心列表项（序列化 camelCase 给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCore {
    /// `local` | `app`（无 tag 记录的旧激活行）| `app-<tag>`
    pub id: String,
    pub source: CoreSource,
    /// 版本号（不含 `v` 前缀；缺失为空串）
    pub version: String,
    /// 完整 release tag（如 `dsh-0.1.0-rc.8-32331963388`；local 行为空串）
    pub tag: String,
    /// 核心入口（cli path）：实际传给 Node.js 的 bin.js 绝对路径
    pub path: String,
    /// 「打开目录」入口：本地核心为包目录，预打包为安装/槽位目录；未下载为空
    pub dir: String,
    /// 本地是否可用（文件在盘/可解析）
    pub present: bool,
    /// 当前是否使用中的核心
    pub active: bool,
    /// 是否预览版（GitHub Release 标记 Pre-release，或 tag 命名含预览标记，见
    /// `download::is_preview_tag`）：预览版不参与自动更新提示，但可在核心列表
    /// 手动下载安装，并以「预览版」标签展示。
    pub preview: bool,
    /// 当前版本是否高于资源清单中的推荐版本。
    pub above_recommended: bool,
    /// 本地存在但远程 pkg 仓库已不再提供的历史槽位。
    pub orphaned: bool,
    /// 资源清单中的推荐版本，用于切换前风险提示。
    pub recommended_version: Option<String>,
    pub error: Option<String>,
}

/// 当前活动核心来源（需求 3：本地核心存在时优先，除非用户显式选择预打包）。
pub fn active_source(app_handle: &AppHandle) -> CoreSource {
    let setting = config::get_store_dat_setting(app_handle);
    let local_present = local_core(app_handle).is_some();
    select_source(
        setting.active_core.as_deref().and_then(CoreSource::parse),
        local_present,
    )
}

/// 显式选择本地核心时保持本地来源，即使安装在调用间隙消失也不静默借用内置核心。
fn select_source(selected: Option<CoreSource>, local_present: bool) -> CoreSource {
    match selected {
        Some(CoreSource::App) => CoreSource::App,
        Some(CoreSource::Local) => CoreSource::Local,
        None => {
            if local_present {
                CoreSource::Local
            } else {
                CoreSource::App
            }
        }
    }
}

/// 当前活动核心的 dsh 入口（bin.js 绝对路径）。
///
/// 供服务启动（workflow::launch）与插件操作（plugin::install 等）统一取用，
/// 显式本地核心解析失败时返回错误，禁止静默借用预打包核心。
pub fn active_dsh_binary(app_handle: &AppHandle) -> Result<PathBuf, String> {
    match active_source(app_handle) {
        CoreSource::Local => local_core(app_handle)
            .map(|c| c.bin)
            .ok_or_else(|| "CORE_LOCAL_NOT_FOUND: selected local core is unavailable".to_string()),
        CoreSource::App => Ok(config::get_dsh_binary_path(app_handle)),
    }
}

/// 从核心包或安装前缀解析同一核心树内的 `@deepseek-ai/<package>`。
///
/// 支持四种布局：
/// - 预打包前缀：`<root>/node_modules/@deepseek-ai/<package>`；
/// - 包内嵌套：`<dsh>/node_modules/@deepseek-ai/<package>`；
/// - npm 扁平全局：`<prefix>/node_modules/@deepseek-ai/{dsh,<package>}`。
/// - pnpm workspace link：从仓库的固定 workspace 层级按 package.json 名定位。
pub(crate) fn resolve_core_package_dir(root: &Path, package: &str) -> Option<PathBuf> {
    let nested = root.join("node_modules").join("@deepseek-ai").join(package);
    if nested.join("package.json").is_file() {
        return Some(nested);
    }

    let is_dsh_package = root.file_name().and_then(|name| name.to_str()) == Some("dsh")
        && root
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("@deepseek-ai");
    if is_dsh_package {
        let sibling = root.parent()?.join(package);
        if sibling.join("package.json").is_file() {
            return Some(sibling);
        }
    }

    resolve_workspace_package_dir(root, package)
}

/// 解析活动核心树内的包；本地来源解析失败时不回退到桌面端预打包目录。
pub(crate) fn active_core_package_dir(
    app_handle: &AppHandle,
    package: &str,
) -> Result<PathBuf, String> {
    let root = match active_source(app_handle) {
        CoreSource::Local => local_core(app_handle)
            .map(|core| core.package_dir)
            .ok_or_else(|| {
                "CORE_LOCAL_NOT_FOUND: selected local core is unavailable".to_string()
            })?,
        CoreSource::App => config::get_dsh_install_path(app_handle),
    };
    resolve_core_package_dir(&root, package).ok_or_else(|| {
        format!(
            "CORE_PACKAGE_NOT_FOUND: @deepseek-ai/{package} is not installed under {}",
            root.display()
        )
    })
}

/// 当前活动核心的版本号（`--no-open` 等按版本判定的能力以它为准）。
pub fn active_version(app_handle: &AppHandle) -> Option<String> {
    match active_source(app_handle) {
        CoreSource::Local => local_core(app_handle).map(|c| c.version),
        CoreSource::App => config::get_dsh_pkg_tag(app_handle)
            .as_deref()
            .and_then(parse_version_from_tag)
            .or_else(|| config::get_dsh_version(app_handle)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_source_round_trips() {
        assert_eq!(CoreSource::parse("local"), Some(CoreSource::Local));
        assert_eq!(CoreSource::parse("app"), Some(CoreSource::App));
        assert_eq!(CoreSource::parse("other"), None);
        assert_eq!(CoreSource::Local.as_str(), "local");
        assert_eq!(CoreSource::App.as_str(), "app");
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-core-source-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn write_package(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"name":"fixture"}"#).unwrap();
    }

    #[test]
    fn explicit_local_selection_does_not_fall_back_to_app() {
        assert_eq!(
            select_source(Some(CoreSource::Local), false),
            CoreSource::Local
        );
        assert_eq!(select_source(None, false), CoreSource::App);
    }

    #[test]
    fn resolves_nested_package_layout() {
        let root = temp_dir("nested");
        let renderer = root
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-client-ui-renderer");
        write_package(&renderer);
        assert_eq!(
            resolve_core_package_dir(&root, "dsh-client-ui-renderer"),
            Some(renderer)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolves_npm_flat_package_layout_from_selected_dsh() {
        let scope = temp_dir("flat").join("node_modules").join("@deepseek-ai");
        let dsh = scope.join("dsh");
        let renderer = scope.join("dsh-client-ui-renderer");
        write_package(&dsh);
        write_package(&renderer);
        assert_eq!(
            resolve_core_package_dir(&dsh, "dsh-client-ui-renderer"),
            Some(renderer)
        );
        let _ = std::fs::remove_dir_all(
            scope
                .ancestors()
                .nth(2)
                .expect("scope has node_modules parent"),
        );
    }

    #[test]
    fn packaged_core_resolution_stays_with_packaged_root() {
        let packaged = temp_dir("packaged");
        let unrelated = temp_dir("unrelated");
        let workspace = packaged
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-workspace");
        write_package(&workspace);
        write_package(
            &unrelated
                .join("node_modules")
                .join("@deepseek-ai")
                .join("dsh-workspace"),
        );
        assert_eq!(
            resolve_core_package_dir(&packaged, "dsh-workspace"),
            Some(workspace)
        );
        let _ = std::fs::remove_dir_all(packaged);
        let _ = std::fs::remove_dir_all(unrelated);
    }

    #[test]
    fn resolves_package_from_linked_pnpm_workspace() {
        let root = temp_dir("workspace");
        let cli = root.join("apps").join("cli");
        let persistence = root
            .join("packages")
            .join("session")
            .join("session-persistence-jsonl");
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::create_dir_all(&persistence).unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - packages/*/*\n",
        )
        .unwrap();
        std::fs::write(
            persistence.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh-session-persistence-jsonl"}"#,
        )
        .unwrap();

        assert_eq!(
            resolve_core_package_dir(&cli, "dsh-session-persistence-jsonl"),
            Some(persistence)
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
