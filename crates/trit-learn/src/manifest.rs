//! 评测协议的第一件：**内容级指纹**（清单里记它，复核器重算它）。
//!
//! 口径与 `trit-parts` 的深度/部件/纹理/场景四条线一致：逐文件 SHA-256，再对
//! 「相对键 + 文件哈希」的规范序列做一次总哈希。线程数不影响结果（合并顺序 = 输入顺序）。

use std::path::{Component, Path, PathBuf};

use trit_core::fingerprint::{file_sha256, sha256_hex};

/// 一份数据集的内容级指纹。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    pub hash: String,
}

/// 参与指纹的一个文件：`key` 是清单里记的相对键。
#[derive(Clone, Debug)]
pub struct Item {
    pub key: String,
    pub path: PathBuf,
}

/// 路径写法归一：`E:\x` 与 `E:/x` 必须得到同一指纹。
pub fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 把 `p` 写成相对 `base` 的路径（用于可搬移的资产包清单）。
pub fn rel_path(base: &Path, p: &Path) -> String {
    let b: Vec<Component> = base.components().collect();
    let q: Vec<Component> = p.components().collect();
    let mut i = 0usize;
    while i < b.len() && i < q.len() && b[i] == q[i] {
        i += 1;
    }
    let mut out = PathBuf::new();
    for _ in i..b.len() {
        out.push("..");
    }
    for c in &q[i..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        ".".into()
    } else {
        norm(&out)
    }
}
/// 计算一组文件的内容级指纹；`dirs` 是场景/目录数，只用于清单展示。
pub fn hash_items(items: &[Item], dirs: usize) -> Result<Fingerprint, String> {
    let mut bytes = 0u64;
    for item in items {
        bytes += std::fs::metadata(&item.path)
            .map_err(|e| format!("{}: {e}", item.path.display()))?
            .len();
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    let chunk = items.len().div_ceil(threads).max(1);
    let mut parts: Vec<Vec<String>> = Vec::new();
    std::thread::scope(|scope| -> Result<(), String> {
        let mut handles = Vec::new();
        for slice in items.chunks(chunk) {
            handles.push(scope.spawn(move || -> Result<Vec<String>, String> {
                let mut out = Vec::with_capacity(slice.len());
                for item in slice {
                    let hash = file_sha256(&item.path)?;
                    out.push(format!("{} {hash}", item.key));
                }
                Ok(out)
            }));
        }
        for handle in handles {
            let got = handle.join().map_err(|_| "指纹线程 panic".to_string())?;
            parts.push(got?);
        }
        Ok(())
    })?;
    let mut acc: Vec<u8> = Vec::new();
    for line in parts.iter().flatten() {
        acc.extend_from_slice(line.as_bytes());
        acc.push(b'\n');
    }
    Ok(Fingerprint {
        dirs,
        files: items.len(),
        bytes,
        hash: sha256_hex(&acc),
    })
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
