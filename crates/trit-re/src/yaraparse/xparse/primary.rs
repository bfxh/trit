//! `primary_expression`：算术 / 位运算层（官方优先级，低 → 高）
//!
//! ```text
//! |   ^   &   << >>   + -   * \ %   一元 - ~
//! ```
//!
//! 全部左结合，一元算子右结合且最紧。除法拼作 `\`（`/` 是正则 / 注释
//! 的起点，官方里 `4 / 2` 是语法错误）。操作数只收整数——浮点不在子集
//! 内，字符串做算术是官方编译错误。常量折叠只为复刻编译错误（见
//! `fold.rs`），不改写树。

use super::boolean::{Parser, Res, Ty, Typed, show};
use super::fold;
use super::lex::T;
use crate::sigscan::expr::{Arith, Expr, Sid};

/// 二元算子按优先级分层（低 → 高）。
const LEVELS: &[&[(&str, Arith)]] = &[
    &[("|", Arith::Or)],
    &[("^", Arith::Xor)],
    &[("&", Arith::And)],
    &[("<<", Arith::Shl), (">>", Arith::Shr)],
    &[("+", Arith::Add), ("-", Arith::Sub)],
    &[("*", Arith::Mul), ("\\", Arith::Div), ("%", Arith::Rem)],
];

/// `uintN` / `intN` 及大端变体：`(字节数, 有符号, 大端)`。
fn reader(word: &str) -> Option<(u8, bool, bool)> {
    let (signed, rest) = match word.strip_prefix('u') {
        Some(rest) => (false, rest),
        None => (true, word),
    };
    let rest = rest.strip_prefix("int")?;
    let (bits, big_endian) = match rest.strip_suffix("be") {
        Some(bits) => (bits, true),
        None => (rest, false),
    };
    let size = match bits {
        "8" => 1,
        "16" => 2,
        "32" => 4,
        _ => return None,
    };
    Some((size, signed, big_endian))
}

impl Parser<'_> {
    pub(super) fn arithmetic(&mut self) -> Res<Typed> {
        self.binary(0)
    }

    /// 优先级爬升：只收 `level` 层及更紧的二元算子。递归脊柱上的函数
    /// （`binary` / `unary` / `paren`）刻意写薄——括号每深一层都要再走一遍
    /// 它们，栈帧大了 64 层括号就能撑爆 2 MiB 的线程栈；干活的部分放在
    /// 不在脊柱上的 `binary_rest` / `combine` / `atom` 里。
    fn binary(&mut self, level: usize) -> Res<Typed> {
        let lhs = self.unary();
        if lhs.is_err() || self.operator(level).is_none() {
            return lhs;
        }
        self.binary_rest(lhs?, level)
    }

    /// 当前 token 是不是 `level` 层及更紧的二元算子 → `(所在层, 写法, 算子)`。
    fn operator(&self, level: usize) -> Option<(usize, &'static str, Arith)> {
        let Some(T::P(punct)) = self.peek() else {
            return None;
        };
        let (at, (text, op)) = LEVELS
            .iter()
            .enumerate()
            .skip(level)
            .find_map(|(at, ops)| {
                let hit = ops.iter().find(|(text, _)| text == punct)?;
                Some((at, *hit))
            })?;
        // `N % of them`：百分号紧跟 `of` 是百分比量词，不是取模
        if op == Arith::Rem && self.is_kw(1, "of") {
            return None;
        }
        Some((at, text, op))
    }

    fn binary_rest(&mut self, mut lhs: Typed, level: usize) -> Res<Typed> {
        while let Some((at, text, op)) = self.operator(level) {
            self.pos += 1;
            let rhs = self.binary(at + 1)?;
            lhs = self.combine(lhs, rhs, text, op)?;
        }
        Ok(lhs)
    }

    fn combine(&self, lhs: Typed, rhs: Typed, text: &str, op: Arith) -> Res<Typed> {
        self.want_int(&lhs, &format!("`{text}` 的左操作数"))?;
        self.want_int(&rhs, &format!("`{text}` 的右操作数"))?;
        let konst = fold::binary(op, lhs.konst, rhs.konst)?;
        let kids = [lhs.depth, rhs.depth];
        let (lhs, rhs) = (lhs.expr, rhs.expr);
        let mut out = self.node(Expr::Arith { op, lhs, rhs }, Ty::Int, true, &kids)?;
        out.konst = konst;
        Ok(out)
    }

    fn unary(&mut self) -> Res<Typed> {
        match self.peek() {
            Some(T::P("-")) => self.signed(Expr::Neg, fold::negate, "-"),
            Some(T::P("~")) => self.signed(Expr::BitNot, fold::invert, "~"),
            Some(T::P("(")) => self.paren(),
            _ => self.atom(),
        }
    }

    fn paren(&mut self) -> Res<Typed> {
        self.pos += 1;
        let inner = self.boolean();
        if inner.is_ok() {
            self.expect_p(")")?;
        }
        inner
    }

    fn signed(
        &mut self,
        make: fn(Box<Expr>) -> Expr,
        fold: fn(Option<i64>) -> Option<i64>,
        sign: &str,
    ) -> Res<Typed> {
        self.pos += 1;
        let inner = self.nested(Self::unary)?;
        self.want_int(&inner, &format!("一元 `{sign}` 的操作数"))?;
        let mut out = self.node(make(inner.expr), Ty::Int, true, &[inner.depth])?;
        out.konst = fold(inner.konst);
        Ok(out)
    }

    fn literal(&self, value: i64) -> Res<Typed> {
        let mut out = self.node(Expr::Int(value), Ty::Int, true, &[])?;
        out.konst = Some(value);
        Ok(out)
    }

    fn atom(&mut self) -> Res<Typed> {
        let token = self.peek();
        self.pos += 1;
        match token {
            Some(T::Int(value)) => self.literal(*value),
            Some(T::Str(bytes)) => self.node(Expr::Str(bytes.clone()), Ty::Str, true, &[]),
            Some(T::Float) => Err("浮点数不在子集内（官方支持；这里如实拒绝）".into()),
            Some(T::Length(_)) => Err("`!a` 匹配长度不在子集内（命中表只记偏移）".into()),
            Some(T::Regex { .. }) => Err("正则字面量只能出现在 `matches` 右侧或函数实参里".into()),
            Some(T::Count(name)) => self.count(&name[1..]),
            Some(T::Offset(name)) => self.offset(&name[1..]),
            Some(T::Ident(word)) => match word.as_str() {
                "filesize" => self.node(Expr::Filesize, Ty::Int, true, &[]),
                "entrypoint" => self.node(Expr::Entrypoint, Ty::Int, true, &[]),
                _ => match reader(word) {
                    Some(shape) => self.read(word, shape),
                    None => self.identifier(word),
                },
            },
            other => {
                self.pos -= 1;
                Err(format!("此处应为数值或字符串操作数，实际是{}", show(other)))
            }
        }
    }

    /// `uintN(<整数表达式>)`
    fn read(&mut self, word: &str, (size, signed, big_endian): (u8, bool, bool)) -> Res<Typed> {
        self.expect_p("(")?;
        let at = self.boolean()?;
        self.expect_p(")")?;
        self.want_int(&at, &format!("`{word}()` 的偏移"))?;
        let kids = [at.depth];
        let at = at.expr;
        self.node(
            Expr::Read {
                size,
                signed,
                big_endian,
                at,
            },
            Ty::Int,
            true,
            &kids,
        )
    }

    /// `#a` / `#a in (lo..hi)`
    fn count(&mut self, name: &str) -> Res<Typed> {
        let sid = self.sid(name)?;
        if !self.eat_kw("in") {
            return self.node(Expr::Count(sid), Ty::Int, true, &[]);
        }
        let (lo, hi, kids) = self.range()?;
        self.node(Expr::CountIn { sid, lo, hi }, Ty::Int, true, &kids)
    }

    /// `@a` / `@a[<整数表达式>]`（1 起；省略 = 第 1 个）
    fn offset(&mut self, name: &str) -> Res<Typed> {
        let sid = self.sid(name)?;
        if !self.eat_p("[") {
            let nth = Box::new(Expr::Int(1));
            return self.node(Expr::Offset { sid, nth }, Ty::Int, true, &[1]);
        }
        let nth = self.boolean()?;
        self.expect_p("]")?;
        self.want_int(&nth, "`@a[…]` 的下标")?;
        let kids = [nth.depth];
        let nth = nth.expr;
        self.node(Expr::Offset { sid, nth }, Ty::Int, true, &kids)
    }

    /// 串引用 → 模式下标。`""` = for-of 的当前串（`$` / `#` / `@`）。
    pub(super) fn sid(&mut self, name: &str) -> Res<Sid> {
        if name.is_empty() {
            return if self.in_for_of {
                Ok(Sid::Current)
            } else {
                Err("匿名串引用只能出现在 `for … of` 的循环体里（官方：misplaced anonymous string）"
                    .into())
            };
        }
        let Some(index) = self.ids.iter().position(|id| *id == name) else {
            return Err(format!(
                "条件引用了未声明的串 ${name}（官方：undefined string）"
            ));
        };
        self.used[index] = true;
        Ok(Sid::Named(index))
    }
}
