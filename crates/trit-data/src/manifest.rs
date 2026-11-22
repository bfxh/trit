//! 数据集清单：对输出文件集合做内容级指纹（复用 `trit-learn::manifest`）。

use std::path::Path;

use trit_learn::manifest::{Fingerprint, Item, hash_items, norm};

pub fn dataset_manifest(dir: &Path, files: &[String]) -> Result<Fingerprint, String> {
    let mut items = Vec::with_capacity(files.len());
    for name in files {
        items.push(Item {
            key: norm(Path::new(name)),
            path: dir.join(name),
        });
    }
    hash_items(&items, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("trit-data-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn fingerprint_is_content_level_and_stable() {
        let dir = tmp("manifest");
        std::fs::write(dir.join("a.jsonl"), "a").unwrap();
        std::fs::write(dir.join("b.jsonl"), "b").unwrap();
        let files = vec!["a.jsonl".to_string(), "b.jsonl".to_string()];
        let x = dataset_manifest(&dir, &files).unwrap();
        let y = dataset_manifest(&dir, &files).unwrap();
        assert_eq!(x.hash, y.hash);
        assert_eq!((x.files, x.bytes), (2, 2));
        std::fs::write(dir.join("b.jsonl"), "changed").unwrap();
        let z = dataset_manifest(&dir, &files).unwrap();
        assert_ne!(x.hash, z.hash);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
