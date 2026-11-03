//! Policy 文件读取与定义展开。
//!
//! 这里只做计数需要的事：解析 JSON、展开 `definitions`、把 `assumptions`
//! 当作约束。判定链仍走 [`crate::count_models_under`] 与穷举 oracle，不复用
//! `trit-policy` 的验证路径，以免计数参考实现和验证实现互相遮蔽。

use std::collections::{BTreeMap, HashSet};
use std::fmt::{self, Display};
use std::path::Path;

use serde::Deserialize;
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;

/// 一条已展开的断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    /// 断言名（原样保留，供报告使用）。
    pub name: String,
    /// 展开定义后的表达式。
    pub expr: Expr,
}

/// 解析并展开后的策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// 语义体系。
    pub algebra: Algebra,
    /// 已知二值的输入变量。
    pub binary: HashSet<String>,
    /// 展开后的前提；计数时每条前提都必须为 `True`。
    pub assumptions: Vec<Expr>,
    /// 展开后的断言。
    pub assertions: Vec<Assertion>,
    /// 计数域里的输入变量（排序、去重）。
    pub vars: Vec<String>,
}

/// 策略读取或展开错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// 文件读取失败。
    Io {
        /// 路径。
        path: String,
        /// 失败原因。
        reason: String,
    },
    /// JSON 结构不合法。
    Json(String),
    /// 未知语义名。
    Algebra(String),
    /// 表达式解析失败。
    Parse {
        /// 出处（definition / assumption / assertion）。
        kind: &'static str,
        /// 名称。
        name: String,
        /// 解析器原因。
        reason: String,
    },
    /// 定义名不是合法变量名。
    InvalidName {
        /// 出处。
        kind: &'static str,
        /// 名称。
        name: String,
    },
    /// 定义循环引用。
    RecursiveDefinition(String),
    /// `binary` 里的名字与定义同名。
    BinaryIsDefinition(String),
    /// 没有任何断言。
    NoAssertions,
}

impl Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, reason } => write!(f, "读取策略失败 {path}: {reason}"),
            Self::Json(reason) => write!(f, "策略 JSON 无效: {reason}"),
            Self::Algebra(name) => write!(
                f,
                "未知语义 {name:?}（可选 kleene / lukasiewicz / bochvar）"
            ),
            Self::Parse { kind, name, reason } => {
                write!(f, "{kind} {name:?} 解析失败: {reason}")
            }
            Self::InvalidName { kind, name } => {
                write!(f, "{kind} 名称不合法: {name:?}")
            }
            Self::RecursiveDefinition(name) => write!(f, "定义循环引用: {name:?}"),
            Self::BinaryIsDefinition(name) => {
                write!(f, "binary 变量名与定义同名: {name:?}")
            }
            Self::NoAssertions => write!(f, "策略没有任何断言"),
        }
    }
}

impl std::error::Error for PolicyError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    #[serde(default = "default_algebra")]
    algebra: String,
    #[serde(default)]
    binary: Vec<String>,
    #[serde(default)]
    assumptions: Vec<String>,
    #[serde(default)]
    definitions: BTreeMap<String, String>,
    assertions: Vec<RawAssertion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAssertion {
    name: String,
    expr: String,
}

fn default_algebra() -> String {
    "kleene".to_string()
}

impl Policy {
    /// 从文件读取并展开。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PolicyError> {
        let target = path.as_ref();
        let text = std::fs::read_to_string(target).map_err(|error| PolicyError::Io {
            path: target.display().to_string(),
            reason: error.to_string(),
        })?;
        Self::from_json(&text)
    }

    /// 从 JSON 文本解析并展开。
    pub fn from_json(text: &str) -> Result<Self, PolicyError> {
        let raw: RawPolicy =
            serde_json::from_str(text).map_err(|error| PolicyError::Json(error.to_string()))?;
        if raw.assertions.is_empty() {
            return Err(PolicyError::NoAssertions);
        }
        let algebra = Algebra::parse(&raw.algebra)
            .ok_or_else(|| PolicyError::Algebra(raw.algebra.clone()))?;

        let mut parsed: BTreeMap<String, Expr> = BTreeMap::new();
        for (name, source) in &raw.definitions {
            Expr::try_var(name.clone()).map_err(|_| PolicyError::InvalidName {
                kind: "definition",
                name: name.clone(),
            })?;
            parsed.insert(name.clone(), parse_expression("definition", name, source)?);
        }

        let mut expanded: BTreeMap<String, Expr> = BTreeMap::new();
        let names: Vec<String> = parsed.keys().cloned().collect();
        for name in names {
            expand_definition(&name, &parsed, &mut expanded)?;
        }

        for name in &raw.binary {
            if expanded.contains_key(name) {
                return Err(PolicyError::BinaryIsDefinition(name.clone()));
            }
            Expr::try_var(name.clone()).map_err(|_| PolicyError::InvalidName {
                kind: "binary",
                name: name.clone(),
            })?;
        }

        let assumptions = raw
            .assumptions
            .iter()
            .enumerate()
            .map(|(index, source)| {
                parse_and_expand("assumption", &index.to_string(), source, &expanded)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let assertions = raw
            .assertions
            .iter()
            .map(|assertion| {
                let expr =
                    parse_and_expand("assertion", &assertion.name, &assertion.expr, &expanded)?;
                Ok(Assertion {
                    name: assertion.name.clone(),
                    expr,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut vars: Vec<String> = Vec::new();
        for expr in assumptions.iter().chain(assertions.iter().map(|a| &a.expr)) {
            vars.extend(expr.variables());
        }
        vars.extend(raw.binary.iter().cloned());
        vars.sort();
        vars.dedup();

        Ok(Self {
            algebra,
            binary: raw.binary.into_iter().collect(),
            assumptions,
            assertions,
            vars,
        })
    }

    /// 前提全为 `True` 的赋值数。
    #[must_use]
    pub fn count_assumptions(&self) -> u128 {
        crate::count_models_under(
            &Expr::Const(trit_core::value::Ternary::True),
            &self.assumptions,
            &self.vars,
            &self.binary,
            self.algebra,
            trit_core::value::Ternary::True,
        )
        .count
    }
}

fn parse_expression(kind: &'static str, name: &str, source: &str) -> Result<Expr, PolicyError> {
    Parser::parse(source).map_err(|error| PolicyError::Parse {
        kind,
        name: name.to_string(),
        reason: error.to_string(),
    })
}

fn parse_and_expand(
    kind: &'static str,
    name: &str,
    source: &str,
    definitions: &BTreeMap<String, Expr>,
) -> Result<Expr, PolicyError> {
    let parsed = parse_expression(kind, name, source)?;
    let mut scratch = BTreeMap::new();
    let mut visiting = HashSet::new();
    substitute(&parsed, definitions, &mut scratch, &mut visiting)
}

fn expand_definition(
    name: &str,
    parsed: &BTreeMap<String, Expr>,
    expanded: &mut BTreeMap<String, Expr>,
) -> Result<Expr, PolicyError> {
    if let Some(done) = expanded.get(name) {
        return Ok(done.clone());
    }
    let raw = parsed
        .get(name)
        .ok_or_else(|| PolicyError::RecursiveDefinition(name.to_string()))?
        .clone();
    let mut visiting = HashSet::new();
    visiting.insert(name.to_string());
    let substituted = substitute(&raw, parsed, expanded, &mut visiting)?;
    expanded.insert(name.to_string(), substituted.clone());
    Ok(substituted)
}

fn substitute(
    expr: &Expr,
    definitions: &BTreeMap<String, Expr>,
    expanded: &mut BTreeMap<String, Expr>,
    visiting: &mut HashSet<String>,
) -> Result<Expr, PolicyError> {
    match expr {
        Expr::Const(value) => Ok(Expr::Const(*value)),
        Expr::Var(name) => {
            if !definitions.contains_key(name) {
                return Ok(Expr::Var(name.clone()));
            }
            if !visiting.insert(name.clone()) {
                return Err(PolicyError::RecursiveDefinition(name.clone()));
            }
            let body = definitions.get(name).expect("definition present").clone();
            let substituted = substitute(&body, definitions, expanded, visiting)?;
            visiting.remove(name);
            expanded.insert(name.clone(), substituted.clone());
            Ok(substituted)
        }
        Expr::Not(a) => Ok(Expr::Not(Box::new(substitute(
            a,
            definitions,
            expanded,
            visiting,
        )?))),
        Expr::And(a, b)
        | Expr::Or(a, b)
        | Expr::Xor(a, b)
        | Expr::Implies(a, b)
        | Expr::Equiv(a, b) => {
            let left = substitute(a, definitions, expanded, visiting)?;
            let right = substitute(b, definitions, expanded, visiting)?;
            Ok(match expr {
                Expr::And(..) => Expr::and(left, right),
                Expr::Or(..) => Expr::or(left, right),
                Expr::Xor(..) => Expr::xor(left, right),
                Expr::Implies(..) => Expr::implies(left, right),
                _ => Expr::equiv(left, right),
            })
        }
    }
}
