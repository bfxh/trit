//! 表达式抽象语法树（AST）。
//!
//! 语法（优先级从低到高）：
//!
//! ```text
//! expr    := equiv
//! equiv   := implies ( ("≡" | "==" | "<=>") implies )*
//! implies := or ( ("=>" | "→") or )*            // 右结合
//! or      := xor ( ("∨" | "|" | "||") xor )*
//! xor     := and ( ("⊕" | "^") and )*
//! and     := unary ( ("∧" | "&" | "&&") unary )*
//! unary   := ("!" | "~" | "¬") unary | primary
//! primary := "(" expr ")" | ident | const
//! const   := "0" | "1" | "X" | "x" | "?" | "True" | "False"
//! ```
//!
//! `¬` 的优先级高于 `∧`，`∧` 高于 `∨`，`∨` 高于 `⊕` 之外的混合用法由
//! 上表固定；`⊕` 夹在 `∨` 与 `∧` 之间，表示强加法。

use serde::{Deserialize, Serialize};

/// 变量名错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VarNameError {
    /// 空名字。
    #[error("变量名不能为空")]
    Empty,
    /// 不是合法标识符。
    #[error("变量名 {0:?} 不是合法标识符")]
    Invalid(String),
    /// 与常量 token 冲突（`X`、`True` 等在解析时会被识别为常量）。
    #[error("变量名 {0:?} 是保留常量 token")]
    Reserved(String),
}

/// 是否为合法标识符（与解析器一致：首字符为字母或 `_`，其余为字母/数字/`_`）。
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_') && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// 作为变量名时会被解析成常量的保留 token。
pub fn is_reserved_token(name: &str) -> bool {
    matches!(
        name,
        "0" | "False"
            | "false"
            | "FALSE"
            | "1"
            | "True"
            | "true"
            | "TRUE"
            | "X"
            | "x"
            | "?"
            | "Unknown"
            | "unknown"
    )
}

/// 表达式节点。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Expr {
    /// 逻辑常量。
    Const(crate::value::Ternary),
    /// 变量引用。
    Var(String),
    /// 否定 `¬a`。
    Not(Box<Expr>),
    /// 与 `a ∧ b`。
    And(Box<Expr>, Box<Expr>),
    /// 或 `a ∨ b`。
    Or(Box<Expr>, Box<Expr>),
    /// 强加法 `a ⊕ b`。
    Xor(Box<Expr>, Box<Expr>),
    /// 蕴含 `a ⇒ b`（右结合）。
    Implies(Box<Expr>, Box<Expr>),
    /// 等价 `a ≡ b`。
    Equiv(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// 构造与节点。
    pub fn and(a: Expr, b: Expr) -> Self {
        Self::And(Box::new(a), Box::new(b))
    }
    /// 构造或节点。
    pub fn or(a: Expr, b: Expr) -> Self {
        Self::Or(Box::new(a), Box::new(b))
    }
    /// 构造否定节点。
    ///
    /// 命名为 `logical_not` 而非 `not`，避免与 `std::ops::Not::not` 混淆。
    pub fn logical_not(a: Expr) -> Self {
        Self::Not(Box::new(a))
    }
    /// 构造强加法节点。
    pub fn xor(a: Expr, b: Expr) -> Self {
        Self::Xor(Box::new(a), Box::new(b))
    }
    /// 构造蕴含节点。
    pub fn implies(a: Expr, b: Expr) -> Self {
        Self::Implies(Box::new(a), Box::new(b))
    }
    /// 构造等价节点。
    pub fn equiv(a: Expr, b: Expr) -> Self {
        Self::Equiv(Box::new(a), Box::new(b))
    }

    /// 校验并构造变量节点。
    ///
    /// [`Expr::Var`] 的字段是公开的，直接构造可以塞进空白、运算符或常量 token；
    /// 这类 AST 的 [`Display`](std::fmt::Display) 不保证能被 [`Parser`](crate::parser::Parser)
    /// 无损解析回来。
    pub fn try_var(name: impl Into<String>) -> Result<Self, VarNameError> {
        let name = name.into();
        if name.is_empty() {
            return Err(VarNameError::Empty);
        }
        if is_reserved_token(&name) {
            return Err(VarNameError::Reserved(name));
        }
        if !is_identifier(&name) {
            return Err(VarNameError::Invalid(name));
        }
        Ok(Self::Var(name))
    }

    /// 是否为常量 `Unknown`。
    pub const fn is_unknown_const(&self) -> bool {
        matches!(self, Self::Const(crate::value::Ternary::Unknown))
    }

    /// 收集表达式中出现的全部变量名（保持首次出现顺序）。
    ///
    /// 迭代实现，不对任意深的树递归。
    pub fn variables(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            match node {
                Self::Const(_) => {}
                Self::Var(name) => {
                    if seen.insert(name.clone()) {
                        out.push(name.clone());
                    }
                }
                Self::Not(a) => stack.push(a),
                Self::And(a, b)
                | Self::Or(a, b)
                | Self::Xor(a, b)
                | Self::Implies(a, b)
                | Self::Equiv(a, b) => {
                    stack.push(b);
                    stack.push(a);
                }
            }
        }
        out
    }

    /// 节点总数（用于复杂度统计）。迭代实现。
    pub fn node_count(&self) -> usize {
        let mut count = 0usize;
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            count += 1;
            match node {
                Self::Const(_) | Self::Var(_) => {}
                Self::Not(a) => stack.push(a),
                Self::And(a, b)
                | Self::Or(a, b)
                | Self::Xor(a, b)
                | Self::Implies(a, b)
                | Self::Equiv(a, b) => {
                    stack.push(a);
                    stack.push(b);
                }
            }
        }
        count
    }

    /// 树的最大深度（根为 1）。迭代实现。
    pub fn depth(&self) -> usize {
        let mut max = 0usize;
        let mut stack = vec![(self, 1usize)];
        while let Some((node, depth)) = stack.pop() {
            max = max.max(depth);
            match node {
                Self::Const(_) | Self::Var(_) => {}
                Self::Not(a) => stack.push((a, depth + 1)),
                Self::And(a, b)
                | Self::Or(a, b)
                | Self::Xor(a, b)
                | Self::Implies(a, b)
                | Self::Equiv(a, b) => {
                    stack.push((a, depth + 1));
                    stack.push((b, depth + 1));
                }
            }
        }
        max
    }

    /// 渲染为规范的中缀记法（带全角运算符），用于展示与化简输出。
    ///
    /// 递归实现；对未经过预算检查的手工深树，请先调用
    /// [`check_expr`](crate::limits::check_expr)。任意 [`Expr::Var`] 的文本
    /// 往返由 [`Expr::try_var`] 约束，而不是由 `Display` 本身保证。
    pub fn to_infix(&self) -> String {
        match self {
            Self::Const(v) => v.to_string(),
            Self::Var(name) => name.clone(),
            Self::Not(a) => format!("¬{}", a.to_infix()),
            Self::And(a, b) => format!("({} ∧ {})", a.to_infix(), b.to_infix()),
            Self::Or(a, b) => format!("({} ∨ {})", a.to_infix(), b.to_infix()),
            Self::Xor(a, b) => format!("({} ⊕ {})", a.to_infix(), b.to_infix()),
            Self::Implies(a, b) => format!("({} ⇒ {})", a.to_infix(), b.to_infix()),
            Self::Equiv(a, b) => format!("({} ≡ {})", a.to_infix(), b.to_infix()),
        }
    }
}

impl std::fmt::Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_infix())
    }
}

#[cfg(test)]
mod tests;
