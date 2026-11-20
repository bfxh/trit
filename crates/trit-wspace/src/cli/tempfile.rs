//! 落盘的原子写：不可预测的临时文件 + 改名。
//!
//! 名字由 pid + 纳秒 + 进程内计数器组成，用 `create_new` 打开：同目录可写的攻击者
//! 既猜不到名字，预置同名普通文件或符号链接也会让写入失败，而不是写穿到链接目标。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 在 `path` 同目录创建一个空临时文件，返回它的名字与句柄。
fn create(path: &str) -> Result<(String, fs::File), String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = format!("{path}.tmp-{}-{nanos}-{serial}", std::process::id());
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("{temp}: {error}"))?;
    Ok((temp, file))
}

pub(crate) fn atomic_write(path: &str, text: &str) -> Result<(), String> {
    atomic_write_bytes(path, text.as_bytes())
}

pub(crate) fn atomic_write_bytes(path: &str, bytes: &[u8]) -> Result<(), String> {
    let (temp, mut file) = create(path)?;
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(format!("{temp}: {error}"));
    }
    drop(file);
    if fs::rename(&temp, path).is_ok() {
        return Ok(());
    }
    // Windows 上 rename 覆盖已存在文件会失败：先删目标（符号链接只删链接本身）再改名。
    let _ = fs::remove_file(path);
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("{path}: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("trit-atomic-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("建目录");
        dir.join("out.wsp").display().to_string()
    }

    #[test]
    fn overwrites_existing_target_atomically() {
        let target = scratch("overwrite");
        fs::write(&target, b"old").expect("预置目标");
        atomic_write_bytes(&target, b"new").expect("写入");
        assert_eq!(fs::read(&target).expect("读"), b"new");
    }

    #[test]
    fn predictable_placeholder_temp_file_is_left_alone() {
        let target = scratch("placeholder");
        // 攻击者按旧实现的固定临时名预置一个「占位」文件
        let placeholder = format!("{target}.tmp-{}", std::process::id());
        fs::write(&placeholder, b"attacker").expect("预置占位");
        atomic_write_bytes(&target, b"payload").expect("写入");
        assert_eq!(fs::read(&target).expect("读"), b"payload");
        assert_eq!(
            fs::read(&placeholder).expect("读占位"),
            b"attacker",
            "预置的占位文件不应被复用或改写"
        );
    }
}
