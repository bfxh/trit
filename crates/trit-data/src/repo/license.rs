//! 仓库根许可证识别：只读仓库根，不向上层目录猜。

use std::path::Path;

use super::read_head;

const MARKS: &[&str] = &[
    "MIT", "APACHE", "GPL", "BSD", "MPL", "UNLICENSE", "PROPRIETARY", "ISC",
];
pub fn detect(root: &Path) -> Option<String> {
    let entries = std::fs::read_dir(root).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_uppercase();
        if name.starts_with("LICENSE") || name.starts_with("COPYING") || name.starts_with("NOTICE") {
            let text = read_head(&e.path(), 4096).to_uppercase();
            for k in MARKS {
                if text.contains(k) {
                    return Some((*k).to_string());
                }
            }
            return Some("UNKNOWN".to_string());
        }
    }
    None
}

