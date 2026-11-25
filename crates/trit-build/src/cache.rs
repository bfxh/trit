//! 产物缓存命中：只认 `build.json` 里每个产物的哈希。
//!
//! 从 `main.rs` 拆出，主流程只负责「算」与「写」；缓存判断独立可测。

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::hash64;

/// 读缓存清单；产物哈希全部对得上就返回清单（可跳过构建）。
pub(crate) fn cache_hit(manifest_path: &Path, out_dir: &Path) -> Option<Value> {
    let text = fs::read_to_string(manifest_path).ok()?;
    let manifest: Value = serde_json::from_str(&text).ok()?;
    let outputs = manifest.get("outputs")?.as_object()?;
    for (name, info) in outputs {
        let path = out_dir.join(name);
        let bytes = fs::read(&path).ok()?;
        if hash64(&bytes) != u64::from_str_radix(info.get("hash")?.as_str()?, 16).ok()? {
            return None;
        }
    }
    Some(manifest)
}
