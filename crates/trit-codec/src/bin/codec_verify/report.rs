//! 复核报告：一条检查一行，失败留首个反例；JSON 与文本两种出口。

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
        }
    }

    pub fn finish(&mut self, check: Check) {
        self.checks.push(check);
    }

    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.failed == 0)
    }

    pub fn totals(&self) -> (u64, u64) {
        self.checks.iter().fold((0, 0), |(cases, failed), check| {
            (cases + check.cases, failed + check.failed)
        })
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("{}\n", self.title));
        out.push_str(&format!(
            "kind={} seed=0x{:016x} quick={} 语料={} 样本 / {} trits\n",
            self.kind, self.seed, self.quick, self.samples, self.trits
        ));
        out.push_str(&format!(
            "{:<30} {:>10} {:>8}  {}\n",
            "check", "cases", "failed", "note"
        ));
        for check in &self.checks {
            out.push_str(&format!(
                "{:<30} {:>10} {:>8}  {}\n",
                check.name,
                check.cases,
                check.failed,
                check.note()
            ));
        }
        let (cases, failed) = self.totals();
        let verdict = if self.ok() { "绿" } else { "红" };
        out.push_str(&format!(
            "结论：{verdict}（{} 项检查 / {cases} 例 / {failed} 失败）\n",
            self.checks.len()
        ));
        out
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{{\"schema\":1,\"kind\":\"{}\",\"seed\":\"0x{:016x}\",\"quick\":{},\"corpus\":{{\"samples\":{},\"trits\":{}}},\"checks\":[",
            self.kind, self.seed, self.quick, self.samples, self.trits
        ));
        for (index, check) in self.checks.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"name\":\"{}\",\"cases\":{},\"failed\":{},\"first_failure\":",
                check.name, check.cases, check.failed
            ));
            match &check.first {
                Some(text) => out.push_str(&format!("\"{}\"", escape(text))),
                None => out.push_str("null"),
            }
            out.push('}');
        }
        let (cases, failed) = self.totals();
        out.push_str(&format!(
            "],\"totals\":{{\"cases\":{cases},\"failed\":{failed}}},\"ok\":{}}}\n",
            self.ok()
        ));
        out
    }
}

/// 只转义 JSON 必需字符；报告里的反例都是 ASCII 字母数字加少量标点。
fn escape(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
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
        let text = report.to_text();
        assert!(text.contains("demo.check"));
        assert!(text.contains("首个反例: 第一个失败"));
        assert!(text.contains("结论：红"));
    }

    #[test]
    fn json_escapes_quotes_and_backslashes() {
        let mut report = Report::new("k", "t", 2, false);
        let mut check = Check::new("demo.check");
        check.case(false, || "对 \"x\" 与 \\n".to_string());
        report.finish(check);
        let json = report.to_json();
        assert!(json.contains("\\\"x\\\""));
        assert!(json.contains("\\\\n"));
        assert!(json.contains("\"ok\":false"));
    }
}
