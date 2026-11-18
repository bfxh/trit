//! 表达式解析器的骨架与布尔层：`or` < `and` < `not`/`defined` < 比较 /
//! 字符串算子。照官方 `grammar.y` 的两层文法走——
//! - `primary_expression`（数值 / 字符串：算术、读取、路径、字面量）；
//! - `expression`（布尔：比较、串引用、`of`、`for`、`and`/`or`/`not`）。
//!
//! 比较与字符串算子的操作数必须是 `primary_expression`，所以**不能连写**
//! （`1 < 2 < 3`、`(1 < 2) == true` 都是官方语法错误）。每个节点带着
//! `primary` 标记：`( … )` 里包的是什么，括号外就还是什么。
//!
//! 类型在这里静态定死（官方同款）：类型不匹配、把字符串拿去做算术、
//! 把整数拿去 `contains`，都是编译错误 → `Err` → 整条规则 `Unsupported`。

use super::lex::T;
use crate::sigscan::expr::schema::Sch;
use crate::sigscan::expr::{Expr, Matcher, Rel, StrFn};

/// 括号 / 一元算子 / 循环体的嵌套上限（解析器递归深度）。
pub(super) const MAX_NESTING: u32 = 64;
/// 表达式树的深度上限——求值器是递归的，树太深会爆栈；`1 + 1 + …` 这种
/// 左结合长链不走解析器递归，却照样长出深树，所以单独设限。
const MAX_TREE_DEPTH: u32 = 256;

#[derive(Debug, Clone, Copy)]
pub(super) enum Ty {
    Int,
    Str,
    Bool,
    /// 结构 / 数组 / 字典 / 函数——不能当值用，只能继续走路径或被迭代。
    Obj(&'static Sch),
}

pub(super) struct Typed {
    /// 装箱：`Typed` 在递归下降的每一帧里都要按值传好几份，表达式本体
    /// 留在堆上，栈帧才薄（见 `primary.rs` 的栈帧说明）。
    pub(super) expr: Box<Expr>,
    pub(super) ty: Ty,
    /// 编译期常量值（官方口径：只有整数字面量及其运算结果）。
    pub(super) konst: Option<i64>,
    /// 是不是官方文法里的 `primary_expression`。
    pub(super) primary: bool,
    pub(super) depth: u32,
}

pub(super) type Res<X> = Result<X, String>;

pub(super) struct Parser<'t> {
    toks: &'t [T],
    pub(super) pos: usize,
    pub(super) ids: &'t [&'t str],
    pub(super) has_pe: bool,
    /// 作用域内的循环变量；槽位号 = 下标。
    pub(super) scope: Vec<(String, Ty)>,
    pub(super) loops: u32,
    pub(super) in_for_of: bool,
    /// 每条串有没有被条件引用（官方：未引用 = 编译错误）。
    pub(super) used: Vec<bool>,
    nesting: u32,
}

pub(super) fn show(token: Option<&T>) -> String {
    match token {
        None => "条件结尾".into(),
        Some(
            T::Ident(word) | T::Pat(word) | T::Count(word) | T::Offset(word) | T::Length(word),
        ) => {
            format!("`{word}`")
        }
        Some(T::Int(value)) => format!("`{value}`"),
        Some(T::P(punct)) => format!("`{punct}`"),
        Some(T::Str(_) | T::RawStr(_)) => "字符串字面量".into(),
        Some(T::Regex { .. }) => "正则字面量".into(),
        Some(T::Float) => "浮点字面量".into(),
        Some(T::Hex) => "十六进制串".into(),
        Some(T::Bad(why)) => format!("非法 token（{why}）"),
    }
}

impl<'t> Parser<'t> {
    pub(super) fn new(toks: &'t [T], ids: &'t [&'t str], has_pe: bool) -> Self {
        Self {
            toks,
            pos: 0,
            ids,
            has_pe,
            scope: Vec::new(),
            loops: 0,
            in_for_of: false,
            used: vec![false; ids.len()],
            nesting: 0,
        }
    }

    pub(super) fn peek(&self) -> Option<&'t T> {
        self.toks.get(self.pos)
    }

    pub(super) fn peek_at(&self, ahead: usize) -> Option<&'t T> {
        self.toks.get(self.pos + ahead)
    }

    pub(super) fn is_kw(&self, ahead: usize, word: &str) -> bool {
        matches!(self.peek_at(ahead), Some(T::Ident(w)) if w == word)
    }

    pub(super) fn eat_kw(&mut self, word: &str) -> bool {
        let hit = self.is_kw(0, word);
        self.pos += usize::from(hit);
        hit
    }

    pub(super) fn eat_p(&mut self, punct: &str) -> bool {
        let hit = matches!(self.peek(), Some(T::P(p)) if *p == punct);
        self.pos += usize::from(hit);
        hit
    }

    pub(super) fn expect_p(&mut self, punct: &str) -> Res<()> {
        if self.eat_p(punct) {
            Ok(())
        } else {
            Err(format!("此处应为 `{punct}`，实际是{}", show(self.peek())))
        }
    }

    /// 造节点并守树深上限。`kids` = 子节点的深度。
    pub(super) fn node(&self, expr: Expr, ty: Ty, primary: bool, kids: &[u32]) -> Res<Typed> {
        let depth = 1 + kids.iter().copied().max().unwrap_or(0);
        if depth > MAX_TREE_DEPTH {
            return Err(format!("表达式过深（树深上限 {MAX_TREE_DEPTH}）"));
        }
        Ok(Typed {
            expr: Box::new(expr),
            ty,
            konst: None,
            primary,
            depth,
        })
    }

    /// 递归下降的入口守卫（括号、一元算子链、循环体……）。
    pub(super) fn nested<X>(&mut self, inner: impl FnOnce(&mut Self) -> Res<X>) -> Res<X> {
        if self.nesting >= MAX_NESTING {
            return Err(format!("嵌套过深（上限 {MAX_NESTING} 层）"));
        }
        self.nesting += 1;
        let out = inner(self);
        self.nesting -= 1;
        out
    }

    /// 布尔上下文只收 整数（非零）/ 字符串（非空）/ 布尔。
    pub(super) fn truthy(&self, value: &Typed, what: &str) -> Res<()> {
        match value.ty {
            Ty::Int | Ty::Str | Ty::Bool => Ok(()),
            Ty::Obj(_) => Err(format!(
                "{what}不是可判真假的值（结构 / 数组 / 函数不能直接当条件）"
            )),
        }
    }

    /// `primary_expression` 且类型为整数——算术、下标、偏移、区间端点通用。
    pub(super) fn want_int(&self, value: &Typed, what: &str) -> Res<()> {
        self.want_primary(value, what)?;
        match value.ty {
            Ty::Int => Ok(()),
            _ => Err(format!("{what}必须是整数（官方：wrong type）")),
        }
    }

    pub(super) fn want_str(&self, value: &Typed, what: &str) -> Res<()> {
        self.want_primary(value, what)?;
        match value.ty {
            Ty::Str => Ok(()),
            _ => Err(format!("{what}必须是字符串（官方：wrong type）")),
        }
    }

    fn want_primary(&self, value: &Typed, what: &str) -> Res<()> {
        if value.primary {
            Ok(())
        } else {
            Err(format!("{what}不能是布尔表达式（官方：syntax error）"))
        }
    }

    /// 整条条件。
    pub(super) fn parse_full(mut self) -> Res<Expr> {
        let named = |id: &&str| !id.is_empty();
        for (index, id) in self.ids.iter().enumerate().filter(|(_, id)| named(id)) {
            if self.ids[..index].contains(id) {
                return Err(format!(
                    "串标识符 ${id} 重复声明（官方：duplicated string identifier）"
                ));
            }
        }
        if let Some(T::Bad(why)) = self.toks.iter().find(|t| matches!(t, T::Bad(_))) {
            return Err(why.clone());
        }
        let top = self.boolean()?;
        if self.peek().is_some() {
            return Err(format!("条件在{}处无法继续解析", show(self.peek())));
        }
        self.truthy(&top, "条件")?;
        // 官方：未被条件引用的串是编译错误（`$_` 开头的具名串豁免）
        for (id, used) in self.ids.iter().zip(&self.used) {
            if !used && !id.starts_with('_') {
                return Err(format!(
                    "串 ${id} 未被条件引用（官方：unreferenced string）"
                ));
            }
        }
        Ok(*top.expr)
    }

    /// `boolean_expression`：`or` 链（n 元，短路语义在求值器）。
    pub(super) fn boolean(&mut self) -> Res<Typed> {
        self.nested(|p| p.chain("or", Self::conjunction, Expr::Or))
    }

    fn conjunction(&mut self) -> Res<Typed> {
        self.chain("and", Self::negation, Expr::And)
    }

    /// `a <kw> b <kw> c`。只有一个操作数时原样透传——这条路在递归脊柱上
    /// （每层括号走两遍），所以不在这里摊开 `Typed`（栈帧说明见 `primary.rs`）。
    fn chain(
        &mut self,
        keyword: &str,
        operand: fn(&mut Self) -> Res<Typed>,
        make: fn(Vec<Expr>) -> Expr,
    ) -> Res<Typed> {
        let first = operand(self);
        if first.is_err() || !self.is_kw(0, keyword) {
            return first;
        }
        self.chain_rest(first?, keyword, operand, make)
    }

    fn chain_rest(
        &mut self,
        first: Typed,
        keyword: &str,
        operand: fn(&mut Self) -> Res<Typed>,
        make: fn(Vec<Expr>) -> Expr,
    ) -> Res<Typed> {
        let (mut kids, mut items) = (Vec::new(), Vec::new());
        self.link(first, &mut kids, &mut items)?;
        while self.eat_kw(keyword) {
            let next = operand(self)?;
            self.link(next, &mut kids, &mut items)?;
        }
        self.node(make(items), Ty::Bool, false, &kids)
    }

    fn link(&self, item: Typed, kids: &mut Vec<u32>, items: &mut Vec<Expr>) -> Res<()> {
        self.truthy(&item, "逻辑算子的操作数")?;
        kids.push(item.depth);
        items.push(*item.expr);
        Ok(())
    }

    /// `not` / `defined`：比 `and` 紧、比比较松（`not a == b` = `not (a == b)`）。
    fn negation(&mut self) -> Res<Typed> {
        if self.is_kw(0, "not") {
            self.negated(Expr::Not)
        } else if self.is_kw(0, "defined") {
            self.negated(Expr::Defined)
        } else {
            self.relation()
        }
    }

    fn negated(&mut self, make: fn(Box<Expr>) -> Expr) -> Res<Typed> {
        self.pos += 1;
        let inner = self.nested(Self::negation)?;
        self.truthy(&inner, "not / defined 的操作数")?;
        self.node(make(inner.expr), Ty::Bool, false, &[inner.depth])
    }

    /// 官方 `expression` 的叶子层：串引用、`of`、`for`、比较、字符串算子。
    fn relation(&mut self) -> Res<Typed> {
        if let Some(done) = self.keyword_leaf() {
            return done;
        }
        match self.arithmetic() {
            // `( 布尔表达式 )`：后面不能再接比较
            Ok(lhs) if lhs.primary => self.relation_tail(lhs),
            other => other,
        }
    }

    /// 以关键字 / 串引用开头的叶子；不是这类开头就 `None`。
    fn keyword_leaf(&mut self) -> Option<Res<Typed>> {
        Some(match self.peek() {
            Some(T::Ident(word)) if word == "true" || word == "false" => {
                self.pos += 1;
                self.node(Expr::Bool(word == "true"), Ty::Bool, false, &[])
            }
            Some(T::Pat(name)) => {
                self.pos += 1;
                self.string_ref(name)
            }
            Some(T::Ident(word)) if word == "for" => {
                self.pos += 1;
                self.for_loop()
            }
            Some(T::Ident(word)) if matches!(word.as_str(), "all" | "any" | "none") => {
                self.pos += 1;
                let quant = self.keyword_quant(word);
                if self.eat_kw("of") {
                    self.of_tail(quant, 0, true)
                } else {
                    Err(format!(
                        "`{word}` 之后应为 `of`，实际是{}",
                        show(self.peek())
                    ))
                }
            }
            _ => return None,
        })
    }

    /// 左操作数之后：比较 / 字符串算子 / `N of` / `N % of`。
    fn relation_tail(&mut self, lhs: Typed) -> Res<Typed> {
        let rel = match self.peek() {
            Some(T::P("==")) => Some(Rel::Eq),
            Some(T::P("!=")) => Some(Rel::Ne),
            Some(T::P("<")) => Some(Rel::Lt),
            Some(T::P("<=")) => Some(Rel::Le),
            Some(T::P(">")) => Some(Rel::Gt),
            Some(T::P(">=")) => Some(Rel::Ge),
            _ => None,
        };
        if let Some(op) = rel {
            self.pos += 1;
            let rhs = self.arithmetic()?;
            self.want_primary(&rhs, "比较的右操作数")?;
            if !matches!((lhs.ty, rhs.ty), (Ty::Int, Ty::Int) | (Ty::Str, Ty::Str)) {
                return Err("比较两侧类型不一致（官方：type mismatch）".into());
            }
            let kids = [lhs.depth, rhs.depth];
            let (lhs, rhs) = (lhs.expr, rhs.expr);
            return self.node(Expr::Cmp { op, lhs, rhs }, Ty::Bool, false, &kids);
        }
        let word = match self.peek() {
            Some(T::Ident(word)) => word.as_str(),
            Some(T::P("%")) if self.is_kw(1, "of") => {
                self.pos += 2;
                let quant = self.percent_quant(lhs)?;
                return self.of_tail(quant.0, quant.1, false);
            }
            _ => return Ok(lhs),
        };
        let op = match word {
            "contains" => StrFn::Contains,
            "icontains" => StrFn::IContains,
            "startswith" => StrFn::StartsWith,
            "istartswith" => StrFn::IStartsWith,
            "endswith" => StrFn::EndsWith,
            "iendswith" => StrFn::IEndsWith,
            "iequals" => StrFn::IEquals,
            "matches" => return self.matches(lhs),
            "of" => {
                self.pos += 1;
                let quant = self.count_quant(lhs)?;
                return self.of_tail(quant.0, quant.1, true);
            }
            _ => return Ok(lhs),
        };
        self.pos += 1;
        self.want_str(&lhs, &format!("`{word}` 的左操作数"))?;
        let rhs = self.arithmetic()?;
        self.want_str(&rhs, &format!("`{word}` 的右操作数"))?;
        let kids = [lhs.depth, rhs.depth];
        let (lhs, rhs) = (lhs.expr, rhs.expr);
        self.node(Expr::StrOp { op, lhs, rhs }, Ty::Bool, false, &kids)
    }

    /// `<串> matches /re/[is]`——右侧只能是正则字面量。
    fn matches(&mut self, lhs: Typed) -> Res<Typed> {
        self.pos += 1;
        self.want_str(&lhs, "`matches` 的左操作数")?;
        let Some(T::Regex { body, flags }) = self.peek() else {
            return Err(format!(
                "`matches` 右侧必须是正则字面量，实际是{}",
                show(self.peek())
            ));
        };
        self.pos += 1;
        let re = compile_regex(body, flags)?;
        let lhs_expr = lhs.expr;
        self.node(
            Expr::Matches { lhs: lhs_expr, re },
            Ty::Bool,
            false,
            &[lhs.depth],
        )
    }
}

/// 正则字面量 → 现有 Pike VM 程序。`/s`（`.` 匹配换行）现有引擎没有；
/// 正则体里没有 `.` 时它是空操作，可以放行，否则如实拒绝。
pub(super) fn compile_regex(body: &str, flags: &str) -> Res<Matcher> {
    if flags.contains('s') && body.contains('.') {
        return Err("正则标志 /s（`.` 匹配换行）不在现有正则子集内".into());
    }
    Matcher::compile(body, flags.contains('i')).map_err(|why| format!("正则不在现有子集内：{why}"))
}
