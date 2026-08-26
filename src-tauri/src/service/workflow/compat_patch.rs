//! 核心兼容补丁的安全写入工具。

use std::fs;
use std::path::Path;

/// 写补丁前保留 `.dsh-backup`，再用同目录临时文件替换；失败时回滚原文件。
pub(super) fn write_with_backup(
    path: &Path,
    content: &str,
    error_prefix: &str,
) -> Result<(), String> {
    let backup = path.with_extension("dsh-backup");
    fs::copy(path, &backup).map_err(|error| {
        format!(
            "{error_prefix}_BACKUP: {} -> {} failed: {error}",
            path.display(),
            backup.display()
        )
    })?;

    let temp = path.with_extension("dsh-patch-tmp");
    if let Err(error) = fs::write(&temp, content) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "{error_prefix}_WRITE: {} failed: {error}",
            temp.display()
        ));
    }

    let replace = match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(path);
            fs::rename(&temp, path)
        }
    };
    if let Err(error) = replace {
        let _ = fs::remove_file(&temp);
        let _ = fs::copy(&backup, path);
        return Err(format!(
            "{error_prefix}_RENAME: replace {} failed: {error}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_backup_when_replacing_file() {
        let root = std::env::temp_dir().join(format!("dsh-compat-write-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("client.js");
        fs::write(&path, "before").unwrap();
        write_with_backup(&path, "after", "TEST_PATCH").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after");
        assert_eq!(
            fs::read_to_string(path.with_extension("dsh-backup")).unwrap(),
            "before"
        );
        let _ = fs::remove_dir_all(root);
    }
}
