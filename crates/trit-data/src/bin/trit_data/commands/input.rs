//! 输入读取：文件，或目录（递归收集 `*.jsonl`）。

use std::path::{Path, PathBuf};

pub fn read_text(path: &Path) -> Result<String, String> {
    if !path.is_dir() {
        return std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
    }
    let mut files = Vec::new();
    collect(path, &mut files)?;
    files.sort();
    let mut out = String::new();
    for f in files {
        out.push_str(&std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
    Ok(())
}
