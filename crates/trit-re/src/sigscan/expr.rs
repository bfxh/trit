//! YARA 表达式 AST（接缝实现，接口按 `docs/YARA-EXPR/04-CODE-MAP.md` §3.1）。
//!
//! 只定义结构，不求值；求值见 [`super::xeval`]。

use super::CmpOp;

/// 字符串引用：具名下标或 `for … of` 体内的当前串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrRef {
    Id(usize),
    Current,
}

/// 正则字面量（保留 `s` 标志，编译时由求值器处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexLit {
    pub src: String,
    pub nocase: bool,
    pub dotall: bool,
}

impl RegexLit {
    pub fn new(src: &str, nocase: bool, dotall: bool) -> Self {
        Self {
            src: src.to_string(),
            nocase,
            dotall,
        }
    }
}

/// `of` 量词。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quant {
    All,
    Any,
    None,
    N(Box<Expr>),
    Percent(Box<Expr>),
}

/// 循环迭代源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Iter {
    Range(Box<Expr>, Box<Expr>),
    Ints(Vec<Expr>),
    Strs(Vec<Expr>),
    Array(ObjPath),
    Dict(ObjPath),
    Strings(Vec<usize>),
}

/// `of` 集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfSet {
    Strings(Vec<usize>),
    Rules(Vec<String>),
}

/// `of` 尾巴。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfTail {
    Plain,
    At(Box<Expr>),
    In(Box<Expr>, Box<Expr>),
}

/// 对象路径根。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Root {
    Module(String),
    Var(u8),
}

/// 对象路径的一段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Field(String),
    Index(Box<Expr>),
    Key(Box<Expr>),
    Call(Vec<Expr>),
}

/// 模块/对象路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjPath {
    pub root: Root,
    pub segs: Vec<Seg>,
}

/// 表达式结点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Int(i64),
    Float(u64),
    Str(Vec<u8>),
    Bool(bool),
    Regex(RegexLit),
    Filesize,
    Entrypoint,
    ReadInt {
        signed: bool,
        bytes: u8,
        big_endian: bool,
        at: Box<Expr>,
    },
    Found(StrRef),
    FoundAt(StrRef, Box<Expr>),
    FoundIn(StrRef, Box<Expr>, Box<Expr>),
    Count(StrRef),
    CountIn(StrRef, Box<Expr>, Box<Expr>),
    Offset(StrRef, Box<Expr>),
    Length(StrRef, Box<Expr>),
    Var(u8),
    Obj(ObjPath),
    Neg {
        float: bool,
        e: Box<Expr>,
    },
    BitNot(Box<Expr>),
    Bin {
        op: BinOp,
        float: bool,
        a: Box<Expr>,
        b: Box<Expr>,
    },
    Cmp {
        op: CmpOp,
        kind: CmpKind,
        a: Box<Expr>,
        b: Box<Expr>,
    },
    StrOp(StrOp, Box<Expr>, Box<Expr>),
    Matches(Box<Expr>, RegexLit),
    Not(Box<Expr>),
    Defined(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    StrToBool(Box<Expr>),
    Of {
        quant: Quant,
        set: OfSet,
        tail: OfTail,
    },
    For {
        quant: Quant,
        iter: Iter,
        slot: u8,
        body: Box<Expr>,
    },
    RuleRef(String),
    /// 记名弃权：求值恒 `Unknown`。
    Abstain(String),
}

/// 二元算术/位运算算子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

/// 比较类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpKind {
    Int,
    Float,
    Str,
}

/// 字符串算子（`matches` 单独走 [`Expr::Matches`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrOp {
    Contains,
    IContains,
    StartsWith,
    IStartsWith,
    EndsWith,
    IEndsWith,
    IEquals,
}
