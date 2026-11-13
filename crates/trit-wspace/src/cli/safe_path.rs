//! manifest 里 `file` 字段的路径护栏：目录清单是攻击者可控输入。
//!
//! `Path::join` 的语义是「绝对路径直接覆盖前缀」，所以清单里写
//! `"C:/Users/x/secrets"` 或 `"../../secrets.wsp"` 会让打包命令读到清单
//! 指向的**任意文件**并打进容器。这里统一拒绝：绝对路径、`..` 分量、
//! 空串、盘符前缀；只允许落在清单同目录下的普通相对路径。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// 把清单里的 `file` 字段解析成 `dir` 内的安全路径；越界一律拒绝。
pub fn resolve(dir: &str, file: &str) -> Result<PathBuf> {
    if file.is_empty() {
        return Err(Error::Wire("manifest 的 file 字段为空".into()));
    }
    let candidate = Path::new(file);
    if candidate.is_absolute() {
        return Err(Error::Wire(format!("manifest 的 file `{file}` 是绝对路径")));
    }
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(_) => {}
            other => {
                return Err(Error::Wire(format!(
                    "manifest 的 file `{file}` 含越界分量 {other:?}"
                )));
            }
        }
    }
    let joined = Path::new(dir).join(candidate);
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_relative_names() {
        let path = resolve("D:/model", "0001-enc.weight.wsp").expect("ok");
        assert_eq!(path, Path::new("D:/model").join("0001-enc.weight.wsp"));
        let nested = resolve("D:/model", "shards/0002-a.wsp").expect("ok");
        assert!(nested.starts_with("D:/model"));
    }

    #[test]
    fn rejects_traversal_absolute_and_empty() {
        assert!(resolve("D:/model", "../secrets.wsp").is_err());
        assert!(resolve("D:/model", "..\\secrets.wsp").is_err());
        assert!(resolve("D:/model", "C:/Users/x/secrets.wsp").is_err());
        assert!(resolve("D:/model", "/etc/passwd").is_err());
        assert!(resolve("D:/model", "").is_err());
        assert!(resolve("D:/model", ".").is_err());
    }
}
