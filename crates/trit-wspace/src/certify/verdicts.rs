//! 逐样本判定的证书报告（从 `certify.rs` 拆出，控制单文件行数）。

use super::{KeepReport, SampleRow, Verdict};
use crate::trit::Trit;

impl KeepReport {
    /// 从逐样本判定构造证书报告（pair 判定等非单输出任务用）。
    ///
    /// `want` / `before` / `after` 是同一批样本的期望与编辑前后判定，长度取三者最小值；
    /// 明细的 `input` 留空（pair 判定没有单输入），`want` / `before` / `after` 用单元素三值表示。
    #[must_use]
    pub fn from_verdicts(want: &[bool], before: &[bool], after: &[bool], rows: usize) -> Self {
        let count = want.len().min(before.len()).min(after.len());
        let mut report = Self {
            samples: count,
            before_loss: 0,
            after_loss: 0,
            preserved: 0,
            fixed: 0,
            regressed: 0,
            drifted: 0,
            stale: 0,
            rows: Vec::new(),
            truncated: false,
        };
        let flag = |value: bool| vec![if value { Trit::Pos } else { Trit::Neg }];
        for index in 0..count {
            let want_value = want[index];
            let before_ok = before[index] == want_value;
            let after_ok = after[index] == want_value;
            if !before_ok {
                report.before_loss += 1;
            }
            if !after_ok {
                report.after_loss += 1;
            }
            let verdict = Verdict::classify(before_ok, after_ok, before[index] == after[index]);
            match verdict {
                Verdict::Preserved => report.preserved += 1,
                Verdict::Fixed => report.fixed += 1,
                Verdict::Regressed => report.regressed += 1,
                Verdict::Drifted => report.drifted += 1,
                Verdict::Stale => report.stale += 1,
            }
            if verdict == Verdict::Preserved {
                continue;
            }
            if report.rows.len() < rows {
                report.rows.push(SampleRow {
                    sample: index,
                    verdict,
                    input: Vec::new(),
                    want: flag(want_value),
                    before: flag(before[index]),
                    after: flag(after[index]),
                });
            } else {
                report.truncated = true;
            }
        }
        report
    }
}
