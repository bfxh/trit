//! 复核报告：一条检查一行，失败留首个反例；JSON 与文本两种出口。
//!
//! 输出实现分在 `report_text.rs` / `report_json.rs`，本文件只管计数与数据。

#[path = "report_json.rs"]
mod report_json;
#[path = "report_text.rs"]
mod report_text;

/// 单条检查的计数。`case` 只在失败时构造反例描述，正常路径零分配。
pub struct Check {
    pub name: &'static str,
    pub cases: u64,
    pub failed: u64,
    first: Option<String>,
}

impl Check {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            cases: 0,
            failed: 0,
            first: None,
        }
    }

    pub fn case(&mut self, ok: bool, detail: impl FnOnce() -> String) {
        self.cases += 1;
        if !ok {
            self.failed += 1;
            if self.first.is_none() {
                self.first = Some(detail());
            }
        }
    }

    pub fn note(&self) -> String {
        match &self.first {
            Some(text) => format!("首个反例: {text}"),
            None => String::new(),
        }
    }
}

pub struct Report {
    pub kind: &'static str,
    pub title: &'static str,
    pub seed: u64,
    pub quick: bool,
    pub samples: usize,
    pub trits: usize,
    pub checks: Vec<Check>,
    /// 聚合读数：检查管“是否成立”，读数管“值是多少”。
    pub metrics: Vec<(String, String)>,
}

impl Report {
    pub fn new(kind: &'static str, title: &'static str, seed: u64, quick: bool) -> Self {
        Self {
            kind,
            title,
            seed,
            quick,
            samples: 0,
            trits: 0,
            checks: Vec::new(),
            metrics: Vec::new(),
        }
    }

    pub fn finish(&mut self, check: Check) {
        self.checks.push(check);
    }

    /// 记一条读数；值按 `Display` 原样进报告（数字保持可解析）。
    pub fn metric(&mut self, name: impl Into<String>, value: impl std::fmt::Display) {
        self.metrics.push((name.into(), value.to_string()));
    }

    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.failed == 0)
    }

    pub fn totals(&self) -> (u64, u64) {
        self.checks.iter().fold((0, 0), |(cases, failed), check| {
            (cases + check.cases, failed + check.failed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_report_carries_every_check_and_a_verdict() {
        let mut report = Report::new("k", "t", 1, true);
        let mut check = Check::new("demo.check");
        check.case(true, || unreachable!("成功路径不该构造反例"));
        check.case(false, || "第一个失败".to_string());
        report.finish(check);
        report.samples = 1;
        report.trits = 5;
        report.metric("demo.rate", 0.5);
        let text = report.to_text();
        assert!(text.contains("demo.check"));
        assert!(text.contains("首个反例: 第一个失败"));
        assert!(text.contains("demo.rate = 0.5"));
        assert!(text.contains("结论：红"));
    }

    #[test]
    fn json_escapes_quotes_and_backslashes() {
        let mut report = Report::new("k", "t", 2, false);
        let mut check = Check::new("demo.check");
        check.case(false, || "对 \"x\" 与 \\n".to_string());
        report.finish(check);
        report.metric("demo.rate", 0.5);
        let json = report.to_json();
        assert!(json.contains("\\\"x\\\""));
        assert!(json.contains("\\\\n"));
        assert!(json.contains("\"demo.rate\":0.5"));
        assert!(json.contains("\"ok\":false"));
    }
}
