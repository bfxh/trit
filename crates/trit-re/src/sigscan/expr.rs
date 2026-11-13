//! YARA 表达式语言（N4 深水区）：算术/位运算、循环变量与区间迭代、动态数组
//! 下标、字符串函数、pe 模块对象模型。语义逐条由官方 yara-python 4.5.4
//! 探针定案（见 `docs/YARA-EXPR.md`），**不是**凭文档猜的。
//!
//! 三值纪律（硬约束）：求值域是四态 `值 / 确定的未定义 / 弃权`——
//! - **确定的未定义**（`Undef`）= 官方 `YR_UNDEFINED`：越界读、除零、越界
//!   下标、非 PE 上的 `pe.*`。它在严格算子里传染，在 `and`/`or` 里当假，
//!   过 `not` 仍是未定义，顶层未定义 = 不命中。
//! - **弃权**（`Unknown`）= 证据缺失：截断输入、缺 PE 上下文、子集外字段、
//!   求值预算耗尽。**绝不折叠成假**；只有当另一侧让结果与它无关时才消失
//!   （Kleene：`假 and ?` = 假，`真 or ?` = 真；严格算子里 `Undef` 压过它）。
//!
//! 本模块只定义 AST 与入口；求值在 [`eval`]，对象模型在 [`node`]/[`yrpe`]。

use super::eval::EvalCtx;
use super::regex::{ByteSet, Instr, RegexPat};
use trit_core::value::Ternary;

mod call;
mod eval;
mod loops;
mod node;
pub(super) use node::PeLazy;
mod num;
mod ordnames;
mod peconst;
pub(crate) mod schema;
mod strfunc;
mod yrdbg;
mod yrexp;
mod yrimp;
mod yrpe;
mod yrres;

/// 比较算子（整数按 int64 有符号；字符串按字节序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rel {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// 二元算术 / 位运算（`Div` 的源码拼写是 `\`，不是 `/`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

/// 字符串函数（两侧都必须是字符串；`i*` = ASCII 大小写不敏感）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrFn {
    Contains,
    IContains,
    StartsWith,
    IStartsWith,
    EndsWith,
    IEndsWith,
    IEquals,
}

/// 串引用：具名（`$a` = 模式下标）或 for-of 循环的当前串（`$`/`#`/`@`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sid {
    Named(usize),
    Current,
}

/// 已编译的正则（`matches` 右侧 / `pe.imports(/re/, /re/)` 实参）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matcher {
    pub(crate) pat: RegexPat,
}

impl Matcher {
    /// 编译正则字面量；现有 Pike VM 子集之外的语法如实 `Err`。
    pub(crate) fn compile(body: &str, nocase: bool) -> Result<Self, String> {
        let mut pat = super::regex::compile(body, nocase)?;
        if nocase {
            // 现有引擎的 nocase 只把**数据**字节折成小写再查集合，模式里的
            // 大写字母因此永远配不上（`/HELLO/i` 对 "hello" 不命中）。这里
            // 把每个字节集补成大小写封闭；串扫描那一侧的同一问题不在本
            // 模块职责内，登记在 docs/YARA-EXPR.md。
            for instr in &mut pat.prog {
                if let Instr::Byte(ByteSet::Set { set, .. }) = instr {
                    for upper in b'A'..=b'Z' {
                        let (upper, lower) = (usize::from(upper), usize::from(upper | 0x20));
                        let either = set[upper] || set[lower];
                        (set[upper], set[lower]) = (either, either);
                    }
                }
            }
        }
        Ok(Self { pat })
    }
}

/// 量词：`all` / `any` / `none` / 表达式 / 百分比（仅 `of`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quant {
    All,
    Any,
    None,
    N(Box<Expr>),
    Percent(Box<Expr>),
}

/// `N of <集合>` 的位置限定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Limit {
    Anywhere,
    At(Box<Expr>),
    In(Box<Expr>, Box<Expr>),
}

/// `for … in <迭代器>`：闭区间 / 枚举 / 模块数组或字典。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Iter {
    Range(Box<Expr>, Box<Expr>),
    List(Vec<Expr>),
    Obj(Path),
}

/// 对象路径的根：`pe` 模块或绑定到对象的循环变量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    Pe,
    Var(usize),
}

/// 路径段：字段 / 数组下标 / 字典键 / 函数调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Field(&'static str),
    Index(Box<Expr>),
    Key(Box<Expr>),
    Call(Vec<Arg>),
}

/// 函数实参：值表达式或正则字面量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Val(Expr),
    Re(Matcher),
}

/// 模块对象路径（`pe.sections[i].name`、`s.name`、`pe.imports("a", "b")`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub root: Root,
    pub segs: Vec<Seg>,
}

/// 表达式树。类型（整数/字符串/布尔）在解析期静态定型，这里不再携带。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Int(i64),
    Str(Vec<u8>),
    Bool(bool),
    Filesize,
    Entrypoint,
    /// 循环变量（槽位由解析器按嵌套层分配）。
    Var(usize),
    /// `uintN` / `intN` / `…be`：`size` 字节，越界 = 确定的未定义。
    Read {
        size: u8,
        signed: bool,
        big_endian: bool,
        at: Box<Expr>,
    },
    /// `#a`
    Count(Sid),
    /// `#a in (lo..hi)`
    CountIn {
        sid: Sid,
        lo: Box<Expr>,
        hi: Box<Expr>,
    },
    /// `@a[nth]`（1 起；`@a` = `@a[1]`）
    Offset {
        sid: Sid,
        nth: Box<Expr>,
    },
    Neg(Box<Expr>),
    BitNot(Box<Expr>),
    Arith {
        op: Arith,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Path(Path),
    Cmp {
        op: Rel,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    StrOp {
        op: StrFn,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Matches {
        lhs: Box<Expr>,
        re: Matcher,
    },
    /// `$a`
    Found(Sid),
    /// `$a at <expr>`
    FoundAt {
        sid: Sid,
        at: Box<Expr>,
    },
    /// `$a in (lo..hi)`
    FoundIn {
        sid: Sid,
        lo: Box<Expr>,
        hi: Box<Expr>,
    },
    /// `<量词> of <串集合> [at X | in (lo..hi)]`
    Of {
        quant: Quant,
        set: Vec<usize>,
        limit: Limit,
    },
    /// `for <量词> of <串集合> : ( 体 )`。`raw` = 体是整数类型：官方把体的
    /// **原始整数值**（不是真值）累加进"真"计数（见 `loops.rs`）。
    ForOf {
        quant: Quant,
        set: Vec<usize>,
        body: Box<Expr>,
        raw: bool,
    },
    /// `for <量词> v[, w] in <迭代器> : ( 体 )`；`vars` = 循环变量槽位。
    ForIn {
        quant: Quant,
        vars: (usize, Option<usize>),
        iter: Iter,
        body: Box<Expr>,
        raw: bool,
    },
    Defined(Box<Expr>),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

/// 求值入口：顶层未定义 = 不命中（`False`）；弃权如实上报 `Unknown`。
pub(super) fn run<'a>(
    expr: &Expr,
    occurrences: &'a [(String, Vec<u64>)],
    ctx: &EvalCtx<'a, 'a>,
) -> Ternary {
    let mut env = eval::Env::new(occurrences, ctx);
    let value = env.eval(expr);
    match eval::truth(value) {
        eval::V::Bool(true) => Ternary::True,
        eval::V::Unknown => Ternary::Unknown,
        _ => Ternary::False,
    }
}

#[cfg(test)]
mod tests;
