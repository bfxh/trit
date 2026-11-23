//! 复核报告的文本出口（`Report::to_text`）。

use super::Report;

impl Report {
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
        if !self.metrics.is_empty() {
            out.push_str("读数：\n");
            for (name, value) in &self.metrics {
                out.push_str(&format!("{name} = {value}\n"));
            }
        }
        let (cases, failed) = self.totals();
        let verdict = if self.ok() { "绿" } else { "红" };
        out.push_str(&format!(
            "结论：{verdict}（{} 项检查 / {cases} 例 / {failed} 失败）\n",
            self.checks.len()
        ));
        out
    }
}
