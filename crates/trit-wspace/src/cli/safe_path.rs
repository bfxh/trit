//! manifest 里 `file` 字段的路径护栏：`Path::join` 遇绝对路径直接覆盖前缀，攻击者可控的清单能让打包命令读**任意文件**。校验是字符串级、平台无关的（组件语义在 Windows/Linux 不同，平台 `Path` 判断会漏掉另一侧的攻击，CI 实测抓到过）。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// 把清单里的 `file` 字段解析成 `dir` 内的安全路径；越界一律拒绝。
pub fn resolve(dir: &str, file: &str) -> Result<PathBuf> {
    if file.is_empty() {
        return Err(Error::Wire("manifest 的 file 字段为空".into()));
    }
    if file.starts_with('/') || file.starts_with('\\') || Path::new(file).is_absolute() {
        return Err(Error::Wire(format!("manifest 的 file `{file}` 是绝对路径")));
    }
    if file.contains(':') {
        return Err(Error::Wire(format!("manifest 的 file `{file}` 含冒号分量")));
    }
    for part in file.split(['/', '\\']) {
        if matches!(part, "" | "." | "..") {
            return Err(Error::Wire(format!(
                "manifest 的 file `{file}` 含越界分量 {part:?}"
            )));
        }
    }
    Ok(Path::new(dir).join(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_relative_names() {
        let path = resolve("D:/model", "0001-enc.weight.wsp").expect("ok");
        assert_eq!(path, Path::new("D:/model").join("0001-enc.weight.wsp"));
        assert!(resolve("D:/model", "shards/0002-a.wsp").is_ok());
    }

    #[test]
    fn rejects_traversal_absolute_and_empty() {
        for file in [
            "../secrets.wsp",
            "..\\secrets.wsp",
            "C:/Users/x/secrets.wsp",
            "/etc/passwd",
            "\\Windows\\system32",
            "",
            ".",
            "a/./b",
            "a:b",
        ] {
            assert!(resolve("D:/model", file).is_err(), "{file:?} 必须被拒");
        }
    }
}
