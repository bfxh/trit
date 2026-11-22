//! 各语言的单行 import 提取（不带 AST），以及常量字符串的动态导入。

pub fn py(t: &str) -> Option<String> {
    t.strip_prefix("import ")
        .or_else(|| t.strip_prefix("from "))
        .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
}

pub fn js(t: &str) -> Option<String> {
    t.strip_prefix("import ")
        .map(|s| {
            s.split(" from ")
                .next()
                .unwrap_or(s)
                .trim_matches(|c| c == '"' || c == '\'' || c == ';')
                .to_string()
        })
        .or_else(|| {
            t.split("require(").nth(1).map(|s| {
                s.split(')')
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string()
            })
        })
}

pub fn go(t: &str) -> Option<String> {
    t.strip_prefix("import ").map(|s| {
        s.trim_matches(|c| c == '"' || c == ';' || c == '(' || c == ')')
            .to_string()
    })
}

pub fn jvm(t: &str) -> Option<String> {
    t.strip_prefix("import ")
        .map(|s| s.trim_end_matches(';').trim().to_string())
}

pub fn include(t: &str) -> Option<String> {
    t.strip_prefix("#include ").map(|s| {
        s.trim_matches(|c| c == '<' || c == '>' || c == '"')
            .to_string()
    })
}

pub fn csharp(t: &str) -> Option<String> {
    t.strip_prefix("using ")
        .map(|s| s.trim_end_matches(';').trim().to_string())
}

/// 常量字符串的动态导入：`importlib.import_module("x")` / `__import__("x")` / `import("x")`。
pub fn dynamic(t: &str, lang: &str) -> Option<String> {
    let heads: &[&str] = match lang {
        "python" => &["importlib.import_module(", "__import__("],
        "javascript" | "typescript" => &["import("],
        _ => return None,
    };
    for h in heads {
        if let Some(rest) = t.split(h).nth(1) {
            let lit = rest.split(['"', '\'']).nth(1).unwrap_or("");
            if !lit.is_empty() {
                return Some(lit.to_string());
            }
        }
    }
    None
}
