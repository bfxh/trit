//! 仓库扫描：源码树 → 文件级 `RepoUnit`（路径 / 语言 / 大小 / 生成 / 许可证 / 依赖 / 测试）。

use std::path::Path;

use crate::code_langs::code_languages;
use crate::code_quality::check;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub repo: String,
    pub path: String,
    pub lang: String,
    pub bytes: u64,
    pub generated: bool,
    pub license: Option<String>,
    pub imports: Vec<String>,
    pub imports_known: bool,
    pub is_test: bool,
}

pub fn scan(root: &Path) -> Result<Vec<Unit>, String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    let license = detect_license(&root);
    let mut out = Vec::new();
    walk(&root, &root, &repo, license.as_deref(), &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

const SKIP: &[&str] = &[
    ".git", "target", "node_modules", "vendor", "third_party", "dist", "build", ".venv", "venv",
    "__pycache__", ".next", ".cache", ".turbo", "coverage",
];

const EXTS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "h", "cc", "cpp", "hpp", "cs", "php",
    "rb", "swift", "kt", "scala", "sql", "sh", "bash", "zsh", "fish", "ps1", "psm1", "lua", "r",
    "jl", "hs", "ex", "exs", "erl", "ml", "lean", "glsl", "hlsl", "wgsl", "cu", "cl", "metal",
    "asm", "s", "v", "sv", "vhd", "vhdl", "md", "markdown", "toml", "yaml", "yml", "tex", "rst",
    "proto", "graphql",
];

fn walk(
    root: &Path,
    dir: &Path,
    repo: &str,
    license: Option<&str>,
    out: &mut Vec<Unit>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = entry.map_err(|e| e.to_string())?.path();
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if p.is_dir() {
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            walk(root, &p, repo, license, out)?;
        } else if is_source(&p) {
            let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let head = read_head(&p, 64 * 1024);
            let lang = detect_lang(&p, &head);
            let q = check(&head);
            let generated = q.auto_generated || is_generated_path(&p);
            let (imports, imports_known) = parse_imports(&head, &lang);
            let is_test = is_test_path(&p);
            out.push(Unit {
                repo: repo.to_string(),
                path: rel(root, &p),
                lang,
                bytes,
                generated,
                license: license.map(|s| s.to_string()),
                imports,
                imports_known,
                is_test,
            });
        }
    }
    Ok(())
}

fn is_source(p: &Path) -> bool {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    if name == "Dockerfile" || name == "Makefile" {
        return true;
    }
    p.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| EXTS.contains(&x.to_lowercase().as_str()))
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read_head(p: &Path, n: usize) -> String {
    let data = std::fs::read(p).unwrap_or_default();
    String::from_utf8_lossy(&data[..data.len().min(n)]).into_owned()
}

fn detect_lang(p: &Path, head: &str) -> String {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    if name == "Dockerfile" {
        return "dockerfile".into();
    }
    if name == "Makefile" {
        return "makefile".into();
    }
    let ext = p
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_lowercase();
    let lang = match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "rb" => "ruby",
        "swift" => "swift",
        "kt" => "kotlin",
        "scala" => "scala",
        "sql" => "sql",
        "sh" | "bash" => "bash",
        "zsh" => "zsh",
        "fish" => "fish",
        "ps1" | "psm1" => "powershell",
        "lua" => "lua",
        "r" => "r",
        "jl" => "julia",
        "hs" => "haskell",
        "ex" | "exs" => "elixir",
        "erl" => "erlang",
        "ml" => "ocaml",
        "lean" => "lean",
        "glsl" => "glsl",
        "hlsl" => "hlsl",
        "wgsl" => "wgsl",
        "cu" => "cuda",
        "cl" => "opencl",
        "metal" => "metal",
        "asm" | "s" => "assembly",
        "v" | "sv" => "verilog",
        "vhd" | "vhdl" => "vhdl",
        "md" | "markdown" => "markdown",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "tex" => "latex",
        "rst" => "rst",
        "proto" => "protobuf",
        "graphql" => "graphql",
        _ => {
            return code_languages(head)
                .first()
                .copied()
                .unwrap_or("unknown")
                .to_string();
        }
    };
    lang.to_string()
}

const GENERATED_MARKS: &[&str] = &["/generated/", "_generated.", ".gen.", ".pb.go", "_pb2.py"];
const TEST_MARKS: &[&str] = &["/test/", "/tests/", "_test.", ".test.", ".spec.", "/spec/"];

fn norm_path(p: &Path) -> String { p.to_string_lossy().to_lowercase().replace('\\', "/") }

fn is_generated_path(p: &Path) -> bool {
    let s = norm_path(p);
    GENERATED_MARKS.iter().any(|m| s.contains(m))
}

fn is_test_path(p: &Path) -> bool {
    let s = norm_path(p);
    TEST_MARKS.iter().any(|m| s.contains(m))
}

fn detect_license(root: &Path) -> Option<String> {
    let entries = std::fs::read_dir(root).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_uppercase();
        if name.starts_with("LICENSE") || name.starts_with("COPYING") || name.starts_with("NOTICE") {
            let text = read_head(&e.path(), 4096).to_uppercase();
            for k in ["MIT", "APACHE", "GPL", "BSD", "MPL", "UNLICENSE", "PROPRIETARY", "ISC"] {
                if text.contains(k) {
                    return Some(k.to_string());
                }
            }
            return Some("UNKNOWN".to_string());
        }
    }
    None
}

fn parse_imports(head: &str, lang: &str) -> (Vec<String>, bool) {
    let known = matches!(lang, "rust" | "python" | "javascript" | "typescript" | "go" | "java" | "kotlin" | "scala" | "c" | "cpp" | "cuda" | "opencl" | "csharp");
    let mut out = Vec::new();
    for line in head.lines() {
        let t = line.trim();
        let imp = match lang {
            "rust" => t
                .strip_prefix("use ")
                .map(|s| s.trim_end_matches(';').to_string()),
            "python" => t
                .strip_prefix("import ")
                .or_else(|| t.strip_prefix("from "))
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string()),
            "javascript" | "typescript" => t
                .strip_prefix("import ")
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
                }),
            "go" => t
                .strip_prefix("import ")
                .map(|s| s.trim_matches('"').to_string()),
            "java" | "kotlin" | "scala" => t
                .strip_prefix("import ")
                .map(|s| s.trim_end_matches(';').to_string()),
            "c" | "cpp" | "cuda" | "opencl" => t.strip_prefix("#include ").map(|s| {
                s.trim_matches(|c| c == '<' || c == '>' || c == '"')
                    .to_string()
            }),
            "csharp" => t
                .strip_prefix("using ")
                .map(|s| s.trim_end_matches(';').to_string()),
            _ => None,
        };
        if let Some(i) = imp
            && !i.is_empty()
            && !out.contains(&i)
            && out.len() < 50
        {
            out.push(i);
        }
    }
    (out, known)
}

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;
