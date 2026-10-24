//! 发现归属：**是我们的问题，还是 Rust 工具链/链接器本身的问题**。
//!
//! 判据是**证据字符串**，不是猜测：
//! - 路径里带 `.rustup` / `/rustc/` / `library/core` ⇒ Rust 工具链带进来的；
//! - 硬化位缺失 ⇒ 由链接参数决定（我们能改）；
//! - PE + GNU 链接器要 `/GUARD:CF` ⇒ binutils 不支持，属工具链限制；
//! - 其余（格式设计、我们自己产出的字符串）⇒ 我们的。
//!
//! 限制：这是启发式。字符串证据只能说明"这段字节来自哪里"，不能说明"这个风险该谁负责"。

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 我们的代码/格式/构建配置，能直接改。
    Project,
    /// Rust 编译器 / std / rustup 带进来的，改不了，只能绕过（如 remap 路径）。
    RustToolchain,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::RustToolchain => "rust-toolchain",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub id: String,
    pub origin: Origin,
    /// 更细的位置标签（format-design / build-flags / std-paths / artifact-content …）。
    pub area: &'static str,
    pub hint: &'static str,
}

impl Finding {
    pub fn project(id: impl Into<String>, area: &'static str, hint: &'static str) -> Self {
        Self {
            id: id.into(),
            origin: Origin::Project,
            area,
            hint,
        }
    }

    pub fn rust(id: impl Into<String>, area: &'static str, hint: &'static str) -> Self {
        Self {
            id: id.into(),
            origin: Origin::RustToolchain,
            area,
            hint,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "origin": self.origin.as_str(),
            "area": self.area,
            "hint": self.hint,
        })
    }
}

pub fn list_to_json(findings: &[Finding]) -> Value {
    Value::Array(findings.iter().map(Finding::to_json).collect())
}

pub fn summary(findings: &[Finding]) -> Value {
    let mut project = 0usize;
    let mut rust = 0usize;
    for finding in findings {
        match finding.origin {
            Origin::Project => project += 1,
            Origin::RustToolchain => rust += 1,
        }
    }
    json!({ "project": project, "rust-toolchain": rust })
}

/// 一段路径字符串是 Rust 工具链带进来的吗？
pub fn rust_toolchain_path(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains(".rustup")
        || lower.contains("/rustc/")
        || lower.contains("library\\std\\")
        || lower.contains("library/std/")
        || lower.contains("library\\core\\")
        || lower.contains("library/core/")
        || lower.contains("library\\alloc\\")
        || lower.contains("library/alloc/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_rustup_and_rustc_paths_as_toolchain() {
        assert!(rust_toolchain_path(
            r"C:\Users\lbx13\.rustup\toolchains\1.99.0\lib\rustlib\src\rust\library\core\src\cell\once.rs"
        ));
        assert!(rust_toolchain_path(
            "/rustc/b940084d/library/core/src/cell/once.rs"
        ));
        assert!(!rust_toolchain_path(
            r"D:\KF\trit\target\release\trit-build.exe"
        ));
        assert!(!rust_toolchain_path(r"C:\Users\lbx13\Documents\out.exe"));
    }

    #[test]
    fn summary_counts_by_origin() {
        let findings = vec![
            Finding::project("a", "x", "h"),
            Finding::rust("b", "y", "h"),
            Finding::project("c", "z", "h"),
        ];
        let summary = summary(&findings);
        assert_eq!(summary["project"], 2);
        assert_eq!(summary["rust-toolchain"], 1);
    }
}
