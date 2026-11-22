use std::path::{Component, Path};

use crate::Error;
use crate::digest;

/// 宿主计算测试工件摘要：路径排序后把「路径 + 内容」拼起来做 SHA-256。
///
/// 它固定“这次跑的是哪份测试代码”，不证明测试有语义价值；路径必须
/// repo-relative，绝对路径、盘符前缀和 `..` 直接拒绝。
pub fn test_files_digest(paths: &[String]) -> Result<String, Error> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(paths.len());
    for raw in paths {
        let path = Path::new(raw);
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(Error::ReceiptExecution {
                task: "test-files".into(),
                message: format!("test file must stay repo-relative: {raw}"),
            });
        }
        let bytes = std::fs::read(path).map_err(|error| Error::ReceiptExecution {
            task: "test-files".into(),
            message: format!("read {raw}: {error}"),
        })?;
        files.push((raw.replace('\\', "/"), bytes));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut input = Vec::new();
    for (name, bytes) in files {
        input.extend_from_slice(name.as_bytes());
        input.push(0);
        input.extend_from_slice(&bytes);
        input.push(0);
    }
    Ok(digest::hex(&digest::sha256(&input)))
}
