//! 扩展名 → 语言名；未知扩展名回退到内容特征（`code_languages`）。

use std::path::Path;

use crate::code_langs::code_languages;
pub fn detect(p: &Path, head: &str) -> String {
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