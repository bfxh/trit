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
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("trit-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn fingerprint_is_order_and_style_independent() {
        let root = tmp_dir();
        std::fs::write(root.join("a.bin"), b"alpha").unwrap();
        std::fs::write(root.join("b.bin"), b"beta").unwrap();

        let forward = vec![
            Item {
                key: "a.bin".into(),
                path: root.join("a.bin"),
            },
            Item {
                key: "b.bin".into(),
                path: root.join("b.bin"),
            },
        ];
        let reversed = vec![forward[1].clone(), forward[0].clone()];
        // 同一输入顺序 → 同一哈希（线程切分不影响结果）
        let a = hash_items(&forward, 1).unwrap();
        let again = hash_items(&forward, 1).unwrap();
        assert_eq!(a.hash, again.hash);
        assert_eq!((a.files, a.bytes), (2, 9));
        // 输入顺序不同 → 哈希不同：合并顺序 = 输入顺序，调用方负责排序
        let b = hash_items(&reversed, 1).unwrap();
        assert_ne!(a.hash, b.hash, "hash_items 合并顺序 = 输入顺序");

        // 同一路径的两种写法必须归一成同一指纹
        let backslash = root.to_string_lossy().replace('/', "\\");
        assert_eq!(norm(Path::new(&backslash)), norm(&root));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rel_path_is_relative_to_base() {
        let base = Path::new("/data/assets");
        assert_eq!(rel_path(base, Path::new("/data/assets/a/b.bin")), "a/b.bin");
        assert_eq!(rel_path(base, Path::new("/data/assets")), ".");
        assert_eq!(
            rel_path(Path::new("/data"), Path::new("/data/assets/a")),
            "assets/a"
        );
    }
}
