//! 路径与文件标记：源码扩展名、相对路径、生成/测试标记。

use std::path::Path;

pub const EXTS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "h", "cc", "cpp", "hpp", "cs", "php",
    "rb", "swift", "kt", "scala", "sql", "sh", "bash", "zsh", "fish", "ps1", "psm1", "lua", "r",
    "jl", "hs", "ex", "exs", "erl", "ml", "lean", "glsl", "hlsl", "wgsl", "cu", "cl", "metal",
    "asm", "s", "v", "sv", "vhd", "vhdl", "md", "markdown", "toml", "yaml", "yml", "tex", "rst",
    "proto", "graphql",
];

pub fn is_source(p: &Path) -> bool {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    if name == "Dockerfile" || name == "Makefile" {
        return true;
    }
    p.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| EXTS.contains(&x.to_lowercase().as_str()))
}

pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

const GENERATED_MARKS: &[&str] = &["/generated/", "_generated.", ".gen.", ".pb.go", "_pb2.py"];
const TEST_MARKS: &[&str] = &["/test/", "/tests/", "_test.", ".test.", ".spec.", "/spec/"];

pub fn is_generated_path(p: &Path) -> bool {
    let s = norm_path(p);
    GENERATED_MARKS.iter().any(|m| s.contains(m))
}

pub fn is_test_path(p: &Path) -> bool {
    let s = norm_path(p);
    TEST_MARKS.iter().any(|m| s.contains(m))
}

fn norm_path(p: &Path) -> String {
    p.to_string_lossy().to_lowercase().replace('\\', "/")
}
