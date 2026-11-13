//! `KeepReport::from_verdicts`：pair 判定的证书组装与截断。

use trit_wspace::certify::{KeepReport, Verdict};

#[test]
fn from_verdicts_classifies_pair_judgements() {
    // want:   [T, F, T, F]
    // before: [T, F, T, F]（全对）
    // after:  [T, T, F, F]（第 2、3 个被破坏）
    let report = KeepReport::from_verdicts(
        &[true, false, true, false],
        &[true, false, true, false],
        &[true, true, false, false],
        8,
    );
    assert_eq!(
        (report.samples, report.before_loss, report.after_loss),
        (4, 0, 2)
    );
    assert_eq!((report.preserved, report.regressed), (2, 2));
    assert_eq!((report.fixed, report.drifted, report.stale), (0, 0, 0));
    assert_eq!(report.rows.len(), 2);
    assert_eq!(report.rows[0].verdict, Verdict::Regressed);
    assert!(report.rows[0].input.is_empty(), "pair 判定没有单输入");
    assert_eq!(report.violations(), 2);

    // 截断：只留 1 条明细。
    let truncated = KeepReport::from_verdicts(&[true, true], &[true, true], &[false, false], 1);
    assert_eq!(
        (
            truncated.regressed,
            truncated.rows.len(),
            truncated.truncated
        ),
        (2, 1, true)
    );
}
