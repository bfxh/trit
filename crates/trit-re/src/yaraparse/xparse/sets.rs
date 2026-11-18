//! 串引用、区间、串集合、量词、`of` 与 `for` 循环。官方编译期检查逐条
//! 对应（`grammar.y`）：
//! - 区间 `(lo..hi)`：两端都是常量时 `lo > hi` 或 `lo < 0` = 编译错误。
//! - 量词：常量负数 = 编译错误；百分比常量须在 1..=100；`for` 不收百分比。
//! - 循环嵌套上限 4 层（`for … in` 与 `for … of` 合计）；`for … of` 不能
//!   套在另一个 `for … of` 里；循环变量至多 2 个且不得与外层重名。
//! - 串集合：`them` / `($a, $b*)`；通配项一条都配不上 = 编译错误；
//!   **重复项照算**（官方 `2 of ($a, $a)` 在 `$a` 命中时为真）。

use super::boolean::{Parser, Res, Ty, Typed, show};
use super::lex::T;
use crate::sigscan::expr::schema::Sch;
use crate::sigscan::expr::{Expr, Iter, Limit, Quant};

/// 官方 `YR_MAX_LOOP_NESTING`。
const MAX_LOOPS: u32 = 4;

const RESERVED: &[&str] = &[
    "all",
    "and",
    "any",
    "at",
    "condition",
    "contains",
    "defined",
    "endswith",
    "entrypoint",
    "false",
    "filesize",
    "for",
    "icontains",
    "iendswith",
    "iequals",
    "in",
    "istartswith",
    "matches",
    "none",
    "not",
    "of",
    "or",
    "startswith",
    "them",
    "true",
];

type Bound = Box<Expr>;

impl Parser<'_> {
    /// `$a` / `$a at <expr>` / `$a in (lo..hi)`
    pub(super) fn string_ref(&mut self, token: &str) -> Res<Typed> {
        if token.ends_with('*') {
            return Err(format!("通配串 {token} 只能出现在串集合里"));
        }
        let sid = self.sid(&token[1..])?;
        if self.eat_kw("at") {
            let at = self.arithmetic()?;
            self.want_int(&at, "`at` 的偏移")?;
            let kids = [at.depth];
            let at = at.expr;
            return self.node(Expr::FoundAt { sid, at }, Ty::Bool, false, &kids);
        }
        if self.eat_kw("in") {
            let (lo, hi, kids) = self.range()?;
            return self.node(Expr::FoundIn { sid, lo, hi }, Ty::Bool, false, &kids);
        }
        self.node(Expr::Found(sid), Ty::Bool, false, &[])
    }

    /// 区间端点 / 枚举项：括在 `( … )` 里的算术式。这层括号不经过
    /// `boolean()`，所以自己过一遍嵌套闸（`#a in (0..#a in (0..…` 否则不设限）。
    fn enclosed(&mut self) -> Res<Typed> {
        self.nested(Self::arithmetic)
    }

    /// `( lo .. hi )`：闭区间。
    pub(super) fn range(&mut self) -> Res<(Bound, Bound, [u32; 2])> {
        self.expect_p("(")?;
        let lo = self.enclosed()?;
        self.expect_p("..")?;
        self.range_tail(lo)
    }

    /// 已读完 `( lo ..`。
    fn range_tail(&mut self, lo: Typed) -> Res<(Bound, Bound, [u32; 2])> {
        let hi = self.enclosed()?;
        self.expect_p(")")?;
        self.want_int(&lo, "区间下界")?;
        self.want_int(&hi, "区间上界")?;
        if let (Some(low), Some(high)) = (lo.konst, hi.konst) {
            if low > high {
                return Err(format!(
                    "常量区间下界 {low} 大于上界 {high}（官方：invalid value）"
                ));
            }
            if low < 0 {
                return Err(format!("常量区间下界 {low} 为负（官方：invalid value）"));
            }
        }
        Ok((lo.expr, hi.expr, [lo.depth, hi.depth]))
    }

    /// `them` / `( $a, $b*, … )` → 模式下标表（按书写顺序，重复保留）。
    fn string_set(&mut self) -> Res<Vec<usize>> {
        if self.eat_kw("them") {
            return self.matching("$*");
        }
        self.expect_p("(")?;
        let mut set = Vec::new();
        loop {
            match self.peek() {
                Some(T::Pat(item)) => {
                    self.pos += 1;
                    set.extend(self.matching(item)?);
                }
                Some(T::Ident(_)) => {
                    return Err("规则集合（`of (rule1, rule2*)`）不在子集内".into());
                }
                other => return Err(format!("串集合里应为 `$串`，实际是{}", show(other))),
            }
            if !self.eat_p(",") {
                break;
            }
        }
        self.expect_p(")")?;
        Ok(set)
    }

    /// 官方 `yr_parser_emit_pushes_for_strings`：精确名或 `前缀*`。
    fn matching(&mut self, item: &str) -> Res<Vec<usize>> {
        let hit = |id: &str| match item[1..].strip_suffix('*') {
            Some(prefix) => id.starts_with(prefix),
            None => !id.is_empty() && id == &item[1..],
        };
        let found: Vec<usize> = (0..self.ids.len()).filter(|i| hit(self.ids[*i])).collect();
        if found.is_empty() {
            return Err(format!(
                "串集合项 {item} 没有匹配任何已声明的串（官方：undefined string）"
            ));
        }
        for index in &found {
            self.used[*index] = true;
        }
        Ok(found)
    }

    pub(super) fn keyword_quant(&self, word: &str) -> Quant {
        match word {
            "all" => Quant::All,
            "any" => Quant::Any,
            _ => Quant::None,
        }
    }

    /// 整数量词（官方 `for_expression`）：常量负数 = 编译错误。
    pub(super) fn count_quant(&self, count: Typed) -> Res<(Quant, u32)> {
        self.want_int(&count, "量词")?;
        match count.konst {
            Some(value) if value < 0 => Err(format!("量词是负常量 {value}（官方：invalid value）")),
            _ => Ok((Quant::N(count.expr), count.depth)),
        }
    }

    /// `P% of …`：常量须在 1..=100。
    pub(super) fn percent_quant(&self, percent: Typed) -> Res<(Quant, u32)> {
        self.want_int(&percent, "百分比")?;
        match percent.konst {
            Some(value) if !(1..=100).contains(&value) => Err(format!(
                "百分比常量 {value} 不在 1..=100（官方：invalid percentage）"
            )),
            _ => Ok((Quant::Percent(percent.expr), percent.depth)),
        }
    }

    /// 已读完 `<量词> of`。`positional` = 可带 `at X` / `in (lo..hi)`
    /// （百分比形式官方文法里没有位置限定）。
    pub(super) fn of_tail(&mut self, quant: Quant, depth: u32, positional: bool) -> Res<Typed> {
        let set = self.string_set()?;
        let mut kids = vec![depth];
        let limit = if positional && self.eat_kw("at") {
            let at = self.arithmetic()?;
            self.want_int(&at, "`at` 的偏移")?;
            kids.push(at.depth);
            Limit::At(at.expr)
        } else if positional && self.eat_kw("in") {
            let (lo, hi, depths) = self.range()?;
            kids.extend(depths);
            Limit::In(lo, hi)
        } else {
            Limit::Anywhere
        };
        self.node(Expr::Of { quant, set, limit }, Ty::Bool, false, &kids)
    }

    /// 已读完 `for`。
    pub(super) fn for_loop(&mut self) -> Res<Typed> {
        let (quant, quant_depth) = match self.peek() {
            Some(T::Ident(word)) if matches!(word.as_str(), "all" | "any" | "none") => {
                self.pos += 1;
                (self.keyword_quant(word), 0)
            }
            _ => {
                let count = self.arithmetic()?;
                self.count_quant(count)?
            }
        };
        if self.loops >= MAX_LOOPS {
            return Err(format!(
                "循环嵌套超过 {MAX_LOOPS} 层（官方：loop nesting limit exceeded）"
            ));
        }
        self.loops += 1;
        let out = if self.eat_kw("of") {
            self.for_of(quant, quant_depth)
        } else {
            self.for_in(quant, quant_depth)
        };
        self.loops -= 1;
        out
    }

    /// `: ( 体 )`；返回 `(体, 体是整数类型?)`。
    fn body(&mut self) -> Res<(Typed, bool)> {
        self.expect_p(":")?;
        self.expect_p("(")?;
        let body = self.boolean()?;
        self.expect_p(")")?;
        self.truthy(&body, "循环体")?;
        let raw = matches!(body.ty, Ty::Int);
        Ok((body, raw))
    }

    fn for_of(&mut self, quant: Quant, quant_depth: u32) -> Res<Typed> {
        if self.in_for_of {
            return Err("`for … of` 不能嵌套在另一个 `for … of` 里（官方：nested for-of）".into());
        }
        let set = self.string_set()?;
        self.in_for_of = true;
        let body = self.body();
        self.in_for_of = false;
        let (body, raw) = body?;
        let kids = [quant_depth, body.depth];
        let body = body.expr;
        self.node(
            Expr::ForOf {
                quant,
                set,
                body,
                raw,
            },
            Ty::Bool,
            false,
            &kids,
        )
    }

    fn for_in(&mut self, quant: Quant, quant_depth: u32) -> Res<Typed> {
        let mut names: Vec<&str> = Vec::new();
        loop {
            let Some(T::Ident(name)) = self.peek() else {
                return Err(format!(
                    "`for` 之后应为循环变量或 `of`，实际是{}",
                    show(self.peek())
                ));
            };
            self.pos += 1;
            if RESERVED.contains(&name.as_str()) {
                return Err(format!("`{name}` 是保留字，不能当循环变量"));
            }
            if names.len() == 2 {
                return Err("循环变量超过 2 个（官方：too many loop variables）".into());
            }
            if names.contains(&name.as_str()) || self.scope.iter().any(|(used, _)| used == name) {
                return Err(format!(
                    "循环变量 `{name}` 重名（官方：duplicated loop identifier）"
                ));
            }
            names.push(name);
            if !self.eat_p(",") {
                break;
            }
        }
        if !self.eat_kw("in") {
            return Err(format!(
                "循环变量之后应为 `in`，实际是{}",
                show(self.peek())
            ));
        }
        let (iter, types, mut kids) = self.iterator()?;
        if types.len() != names.len() {
            return Err(format!(
                "迭代器每轮给出 {} 个值，循环却声明了 {} 个变量（官方：syntax error）",
                types.len(),
                names.len()
            ));
        }
        let slot = self.scope.len();
        let vars = (slot, (names.len() == 2).then_some(slot + 1));
        for (name, ty) in names.iter().zip(types) {
            self.scope.push(((*name).to_string(), ty));
        }
        let body = self.body();
        self.scope.truncate(slot);
        let (body, raw) = body?;
        kids.extend([quant_depth, body.depth]);
        let body = body.expr;
        self.node(
            Expr::ForIn {
                quant,
                vars,
                iter,
                body,
                raw,
            },
            Ty::Bool,
            false,
            &kids,
        )
    }

    /// 迭代器：区间 / 枚举 / 模块数组或字典 → `(迭代器, 每轮的变量类型, 深度)`。
    fn iterator(&mut self) -> Res<(Iter, Vec<Ty>, Vec<u32>)> {
        if !self.eat_p("(") {
            let Some(T::Ident(word)) = self.peek() else {
                return Err(format!(
                    "`in` 之后应为区间、枚举或可迭代对象，实际是{}",
                    show(self.peek())
                ));
            };
            self.pos += 1;
            let (path, sch, depth) = self.object(word)?;
            let types = match sch {
                Sch::Array(item) => vec![value_type(item)],
                Sch::Dict(item) => vec![Ty::Str, value_type(item)],
                _ => {
                    return Err(format!(
                        "`{word}…` 不可迭代（官方：identifier is not iterable）"
                    ));
                }
            };
            return Ok((Iter::Obj(path), types, vec![depth]));
        }
        let first = self.enclosed()?;
        if self.eat_p("..") {
            let (lo, hi, depths) = self.range_tail(first)?;
            return Ok((Iter::Range(lo, hi), vec![Ty::Int], depths.to_vec()));
        }
        // 枚举：全整数或全字符串
        let ty = first.ty;
        let (mut items, mut kids) = (Vec::new(), Vec::new());
        let mut item = first;
        loop {
            match (ty, item.ty) {
                (Ty::Int, Ty::Int) | (Ty::Str, Ty::Str) if item.primary => {}
                (Ty::Int | Ty::Str, _) => {
                    return Err("枚举项必须同为整数或同为字符串（官方：wrong type）".into());
                }
                _ => return Err("枚举项只能是整数或字符串（官方：wrong type）".into()),
            }
            kids.push(item.depth);
            items.push(*item.expr);
            if !self.eat_p(",") {
                break;
            }
            item = self.enclosed()?;
        }
        self.expect_p(")")?;
        Ok((Iter::List(items), vec![ty], kids))
    }
}

/// 模块类型 → 值类型（标量直接当值，其余留作对象继续走路径）。
pub(super) fn value_type(sch: &'static Sch) -> Ty {
    match sch {
        Sch::Int => Ty::Int,
        Sch::Str => Ty::Str,
        other => Ty::Obj(other),
    }
}
