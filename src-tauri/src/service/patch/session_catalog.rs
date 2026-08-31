//! 会话目录容错补丁：单个损坏的 Zstandard artifact 不应阻断整个 Workspace 启动。
//!
//! 上游 `listArtifacts` 为保证持久化错误可见，会直接传播 header frame 校验失败；
//! 桌面端因此无法进入界面，也就无法让用户处理该会话。本补丁只拦截明确的
//! `corrupt Zstandard session log`，记录具体路径并跳过目录项；权限、I/O、编码冲突
//! 等错误继续传播，原 artifact 也完全不改动。

use crate::utils::{patch_dsh, PatchOutcome};

const PATCH_MARKER: &str = "dsh-tauri-desktop: skip corrupt Zstandard catalog artifact";
const READ_HEADER_ORIGINAL: &str = r#"				const first = this.compression === "zstd" ? await this.readFirstZstdLine(path, signal) : await this.readFirstLine(path, signal);"#;
const READ_HEADER_PATCHED: &str = r#"				let first;
				try {
					first = this.compression === "zstd" ? await this.readFirstZstdLine(path, signal) : await this.readFirstLine(path, signal);
				} catch (error) {
					const message = error instanceof Error ? error.message : String(error);
					if (!message.startsWith("corrupt Zstandard session log:")) throw error;
					console.error(`[dsh-tauri-desktop] skipping unreadable session artifact ${path}: ${message}`);
					continue; /* dsh-tauri-desktop: skip corrupt Zstandard catalog artifact */
				}"#;

/// 相对活动核心的 JSONL session persistence 实现路径。
const SESSION_PERSISTENCE_INDEX_JS: &str =
    "node_modules/@deepseek-ai/dsh-session-persistence-jsonl/lib/index.js";

fn patch_source(source: &str) -> PatchOutcome {
    if source.contains(PATCH_MARKER) {
        return PatchOutcome::AlreadyPatched;
    }
    if !source.contains(READ_HEADER_ORIGINAL) {
        return PatchOutcome::AnchorMissing;
    }
    PatchOutcome::Patched(source.replacen(READ_HEADER_ORIGINAL, READ_HEADER_PATCHED, 1))
}

/// 对活动核心的会话目录读取应用补丁（幂等）。
pub fn apply(app_handle: &tauri::AppHandle) -> Result<(), String> {
    patch_dsh(app_handle, SESSION_PERSISTENCE_INDEX_JS, patch_source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_catalog_header_read_only() {
        let source = format!("before\n{READ_HEADER_ORIGINAL}\nafter\n");
        let PatchOutcome::Patched(patched) = patch_source(&source) else {
            panic!("expected patched source");
        };
        assert!(patched.contains(PATCH_MARKER));
        assert!(patched.contains("message.startsWith(\"corrupt Zstandard session log:\")"));
        assert!(patched.contains("throw error"));
        assert!(patched.contains("continue;"));
        assert!(!patched.contains(READ_HEADER_ORIGINAL));
    }

    #[test]
    fn patch_is_idempotent() {
        let PatchOutcome::Patched(patched) = patch_source(READ_HEADER_ORIGINAL) else {
            panic!("expected patched source");
        };
        assert_eq!(patch_source(&patched), PatchOutcome::AlreadyPatched);
    }

    #[test]
    fn skips_changed_upstream_anchor() {
        assert_eq!(
            patch_source("const first = await readHeader(path);"),
            PatchOutcome::AnchorMissing
        );
    }
}
