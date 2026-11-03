//! `Cargo.lock` 的抽取面：把 `[[package]]` 段读成 `Pin` 清单（name/version/checksum）。
//!
//! 从 `supply.rs` 拆出来是因为供应链判定有两半——**读锁文件**与**比基线**，
//! 前者是宽容的文本解析（易改、易出边界情况），后者是三值裁决协议（稳定）。
//! 公开路径不变：`supply::parse_lock` 由这里 `pub use` 转发。

use super::Pin;

/// 解析 Cargo.lock 文本，抽出全部 `[[package]]` 段的 name/version/checksum。
///
/// 宽容解析：段内只认 `name =`、`version =`、`checksum =` 开头的行，
/// `dependencies = [...]` 多行数组等其余内容忽略；`#` 注释跳过。
pub fn parse_lock(text: &str) -> Vec<Pin> {
    let mut pins: Vec<Pin> = Vec::new();
    let mut in_pkg = false;
    let mut name = String::new();
    let mut version = String::new();
    let mut source: Option<String> = None;
    let mut checksum: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("#") || line.is_empty() {
            continue;
        }
        if line.starts_with("[[package]]") {
            // 新段开始：先落上一段
            if in_pkg && !name.is_empty() {
                pins.push(Pin {
                    name: std::mem::take(&mut name),
                    version: std::mem::take(&mut version),
                    source: source.take(),
                    checksum: checksum.take(),
                });
            }
            in_pkg = true;
            name.clear();
            version.clear();
            source = None;
            checksum = None;
            continue;
        }
        if line.starts_with('[') {
            // 其它段（[[patch]]、[metadata] 等）：落上一段并退出包段
            if in_pkg && !name.is_empty() {
                pins.push(Pin {
                    name: std::mem::take(&mut name),
                    version: std::mem::take(&mut version),
                    source: source.take(),
                    checksum: checksum.take(),
                });
            }
            in_pkg = false;
            name.clear();
            version.clear();
            source = None;
            checksum = None;
            continue;
        }
        if !in_pkg {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = strip_toml_str(rest) {
                name = v;
            }
        } else if let Some(rest) = line.strip_prefix("version") {
            if let Some(v) = strip_toml_str(rest) {
                version = v;
            }
        } else if let Some(rest) = line.strip_prefix("source") {
            if let Some(v) = strip_toml_str(rest) {
                source = Some(v);
            }
        } else if let Some(rest) = line.strip_prefix("checksum") {
            if let Some(v) = strip_toml_str(rest) {
                checksum = Some(v);
            }
        }
    }
    if in_pkg && !name.is_empty() {
        pins.push(Pin {
            name,
            version,
            source,
            checksum,
        });
    }
    pins
}

/// 剥 TOML 字符串值：` = "0.1.0"` → `0.1.0`。非 `= "..."` 形态返回 None。
pub(crate) fn strip_toml_str(after_key: &str) -> Option<String> {
    let rest = after_key.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
