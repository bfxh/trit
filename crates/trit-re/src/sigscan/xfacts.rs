//! 串事实（接缝实现）：`$a` / `#a` / `@a` / `!a` 与位置性条件。
//!
//! 三态按 `docs/YARA-EXPR/01-SEMANTICS.md` §7.1：命中的能证明 1；完整扫描
//! 未命中才能证明 0；不完整扫描（截断/封顶/预算）未命中 → `Unknown`。

use super::CmpOp;
use super::xval::{Val, int};

/// 一条串的扫描事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrFact {
    /// 命中偏移（升序；可能被 `MAX_HITS` 截断）。
    pub hits: Vec<u64>,
    /// 命中表是否完整（未被 `MAX_HITS` 截断）。
    pub exact: bool,
    /// 定长模式的长度；变长模式为 `None`。
    pub fixed_len: Option<u64>,
    /// 输入是否完整（未截断、预算未耗尽）。
    pub complete: bool,
}

impl StrFact {
    pub fn found(&self) -> Val {
        if !self.hits.is_empty() {
            Val::Int(1)
        } else if self.exact && self.complete {
            Val::Int(0)
        } else {
            Val::Unknown
        }
    }

    pub fn found_at(&self, at: Val) -> Val {
        let Val::Int(x) = at else {
            return match at {
                Val::Undef => Val::Undef,
                _ => Val::Unknown,
            };
        };
        if x >= 0 && self.hits.contains(&(x as u64)) {
            return Val::Int(1);
        }
        if self.exact && self.complete {
            Val::Int(0)
        } else {
            Val::Unknown
        }
    }

    pub fn found_in(&self, lo: Val, hi: Val) -> Val {
        let (Val::Int(a), Val::Int(b)) = (lo, hi) else {
            return Val::Unknown;
        };
        if a <= b && self.hits.iter().any(|o| *o as i64 >= a && *o as i64 <= b) {
            return Val::Int(1);
        }
        if self.exact && self.complete {
            Val::Int(0)
        } else {
            Val::Unknown
        }
    }

    pub fn count(&self) -> Val {
        if self.exact {
            int(self.hits.len() as i64)
        } else {
            Val::Unknown
        }
    }

    pub fn count_in(&self, lo: Val, hi: Val) -> Val {
        let (Val::Int(a), Val::Int(b)) = (lo, hi) else {
            return Val::Unknown;
        };
        if !self.exact {
            return Val::Unknown;
        }
        let n = self
            .hits
            .iter()
            .filter(|o| **o as i64 >= a && **o as i64 <= b)
            .count();
        int(n as i64)
    }

    /// `@a[i]`：下标 1 起。
    pub fn offset(&self, nth: Val) -> Val {
        let Val::Int(i) = nth else {
            return Val::Unknown;
        };
        if i <= 0 {
            return Val::Undef;
        }
        match self.hits.get((i - 1) as usize) {
            Some(v) => int(*v as i64),
            None if self.exact && self.complete => Val::Undef,
            None => Val::Unknown,
        }
    }

    /// `!a[i]`：下标 1 起。
    pub fn length(&self, nth: Val) -> Val {
        let Val::Int(i) = nth else {
            return Val::Unknown;
        };
        if i <= 0 {
            return Val::Undef;
        }
        if (i as usize) <= self.hits.len() {
            return match self.fixed_len {
                Some(n) => int(n as i64),
                None => Val::Unknown,
            };
        }
        if self.exact && self.complete {
            Val::Undef
        } else {
            Val::Unknown
        }
    }

    /// `#a <比较> 常数`：不完整时能由下界/上界判定就照判。
    #[allow(dead_code)] // P1 计数比较接入后删除
    pub fn count_vs(&self, op: CmpOp, n: i64) -> Val {
        let len = self.hits.len() as i64;
        if self.exact {
            return int(i64::from(compare(op, len, n)));
        }
        // 不完整：命中表只给下界。
        let decided = match op {
            CmpOp::Ge | CmpOp::Gt => Some(len >= n + i64::from(op == CmpOp::Gt)),
            CmpOp::Le => Some(len == 0 && n >= 0),
            CmpOp::Lt => Some(len == 0 && n > 0),
            CmpOp::Eq => (n == 0 && len == 0).then_some(false),
            CmpOp::Ne => (n == 0 && len == 0).then_some(false),
        };
        match decided {
            Some(true) => Val::Int(1),
            Some(false) => Val::Int(0),
            None => Val::Unknown,
        }
    }
}

fn compare(op: CmpOp, a: i64, b: i64) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    }
}
