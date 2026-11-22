//! 仓库扫描：源码树 → 文件级 `RepoUnit`（路径 / 语言 / 大小 / 生成 / 许可证 / 依赖 / 测试）。

use std::path::Path;

use crate::code_quality::check;

pub mod deps;
mod deps_report;
mod imports;
mod imports_lang;
mod imports_rust;
mod kind;
mod lang_map;
mod license;
pub mod paths;
mod paths_targets;

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
    pub imports_external: Vec<String>,
    pub imports_dynamic: Vec<String>,
    pub imports_resolved: Vec<String>,
    pub unresolved: Vec<String>,
    pub tests: Vec<String>,
    pub tests_for: Vec<String>,
    pub is_test: bool,
}

pub fn scan(root: &Path) -> Result<Vec<Unit>, String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let repo = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    let license = license::detect(&root);
    let mut out = Vec::new();
    walk(&root, &root, &repo, license.as_deref(), &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    deps::annotate(&mut out);
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
            let lang = lang_map::detect(&p, &head);
            let q = check(&head);
            let generated = q.auto_generated || is_generated_path(&p);
            let parsed = imports::parse(&head, &lang);
            let is_test = is_test_path(&p);
            out.push(Unit {
                repo: repo.to_string(),
                path: rel(root, &p),
                lang,
                bytes,
                generated,
                license: license.map(|s| s.to_string()),
                imports: parsed.imports,
                imports_known: parsed.known,
                imports_external: Vec::new(),
                imports_dynamic: parsed.dynamic,
                imports_resolved: Vec::new(),
                unresolved: Vec::new(),
                tests: Vec::new(),
                tests_for: Vec::new(),
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

pub(crate) fn read_head(p: &Path, n: usize) -> String {
    let data = std::fs::read(p).unwrap_or_default();
    String::from_utf8_lossy(&data[..data.len().min(n)]).into_owned()
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

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;

#[cfg(test)]
mod deps_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod imports_tests;
#[cfg(test)]
mod paths_tests;
