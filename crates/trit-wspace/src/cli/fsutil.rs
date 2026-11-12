//! CLI 落盘工具：临时文件 + 改名（文本与二进制共用）。

use std::fs;

pub(crate) fn atomic_write(path: &str, text: &str) -> Result<(), String> {
    atomic_write_bytes(path, text.as_bytes())
}

pub(crate) fn atomic_write_bytes(path: &str, bytes: &[u8]) -> Result<(), String> {
    let temp = format!("{path}.tmp-{}", std::process::id());
    fs::write(&temp, bytes).map_err(|error| format!("{temp}: {error}"))?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows 上 rename 覆盖已存在文件会失败；先删目标再改名。
            let _ = fs::remove_file(path);
            fs::rename(&temp, path).map_err(|error| {
                let _ = fs::remove_file(&temp);
                format!("{path}: {error}")
            })
        }
    }
}
