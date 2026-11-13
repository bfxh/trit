//! 表达式求值核心：四态值域 + 严格算子的传染规则 + Kleene `and`/`or`。
//! 命中表的可信度口径与既有 `resolve.rs` 一致：截断输入 / 命中封顶
//! （[`MAX_HITS`]）时"没找到"与"数量"都不可信 → 弃权；"找到了"是单调
//! 正证据，照常判真。
use super::node::PeLazy;
use super::{Expr, Rel, Sid, num, strfunc};
use crate::sigscan::MAX_HITS;
use crate::sigscan::eval::EvalCtx;
/// 运行期值。`Undef` = 官方的 undefined（确定的）；`Unknown` = 弃权。
#[derive(Debug, Clone)]
pub(super) enum V<'a> {
    Int(i64),
    Str(Vec<u8>),
    Bool(bool),
    Obj(&'a super::node::Node),
    Undef,
    Unknown,
}
/// 布尔上下文转换：整数非零、字符串非空；未定义与弃权原样透传。
pub(super) fn truth(value: V) -> V {
    match value {
        V::Int(i) => V::Bool(i != 0),
        V::Str(s) => V::Bool(!s.is_empty()),
        V::Obj(_) => V::Unknown,
        other => other,
    }
}
/// 严格算子的失败归宿：任一侧确定未定义 → 未定义（另一侧是什么都救不
/// 回来，所以压过弃权）；否则弃权。
pub(super) fn strict_fail<'a>(a: &V, b: &V) -> V<'a> {
    if matches!(a, V::Undef) || matches!(b, V::Undef) {
        V::Undef
    } else {
        V::Unknown
    }
}
/// 单条规则一次求值的总预算（单位 ≈ 一个表达式节点；函数调用与正则
/// 另按工作量加收）。耗尽 = 弃权——官方会真的跑完，这里不陪跑。
/// 2000 万大约对应"逐字节循环扫 4 MB"。
pub(super) const FUEL: u64 = 20_000_000;

pub(super) struct Env<'a> {
    pub(super) occ: &'a [(String, Vec<u64>)],
    pub(super) data: &'a [u8],
    pub(super) truncated: bool,
    entrypoint: Option<u64>,
    pub(super) pe: &'a PeLazy<'a>,
    pub(super) slots: Vec<V<'a>>,
    pub(super) current: Option<usize>,
    pub(super) fuel: u64,
    cut_short: &'a [bool],
}

type Step<'a> = Result<V<'a>, V<'a>>;

/// 一条串的命中偏移表（升序）与完整性。
#[derive(Clone, Copy)]
pub(super) struct Hits<'a> {
    at: &'a [u64],
    partial: bool,
}

impl std::ops::Deref for Hits<'_> {
    type Target = [u64];
    fn deref(&self) -> &[u64] {
        self.at
    }
}

impl<'a> Env<'a> {
    pub(super) fn new(occ: &'a [(String, Vec<u64>)], ctx: &EvalCtx<'a, 'a>) -> Self {
        Self {
            occ,
            data: ctx.data,
            truncated: ctx.truncated,
            entrypoint: ctx.entrypoint,
            pe: ctx.tree,
            slots: Vec::new(),
            current: None,
            fuel: FUEL,
            cut_short: ctx.partial,
        }
    }

    /// 扣预算；`false` = 已耗尽（调用方弃权）。
    pub(super) fn burn(&mut self, cost: u64) -> bool {
        self.fuel = self.fuel.saturating_sub(cost);
        self.fuel > 0
    }

    /// 跑正则的一段：先把剩余预算换成正则步数（1 单位 = 4 步），跑完按
    /// 实际步数扣回。
    pub(super) fn metered<T>(&mut self, run: impl FnOnce(&Self) -> T) -> T {
        strfunc::set_allowance(self.fuel.saturating_mul(4));
        let out = run(self);
        self.fuel = self.fuel.min(strfunc::allowance() / 4);
        out
    }

    pub(super) fn eval(&mut self, expr: &Expr) -> V<'a> {
        if !self.burn(1) {
            return V::Unknown;
        }
        match self.step(expr).unwrap_or_else(|early| early) {
            V::Int(num::UNDEFINED) => V::Undef, // 官方哨兵值 = 未定义
            value => value,
        }
    }

    /// 整数操作数：非整数（未定义 / 弃权）经 `Err` 提前返回。
    pub(super) fn int(&mut self, expr: &Expr) -> Result<i64, V<'a>> {
        match self.eval(expr) {
            V::Int(i) => Ok(i),
            V::Undef => Err(V::Undef),
            _ => Err(V::Unknown),
        }
    }

    /// 两个整数操作数（两侧都求值——未定义压过弃权）。
    pub(super) fn ints(&mut self, a: &Expr, b: &Expr) -> Result<(i64, i64), V<'a>> {
        match (self.eval(a), self.eval(b)) {
            (V::Int(x), V::Int(y)) => Ok((x, y)),
            (x, y) => Err(strict_fail(&x, &y)),
        }
    }

    /// 命中表 + 它是否可能不完整（输入截断 / 命中封顶 / 该串的扫描预算
    /// 耗尽）：不完整时"没有" / "只有这么多"都不能下结论，已找到的照算。
    pub(super) fn hits(&self, sid: Sid) -> Result<Hits<'a>, V<'a>> {
        let index = match sid {
            Sid::Named(index) => Some(index),
            Sid::Current => self.current,
        };
        let index = index.ok_or(V::Unknown)?;
        let (_, offsets) = self.occ.get(index).ok_or(V::Unknown)?;
        let cut_short = self.cut_short.get(index).copied().unwrap_or(true);
        Ok(Hits {
            at: offsets,
            partial: self.truncated || cut_short || offsets.len() >= MAX_HITS,
        })
    }

    fn absent(&self, hits: Hits<'a>) -> V<'a> {
        if hits.partial {
            V::Unknown
        } else {
            V::Bool(false)
        }
    }

    pub(super) fn found(&self, sid: Sid) -> Step<'a> {
        let hits = self.hits(sid)?;
        Ok(if hits.is_empty() {
            self.absent(hits)
        } else {
            V::Bool(true)
        })
    }

    pub(super) fn found_at(&self, sid: Sid, at: i64) -> Step<'a> {
        let hits = self.hits(sid)?;
        Ok(match u64::try_from(at) {
            Err(_) => V::Bool(false), // 负偏移：不可能有命中
            Ok(at) if hits.contains(&at) => V::Bool(true),
            Ok(_) => self.absent(hits),
        })
    }

    pub(super) fn found_in(&self, sid: Sid, lo: i64, hi: i64) -> Step<'a> {
        let hits = self.hits(sid)?;
        Ok(if lo > hi {
            V::Bool(false) // 动态空区间 = 假（常量空区间是编译错误）
        } else if hits.iter().any(|h| in_range(*h, lo, hi)) {
            V::Bool(true)
        } else {
            self.absent(hits)
        })
    }

    /// 求值按"帧薄的在递归脊柱上"分三段：算术链（`1 + 1 + …` 能把树长到
    /// 深度上限）→ 逻辑算子（括号每层都过）→ 其余。全写在一个 `match` 里
    /// 时 debug 构建一帧 4 KiB，256 层树就要 1 MiB 栈。
    fn step(&mut self, expr: &Expr) -> Step<'a> {
        Ok(match expr {
            Expr::Int(v) => V::Int(*v),
            Expr::Neg(inner) => V::Int(self.int(inner)?.wrapping_neg()),
            Expr::BitNot(inner) => V::Int(!self.int(inner)?),
            Expr::Arith { op, lhs, rhs } => {
                let (a, b) = self.ints(lhs, rhs)?;
                num::arith(*op, a, b)
            }
            // 下标 / 实参 / 读取偏移里还能再套同类节点，也算脊柱
            Expr::Path(path) => self.path(path),
            Expr::Read {
                size,
                signed,
                big_endian,
                at,
            } => {
                let at = self.int(at)?;
                if self.truncated {
                    V::Unknown
                } else {
                    num::read(self.data, at, *size, *signed, *big_endian)
                }
            }
            other => return self.logic(other),
        })
    }

    fn logic(&mut self, expr: &Expr) -> Step<'a> {
        Ok(match expr {
            Expr::Defined(inner) => match self.eval(inner) {
                V::Undef => V::Bool(false),
                V::Unknown => V::Unknown,
                _ => V::Bool(true),
            },
            // not：未定义仍未定义（探针：`not (uint8(越界) == 1)` 不命中）
            Expr::Not(inner) => match truth(self.eval(inner)) {
                V::Bool(b) => V::Bool(!b),
                other => other,
            },
            // and / or：未定义当假；结果恒有定义；弃权按 Kleene 传播
            Expr::And(items) => self.junction(items, false),
            Expr::Or(items) => self.junction(items, true),
            other => return self.leafy(other),
        })
    }

    /// `and`（`decisive = false`）/ `or`（`decisive = true`）：遇到决定性
    /// 的那个真值就短路。
    fn junction(&mut self, items: &[Expr], decisive: bool) -> V<'a> {
        let mut unknown = false;
        for item in items {
            match truth(self.eval(item)) {
                V::Unknown => unknown = true,
                V::Bool(true) if decisive => return V::Bool(true),
                V::Bool(true) => {}
                _ if decisive => {}
                _ => return V::Bool(false),
            }
        }
        if unknown {
            V::Unknown
        } else {
            V::Bool(!decisive)
        }
    }

    fn leafy(&mut self, expr: &Expr) -> Step<'a> {
        Ok(match expr {
            Expr::Int(v) => V::Int(*v),
            Expr::Str(s) => V::Str(s.clone()),
            Expr::Bool(b) => V::Bool(*b),
            Expr::Filesize if self.truncated => V::Unknown,
            Expr::Filesize => V::Int(self.data.len() as i64),
            Expr::Entrypoint => self.entrypoint.map_or(V::Unknown, |v| V::Int(v as i64)),
            Expr::Var(slot) => self.slots.get(*slot).cloned().unwrap_or(V::Unknown),
            Expr::Count(sid) => {
                let hits = self.hits(*sid)?;
                if hits.partial {
                    V::Unknown
                } else {
                    V::Int(hits.len() as i64)
                }
            }
            Expr::CountIn { sid, lo, hi } => {
                let (lo, hi) = self.ints(lo, hi)?;
                let hits = self.hits(*sid)?;
                if lo > hi {
                    V::Int(0)
                } else if hits.partial {
                    V::Unknown
                } else {
                    V::Int(hits.iter().filter(|h| in_range(**h, lo, hi)).count() as i64)
                }
            }
            Expr::Offset { sid, nth } => {
                let nth = match self.eval(nth) {
                    V::Int(n) => n,
                    V::Bool(b) => i64::from(b),
                    V::Unknown => return Ok(V::Unknown),
                    _ => return Ok(V::Undef),
                };
                let hits = self.hits(*sid)?;
                let slot = usize::try_from(nth).ok().and_then(|n| n.checked_sub(1));
                match slot.map(|n| hits.get(n)) {
                    None => V::Undef, // 下标 ≤ 0
                    Some(Some(offset)) => V::Int(*offset as i64),
                    Some(None) if hits.partial => V::Unknown,
                    Some(None) => V::Undef,
                }
            }
            Expr::Cmp { op, lhs, rhs } => {
                let (a, b) = (self.eval(lhs), self.eval(rhs));
                let order = match (&a, &b) {
                    (V::Int(x), V::Int(y)) => x.cmp(y),
                    (V::Str(x), V::Str(y)) => strfunc::compare(x, y),
                    _ => return Err(strict_fail(&a, &b)),
                };
                V::Bool(match op {
                    Rel::Eq => order.is_eq(),
                    Rel::Ne => order.is_ne(),
                    Rel::Lt => order.is_lt(),
                    Rel::Le => order.is_le(),
                    Rel::Gt => order.is_gt(),
                    Rel::Ge => order.is_ge(),
                })
            }
            Expr::StrOp { op, lhs, rhs } => match (self.eval(lhs), self.eval(rhs)) {
                (V::Str(hay), V::Str(needle)) => V::Bool(strfunc::apply(*op, &hay, &needle)),
                (a, b) => strict_fail(&a, &b),
            },
            Expr::Matches { lhs, re } => match self.eval(lhs) {
                V::Str(hay) => self.metered(|_| strfunc::matches(re, &hay)),
                V::Undef => V::Undef,
                _ => V::Unknown,
            },
            Expr::Found(sid) => self.found(*sid)?,
            Expr::FoundAt { sid, at } => {
                let at = self.int(at)?;
                self.found_at(*sid, at)?
            }
            Expr::FoundIn { sid, lo, hi } => {
                let (lo, hi) = self.ints(lo, hi)?;
                self.found_in(*sid, lo, hi)?
            }
            Expr::Of { quant, set, limit } => self.of(quant, set, limit)?,
            Expr::ForOf {
                quant,
                set,
                body,
                raw,
            } => self.for_of(quant, set, body, *raw),
            Expr::ForIn {
                quant,
                vars,
                iter,
                body,
                raw,
            } => self.for_in(quant, *vars, iter, body, *raw),
            // 前两段已经分走的变体到不了这里
            Expr::Neg(_) | Expr::BitNot(_) | Expr::Arith { .. } => V::Unknown,
            Expr::Path(_) | Expr::Read { .. } => V::Unknown,
            Expr::Defined(_) | Expr::Not(_) | Expr::And(_) | Expr::Or(_) => V::Unknown,
        })
    }
}

fn in_range(hit: u64, lo: i64, hi: i64) -> bool {
    i64::try_from(hit).is_ok_and(|h| lo <= h && h <= hi)
}
