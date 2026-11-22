//! 独立复核器的骨架：检查表 + 裁决写盘（路径与互斥见 [`paths`]）。
//!
//! 族特有逻辑（重跑导出、逐档比对）留在各复核器；这里只放每个复核器都必须一致的
//! 那几件事——否则「PASS 与 reject 同时存在」「记录写到别的目录」这类漂移会各写一遍。

mod paths;

pub use paths::{publish, publish_base, sibling_exe, verdict_paths};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check(mut self, name: &'static str, ok: bool, detail: impl Into<String>) -> Self {
        self.checks.push(Check {
            name,
            ok,
            detail: detail.into(),
        });
        self
    }

    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }

    pub fn failed(&self) -> Vec<String> {
        self.checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| {
                if c.detail.is_empty() {
                    c.name.to_string()
                } else {
                    format!("{}({})", c.name, c.detail)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_all_must_pass() {
        let ok = Report::new()
            .check("model", true, "")
            .check("data", true, "6 文件");
        assert!(ok.ok());
        assert!(ok.failed().is_empty());
        let bad = ok.check("levels", false, "1/3");
        assert!(!bad.ok());
        assert_eq!(bad.failed(), vec!["levels(1/3)".to_string()]);
    }
}
