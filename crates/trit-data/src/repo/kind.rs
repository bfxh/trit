//! 未解析 import 的分类：外部依赖（预期）还是本地解析失败（噪声）。

use std::collections::BTreeSet;

pub enum Kind {
    External,
    Unresolved,
}

pub fn classify(lang: &str, imp: &str, paths: &BTreeSet<String>) -> Kind {
    match lang {
        "rust" => rust(imp),
        "python" => {
            if imp.starts_with('.') || dir_in_repo(&imp.replace('.', "/"), paths) {
                Kind::Unresolved
            } else {
                Kind::External
            }
        }
        "javascript" | "typescript" => {
            if imp.starts_with('.') {
                Kind::Unresolved
            } else {
                Kind::External
            }
        }
        "java" | "kotlin" | "scala" => {
            if ["java.", "javax.", "kotlin.", "scala."]
                .iter()
                .any(|p| imp.starts_with(p))
            {
                Kind::External
            } else {
                local_or_external(imp, paths)
            }
        }
        "go" | "csharp" => local_or_external(imp, paths),
        _ => Kind::Unresolved,
    }
}

fn rust(imp: &str) -> Kind {
    let local = ["crate::", "self::", "super::"]
        .iter()
        .any(|p| imp.starts_with(p));
    if local || !imp.contains("::") {
        Kind::Unresolved
    } else {
        Kind::External
    }
}

fn local_or_external(imp: &str, paths: &BTreeSet<String>) -> Kind {
    if dir_in_repo(imp, paths) {
        Kind::Unresolved
    } else {
        Kind::External
    }
}

/// 首段是否在仓库里作为目录出现（`pkg` → 存在 `pkg/...` 文件）。
fn dir_in_repo(p: &str, paths: &BTreeSet<String>) -> bool {
    let first = p.split('/').next().unwrap_or("");
    if first.is_empty() || first == "." || first == ".." {
        return false;
    }
    let head = format!("{first}/");
    paths.iter().any(|x| x.starts_with(&head))
}
