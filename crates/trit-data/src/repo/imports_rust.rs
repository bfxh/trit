//! Rust 行解析：`use` / `mod` 声明；`use a::{b, c}` 展开成单条依赖。

pub fn parse_line(t: &str) -> Vec<String> {
    let s = t.strip_prefix("pub ").unwrap_or(t);
    if let Some(x) = s.strip_prefix("use ") {
        return expand(x.trim_end_matches(';').trim());
    }
    s.strip_prefix("mod ")
        .and_then(|m| m.strip_suffix(';'))
        .map(|m| vec![m.trim().to_string()])
        .unwrap_or_default()
}

fn expand(s: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (s.find('{'), s.rfind('}')) else {
        return vec![s.to_string()];
    };
    let prefix = &s[..open];
    let mut out = Vec::new();
    for item in s[open + 1..close].split(',') {
        let item = item.trim();
        if item.is_empty() || matches!(item, "self" | "*") || item.contains(['{', '}']) {
            continue;
        }
        out.push(if item.contains("::") && prefix.is_empty() {
            item.to_string()
        } else {
            format!("{prefix}{item}")
        });
    }
    if out.is_empty() {
        vec![prefix.trim_end_matches("::").to_string()]
    } else {
        out
    }
}
