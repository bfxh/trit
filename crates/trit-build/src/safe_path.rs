//! 缓存清单里的产物名解析：只允许单段文件名，绝不逃出输出目录。
//!
//! 清单是磁盘上的普通 JSON，可能被篡改；读取侧必须把 `name` 当不可信输入。

use std::path::{Component, Path, PathBuf};

/// 把清单产物名接到 `dir` 下；名字不是单段普通文件名时返回 `None`。
///
/// 拒绝空名、`.`/`..`、`/`、`\`、盘符/UNC、NUL/控制字符、Windows 设备名，
/// 以及结尾的点或空格（Windows 会归一化这些名字）。
pub(crate) fn safe_join(dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(char::is_control)
        || name.ends_with('.')
        || name.ends_with(' ')
        || is_windows_device(name)
    {
        return None;
    }
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(_)), None) => Some(dir.join(name)),
        _ => None,
    }
}

fn is_windows_device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .trim_end_matches('.');
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_artifact_names() {
        for name in [
            "expr.txt",
            "truth.csv",
            "truth.tritbin",
            "lookup.c",
            "direct.c",
            "design.v",
            "design_tb.v",
            "expected.mem",
            "truth.wgsl",
            "truth.gpu.bin",
            "truth_gpu.json",
        ] {
            let path = safe_join(Path::new("out"), name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(path, Path::new("out").join(name));
        }
    }

    #[test]
    fn rejects_traversal_absolute_and_device_names() {
        for name in [
            "",
            ".",
            "..",
            "../secret.txt",
            "../../../../etc/hostname",
            "a/b",
            "/etc/passwd",
            "C:\\Windows\\System32",
            "C:/Windows/System32",
            "C:foo",
            "\\\\server\\share",
            "\\server\\share",
            "foo\\bar",
            "a\0b",
            "con",
            "CON.txt",
            "nul",
            "com1",
            "lpt9",
            "foo.",
            "foo ",
        ] {
            assert!(safe_join(Path::new("out"), name).is_none(), "{name:?}");
        }
    }
}
