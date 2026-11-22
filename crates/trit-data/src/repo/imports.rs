//! import 解析：按语言抓 `use` / `import` / `require` / `#include` / `mod` 声明。
//!
//! 只做语法级的浅解析（每行一条），不建 AST；解析不了的语言给 `known=false`，
//! 由调用方标记 `imports_unknown`，不做静默丢弃。常量字符串的动态导入单独入册。

use super::imports_lang as lang;

const KNOWN: &[&str] = &[
    "rust", "python", "javascript", "typescript", "go", "java", "kotlin", "scala", "c", "cpp",
    "cuda", "opencl", "csharp",
];

pub struct Parsed {
    pub imports: Vec<String>,
    pub dynamic: Vec<String>,
    pub known: bool,
}

pub fn parse(head: &str, lang_name: &str) -> Parsed {
    let mut imports = Vec::new();
    let mut dynamic = Vec::new();
    for line in head.lines() {
        let t = line.trim();
        let imps: Vec<String> = match lang_name {
            "rust" => super::imports_rust::parse_line(t),
            "python" => one(lang::py(t)),
            "javascript" | "typescript" => one(lang::js(t)),
            "go" => one(lang::go(t)),
            "java" | "kotlin" | "scala" => one(lang::jvm(t)),
            "c" | "cpp" | "cuda" | "opencl" => one(lang::include(t)),
            "csharp" => one(lang::csharp(t)),
            _ => Vec::new(),
        };
        for i in imps {
            if !i.is_empty() && !imports.contains(&i) && imports.len() < 50 {
                imports.push(i);
            }
        }
        if let Some(d) = lang::dynamic(t, lang_name)
            && !dynamic.contains(&d)
        {
            dynamic.push(d);
        }
    }
    Parsed {
        imports,
        dynamic,
        known: KNOWN.contains(&lang_name),
    }
}

fn one(v: Option<String>) -> Vec<String> {
    v.into_iter().collect()
}
