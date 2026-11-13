//! 量词与迭代（探针 + 官方 `exec.c` 的 `OP_ITER_CONDITION` / `OP_ITER_END`
//! 定案）：
//! - **零次迭代 = 假**，对 `all` / `none` / `any` / `N` 一视同仁（官方
//!   `for all i in (空区间)` 是假，不是数学上的空真）。
//! - 量词：未定义 → `all`；`0` → `none`；其余 = "真"计数 ≥ N（负数只能
//!   是动态值——常量负数是编译错误——于是恒真）。
//! - 体是布尔 / 字符串类型：未定义计为"不真"。体弃权不折叠：只有已有
//!   证据让结论与弃权项无关时才给确定值，否则整个循环弃权。
//! - 体是**整数类型**（`raw`）：官方把体的原始整数值直接加进"真"计数，
//!   并按原始值决定是否提前收尾——`for 3 i in (5..9) : (i)` 第一轮就
//!   攒够 5 ≥ 3 而命中；`for any i in (0..1) : (i * 2 - 1)` 的 -1 与 1
//!   相消而不命中；`for all i in (2..2) : (i)` 因 2 ≠ 迭代数 1 而不命中。
//!   这里逐步照搬那台小状态机；体一旦弃权，整个循环弃权。
//! - 区间闭合；任一端未定义或动态 `lo > hi` = 零次迭代。

use super::eval::{Env, V, truth};
use super::node::{Node, to_v};
use super::num::UNDEFINED;
use super::{Expr, Iter, Limit, Quant, Sid};

/// 量词的运行期值（`All` = 官方的"未定义量词"）。
enum Need {
    All,
    N(i64),
    Unknown,
}

pub(super) struct Tally {
    need: Need,
    raw: bool,
    /// 原始模式下的"真"计数（可为负、可回绕）。
    sum: i64,
    yes: u64,
    no: u64,
    unknown: u64,
}

impl Tally {
    fn new(need: Need, raw: bool) -> Self {
        Self {
            need,
            raw,
            sum: 0,
            yes: 0,
            no: 0,
            unknown: 0,
        }
    }

    /// 记一轮；返回 `true` = 结论已定，可以收尾。
    fn push(&mut self, value: V) -> bool {
        if self.raw {
            return self.push_raw(value);
        }
        match truth(value) {
            V::Bool(true) => self.yes += 1,
            V::Unknown => self.unknown += 1,
            _ => self.no += 1,
        }
        match self.need {
            Need::All => self.no > 0,
            Need::N(0) => self.yes > 0,
            Need::N(n) => n < 0 || self.yes >= n.unsigned_abs(),
            Need::Unknown => false,
        }
    }

    /// 官方 `OP_ITER_CONDITION` + `OP_ADD_M` 的原样复刻。
    fn push_raw(&mut self, value: V) -> bool {
        let body = match value {
            V::Int(i) => Some(i),
            V::Bool(b) => Some(i64::from(b)),
            V::Undef => None,
            _ => {
                self.unknown += 1;
                return true;
            }
        };
        self.yes += 1; // 原始模式下 yes = 迭代数
        let last = body.unwrap_or(UNDEFINED);
        let go_on = match self.need {
            Need::All => last != 0,
            Need::N(0) => last != 1,
            Need::N(n) => self.sum.wrapping_add(last) < n,
            Need::Unknown => true,
        };
        self.sum = self.sum.wrapping_add(body.unwrap_or(0));
        !go_on
    }

    fn finish<'a>(&self) -> V<'a> {
        let total = self.yes + self.no + self.unknown;
        if total == 0 {
            return V::Bool(false);
        }
        if self.raw {
            // 官方 `OP_ITER_END`
            return match self.need {
                _ if self.unknown > 0 => V::Unknown,
                Need::All => V::Bool(i64::try_from(self.yes) == Ok(self.sum)),
                Need::N(0) => V::Bool(self.sum == 0),
                Need::N(n) => V::Bool(self.sum >= n),
                Need::Unknown => V::Unknown,
            };
        }
        let (definite, pending) = match self.need {
            Need::All => (self.no == 0, self.no == 0 && self.unknown > 0),
            Need::N(0) => (self.yes == 0, self.yes == 0 && self.unknown > 0),
            Need::N(n) if n < 0 => (true, false),
            Need::N(n) => {
                let n = n.unsigned_abs();
                (self.yes >= n, self.yes < n && self.yes + self.unknown >= n)
            }
            Need::Unknown => return V::Unknown,
        };
        if pending {
            V::Unknown
        } else {
            V::Bool(definite)
        }
    }
}

impl<'a> Env<'a> {
    fn need(&mut self, quant: &Quant) -> Need {
        let count = match quant {
            Quant::All => return Need::All,
            Quant::Any => return Need::N(1),
            Quant::None => return Need::N(0),
            Quant::N(expr) | Quant::Percent(expr) => self.eval(expr),
        };
        match count {
            V::Undef => Need::All,
            V::Int(n) => Need::N(n),
            _ => Need::Unknown,
        }
    }

    fn tick(&mut self) -> bool {
        self.burn(1)
    }

    fn bind(&mut self, slot: usize, value: V<'a>) {
        if self.slots.len() <= slot {
            self.slots.resize(slot + 1, V::Unknown);
        }
        self.slots[slot] = value;
    }

    /// `<量词> of <串集合> [at X | in (lo..hi)]`
    pub(super) fn of(
        &mut self,
        quant: &Quant,
        set: &[usize],
        limit: &Limit,
    ) -> Result<V<'a>, V<'a>> {
        // 位置限定的操作数未定义 → 整个表达式未定义（官方 OP_OF_FOUND_*）
        let spot = match limit {
            Limit::Anywhere => None,
            Limit::At(at) => {
                let at = self.int(at)?;
                Some((at, at))
            }
            Limit::In(lo, hi) => Some(self.ints(lo, hi)?),
        };
        let percent = match quant {
            Quant::Percent(expr) => Some(self.int(expr)?),
            _ => None,
        };
        let mut tally = Tally::new(self.need(quant), false);
        for index in set {
            let sid = Sid::Named(*index);
            let hit = match (limit, spot) {
                (Limit::At(_), Some((at, _))) => self.found_at(sid, at),
                (_, Some((lo, hi))) => self.found_in(sid, lo, hi),
                _ => self.found(sid),
            };
            tally.push(hit.unwrap_or_else(|early| early));
        }
        let Some(percent) = percent else {
            return Ok(tally.finish());
        };
        // 百分比：(命中 / 总数 × 100) >= P；弃权项按上下界夹逼
        let total = (tally.yes + tally.no + tally.unknown) as f64;
        let reach = |found: u64| (found as f64 / total) * 100.0 >= percent as f64;
        Ok(if reach(tally.yes) {
            V::Bool(true)
        } else if reach(tally.yes + tally.unknown) {
            V::Unknown
        } else {
            V::Bool(false)
        })
    }

    /// `for <量词> of <串集合> : ( 体 )`——集合里**每条**串都迭代（未命中
    /// 的也在：`#` = 0、`$` = 假、`@` = 未定义）。
    pub(super) fn for_of(&mut self, quant: &Quant, set: &[usize], body: &Expr, raw: bool) -> V<'a> {
        let mut tally = Tally::new(self.need(quant), raw);
        for index in set {
            if !self.tick() {
                return V::Unknown;
            }
            let outer = self.current.replace(*index);
            let value = self.eval(body);
            self.current = outer;
            if tally.push(value) {
                break;
            }
        }
        tally.finish()
    }

    /// 绑定循环变量、求一轮体并记账；`None` = 预算耗尽，`Some(true)` = 可收尾。
    fn round(
        &mut self,
        tally: &mut Tally,
        vars: Vars,
        item: Pair<'a>,
        body: &Expr,
    ) -> Option<bool> {
        if !self.tick() {
            return None;
        }
        self.bind(vars.0, item.0);
        if let (Some(slot), Some(second)) = (vars.1, item.1) {
            self.bind(slot, second);
        }
        let value = self.eval(body);
        Some(tally.push(value))
    }

    /// `for <量词> v[, w] in <迭代器> : ( 体 )`
    pub(super) fn for_in(
        &mut self,
        quant: &Quant,
        vars: Vars,
        iter: &Iter,
        body: &Expr,
        raw: bool,
    ) -> V<'a> {
        let mut tally = Tally::new(self.need(quant), raw);
        // 上界是 INT64_MAX 的区间官方跑不完（计数器回绕后继续）——跑到头就弃权
        let mut endless = false;
        let items: Box<dyn Iterator<Item = Pair<'a>> + '_> = match iter {
            Iter::Range(lo, hi) => match (self.eval(lo), self.eval(hi)) {
                (V::Int(lo), V::Int(hi)) => {
                    endless = hi == i64::MAX && lo <= hi;
                    Box::new((lo..=hi).map(|i| (V::Int(i), None)))
                }
                // 任一端未定义 = 零次迭代 = 假（另一端弃权也救不回来）
                (V::Undef, _) | (_, V::Undef) => return V::Bool(false),
                _ => return V::Unknown,
            },
            // 枚举项逐个现算（未定义的项照样迭代，变量就是未定义）
            Iter::List(items) => {
                for item in items {
                    let value = self.eval(item);
                    match self.round(&mut tally, vars, (value, None), body) {
                        None => return V::Unknown,
                        Some(true) => break,
                        Some(false) => {}
                    }
                }
                return tally.finish();
            }
            Iter::Obj(path) => match self.path(path) {
                V::Obj(Node::Array(items)) => Box::new(items.iter().map(|n| (to_v(n), None))),
                V::Obj(Node::Dict(pairs)) => Box::new(
                    pairs
                        .iter()
                        .map(|(key, value)| (V::Str(key.clone()), Some(to_v(value)))),
                ),
                V::Undef => return V::Bool(false),
                _ => return V::Unknown,
            },
        };
        for item in items {
            match self.round(&mut tally, vars, item, body) {
                None => return V::Unknown,
                Some(true) => return tally.finish(),
                Some(false) => {}
            }
        }
        if endless { V::Unknown } else { tally.finish() }
    }
}

type Pair<'a> = (V<'a>, Option<V<'a>>);
type Vars = (usize, Option<usize>);
