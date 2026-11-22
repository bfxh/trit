//! 仓库内路径工具：归一化、上跳、文件名，以及测试文件 → 被测文件的目标匹配。

pub const JS_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "json"];

pub fn parent_dir(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

pub fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

pub fn up(dir: &str, n: usize) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for _ in 0..n {
        parts.pop();
    }
    parts.join("/")
}

pub fn join(dir: &str, tail: &str) -> String {
    norm(&format!("{dir}/{tail}"))
}

pub fn norm(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}
