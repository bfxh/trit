//! 扫描不完整时的定案规则。命中表不完整（输入截断、某条串的扫描预算
//! 耗尽、命中数封顶）只会**少报**命中，所以一个结论只有在"补全命中之后
//! 仍然成立"时才能落账，否则如实 `Unknown`：
//!
//! - 假 → `Unknown`（既有判据，原样保留：没扫到 ≠ 没有）。
//! - 真：条件对命中**单调递增**（`$a`、`#a > 3`、`any of them`）时成立；
//!   靠"没有"得来的真（`not $a`、`#a == 0`、`#a < 3`）补全后可能翻掉 →
//!   `Unknown`。这是本文件新增的收紧——与官方对拍实测：6.9 MB 样本里只在
//!   尾部出现一次的串，`not $a` / `#a == 0` 旧口径报 HIT，官方不命中。
//!
//! 表达式引擎（`Condition::Expr`）逐串带着"是否完整"求值（见
//! `expr/eval.rs` 的 `hits`），它给出的结论已经是补全后仍成立的，不再打折。

use super::super::{CmpOp, Condition, Operand};
use trit_core::value::Ternary;

/// `truncated` = 输入本身不完整；`cut_short` = 有串的扫描没跑完。
pub(super) fn settle(
    condition: &Condition,
    met: Ternary,
    truncated: bool,
    cut_short: bool,
) -> Ternary {
    if !(truncated || cut_short) || matches!(condition, Condition::Expr(_)) {
        return met;
    }
    match met {
        Ternary::True if tone(condition, truncated).rising => Ternary::True,
        _ => Ternary::Unknown,
    }
}

/// 条件对"命中变多"的单调性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tone {
    /// 命中变多只会让它从假变真（真是稳的）。
    rising: bool,
    /// 命中变多只会让它从真变假（假是稳的）。
    falling: bool,
}

const RISING: Tone = Tone {
    rising: true,
    falling: false,
};
const NEITHER: Tone = Tone {
    rising: false,
    falling: false,
};

fn tone(condition: &Condition, truncated: bool) -> Tone {
    match condition {
        Condition::Const(_) => Tone {
            rising: true,
            falling: true,
        },
        Condition::Cmp { lhs, op, rhs } => compare(lhs, *op, rhs, truncated),
        Condition::Not(inner) => {
            let inner = tone(inner, truncated);
            Tone {
                rising: inner.falling,
                falling: inner.rising,
            }
        }
        Condition::And(items) | Condition::Or(items) => {
            items.iter().map(|item| tone(item, truncated)).fold(
                Tone {
                    rising: true,
                    falling: true,
                },
                |all, item| Tone {
                    rising: all.rising && item.rising,
                    falling: all.falling && item.falling,
                },
            )
        }
        // 量词只数"体为真的串有几条"：体单调，整体就同向单调
        Condition::ForOf { body, .. } => tone(body, truncated),
        // 表达式引擎自己处理（`settle` 已提前返回）；保守起见按"都不稳"
        Condition::Expr(_) => NEITHER,
        // `$a at #b`：位置跟着计数走，补全后可能挪开
        Condition::At { offset, .. } | Condition::CurrentAt { offset }
            if matches!(offset, Operand::Count(_) | Operand::CurrentCount) =>
        {
            NEITHER
        }
        // 其余全是"有命中 / 命中在某处 / 至少 n 条"：只增不减
        _ => RISING,
    }
}

fn compare(lhs: &Operand, op: CmpOp, rhs: &Operand, truncated: bool) -> Tone {
    let counts = |operand: &Operand| matches!(operand, Operand::Count(_) | Operand::CurrentCount);
    let offsets = |operand: &Operand| {
        matches!(
            operand,
            Operand::Offset { .. } | Operand::CurrentOffset { .. }
        )
    };
    match (counts(lhs), counts(rhs)) {
        // `#a op 常量`：计数只增
        (true, false) if !offsets(rhs) => match op {
            CmpOp::Gt | CmpOp::Ge => RISING,
            CmpOp::Lt | CmpOp::Le => Tone {
                rising: false,
                falling: true,
            },
            CmpOp::Eq | CmpOp::Ne => NEITHER,
        },
        (false, true) if !offsets(lhs) => match op {
            CmpOp::Lt | CmpOp::Le => RISING,
            CmpOp::Gt | CmpOp::Ge => Tone {
                rising: false,
                falling: true,
            },
            CmpOp::Eq | CmpOp::Ne => NEITHER,
        },
        (true, _) | (_, true) => NEITHER,
        // `@a[n] op X`：已有的第 n 次命中不会变；现在没有的将来可能有
        (false, false) if offsets(lhs) || offsets(rhs) => RISING,
        // 与命中无关的比较（`filesize`、`uintN`、模块字段）：扫描预算不影响
        // 它；输入截断时读不到的将来可能读到——真稳、假不稳
        (false, false) => Tone {
            rising: true,
            falling: !truncated,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(op: CmpOp, n: u64) -> Condition {
        Condition::Cmp {
            lhs: Operand::Count(0),
            op,
            rhs: Operand::Lit(n),
        }
    }

    fn not(inner: Condition) -> Condition {
        Condition::Not(Box::new(inner))
    }

    /// 不完整扫描上的"真"落成什么。
    fn kept(condition: &Condition) -> bool {
        // 假一律弃权（既有判据）
        assert_eq!(
            settle(condition, Ternary::False, false, true),
            Ternary::Unknown
        );
        assert_eq!(
            settle(condition, Ternary::Unknown, false, true),
            Ternary::Unknown
        );
        settle(condition, Ternary::True, false, true) == Ternary::True
    }

    #[test]
    fn complete_scans_are_never_discounted() {
        for verdict in [Ternary::True, Ternary::False, Ternary::Unknown] {
            assert_eq!(
                settle(&not(Condition::Ref(0)), verdict, false, false),
                verdict
            );
        }
    }

    #[test]
    fn only_presence_survives_an_incomplete_scan() {
        let each = |op: CmpOp| Condition::ForOf {
            bound: crate::sigscan::ForBound::All,
            prefix: None,
            body: Box::new(Condition::Cmp {
                lhs: Operand::CurrentCount,
                op,
                rhs: Operand::Lit(0),
            }),
        };
        for (condition, expected) in [
            (Condition::Ref(0), true),
            (Condition::Any, true),
            (Condition::OfAll { prefix: None }, true),
            (
                Condition::In {
                    index: 0,
                    lo: 0,
                    hi: 9,
                },
                true,
            ),
            (
                Condition::At {
                    index: 0,
                    offset: Operand::Lit(4),
                },
                true,
            ),
            (
                Condition::At {
                    index: 0,
                    offset: Operand::Count(1),
                },
                false,
            ),
            (not(Condition::Ref(0)), false),
            (not(not(Condition::Ref(0))), true),
            (count(CmpOp::Gt, 3), true),
            (count(CmpOp::Ge, 1), true),
            (count(CmpOp::Lt, 3), false),
            (count(CmpOp::Eq, 0), false),
            (count(CmpOp::Ne, 0), false),
            (not(count(CmpOp::Gt, 3)), false),
            (not(count(CmpOp::Lt, 3)), true),
            (Condition::Const(true), true),
            (not(Condition::Const(false)), true),
            (
                Condition::And(vec![Condition::Ref(0), count(CmpOp::Gt, 1)]),
                true,
            ),
            (
                Condition::And(vec![Condition::Ref(0), not(Condition::Ref(1))]),
                false,
            ),
            (
                Condition::Or(vec![Condition::Ref(0), not(Condition::Ref(1))]),
                false,
            ),
            (each(CmpOp::Gt), true),
            (each(CmpOp::Eq), false),
        ] {
            assert_eq!(kept(&condition), expected, "{condition:?}");
        }
    }

    #[test]
    fn truncation_also_unsettles_reads_past_the_end() {
        let read = Condition::Cmp {
            lhs: Operand::UintRead { size: 1, offset: 9 },
            op: CmpOp::Eq,
            rhs: Operand::Lit(1),
        };
        // 扫描预算与读取无关：真照旧
        assert!(kept(&read));
        assert!(kept(&not(read.clone())));
        // 输入截断：读到的是真的；读不到的不能当没有
        assert_eq!(settle(&read, Ternary::True, true, false), Ternary::True);
        assert_eq!(settle(&read, Ternary::False, true, false), Ternary::Unknown);
        assert_eq!(
            settle(&not(read), Ternary::True, true, false),
            Ternary::Unknown
        );
    }
}
