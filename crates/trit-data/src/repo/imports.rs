//! import 解析：按语言抓 `use` / `import` / `require` / `#include` / `mod` 声明。
//!
//! 只做语法级的浅解析（每行一条），不建 AST；解析不了的语言返回 `false`，
//! 由调用方标记 `imports_unknown`，不做静默丢弃。

pub fn parse(head: &str, lang: &str) -> (Vec<String>, bool) {
    let known = matches!(
        lang,
        "rust"
            | "python"
            | "javascript"
            | "typescript"
            | "go"
            | "java"
            | "kotlin"
            | "scala"
            | "c"
            | "cpp"
            | "cuda"
            | "opencl"
            | "csharp"
    );
    let mut out = Vec::new();
    for line in head.lines() {
        let t = line.trim();
        let imps: Vec<String> = match lang {
            "rust" => super::imports_rust::parse_line(t),
            "python" => one(
                t.strip_prefix("import ")
                    .or_else(|| t.strip_prefix("from "))
                    .map(|s| s.split_whitespace().next().unwrap_or("").to_string()),
            ),
            "javascript" | "typescript" => one(js(t)),
            "go" => one(
                t.strip_prefix("import ")
                    .map(|s| s.trim_matches(|c| c == '"' || c == ';').to_string()),
            ),
            "java" | "kotlin" | "scala" => one(
                t.strip_prefix("import ")
                    .map(|s| s.trim_end_matches(';').trim().to_string()),
            ),
            "c" | "cpp" | "cuda" | "opencl" => one(t.strip_prefix("#include ").map(|s| {
                s.trim_matches(|c| c == '<' || c == '>' || c == '"')
                    .to_string()
            })),
            "csharp" => one(
                t.strip_prefix("using ")
                    .map(|s| s.trim_end_matches(';').trim().to_string()),
            ),
            _ => Vec::new(),
        };
        for i in imps {
            if !i.is_empty() && !out.contains(&i) && out.len() < 50 {
                out.push(i);
            }
        }
    }
    (out, known)
}

fn js(t: &str) -> Option<String> {
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

fn one(v: Option<String>) -> Vec<String> {
    v.into_iter().collect()
}
