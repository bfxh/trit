//! BDD 模型计数：在合法导轨域上数「约束成立 ∧ 表达式取目标值」的三值赋值。
//!
//! 与等价判定共用 dual-rail 编码，但结论走 `Bdd::exact_cardinality`，
//! 与 SAT 路径、穷举真值表是三条互相独立的判定链。

use std::collections::HashSet;

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::limits::{Limits, check_expr};
use trit_core::value::Ternary;

use crate::encode::{Encoder, Node};
use crate::order::merge_order;
use crate::{BddError, BddRequest};

/// 数出 `vars` 域上，满足全部 `constraints`（每条都必须为 `True`）
/// 且 `expr` 取 `target` 的赋值数。
///
/// 没有出现在表达式里的变量同样计入域（三值变量贡献 3、二值变量贡献 2）；
/// 结果超过 `u128` 时返回 [`BddError::CountTooLarge`]，不环绕。
pub fn count_models_bdd(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary_vars: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
    request: &BddRequest<'_>,
) -> Result<u128, BddError> {
    check_expr(expr, Limits::default())?;
    for constraint in constraints {
        check_expr(constraint, Limits::default())?;
    }

    let mut all: Vec<String> = Vec::with_capacity(vars.len());
    for name in vars {
        if !all.contains(name) {
            all.push(name.clone());
        }
    }
    let ordered = match request.order {
        Some(order) => merge_order(&all, order),
        None => all,
    };

    let enc = Encoder::new(alg, &ordered, binary_vars, request.limits)?;
    let mut acc = rail_value(&enc.encode(expr)?, target);
    for constraint in constraints {
        let node = enc.encode(constraint)?;
        let holds = enc.guard(node.t.and(&node.x.not()))?;
        acc = enc.guard(acc.and(&holds))?;
    }
    let bdd = enc.guard(enc.valid()?.and(&acc))?;

    // `exact_cardinality` 按整个变量集计数：终端节点记的是变量总数，
    // 未被引用的导轨变量自动按 2 的自由度乘进去。
    let cardinality = bdd.exact_cardinality();
    u128::try_from(cardinality).map_err(|_| BddError::CountTooLarge)
}

/// 把「节点等于该目标值」写成 BDD 约束。
fn rail_value(node: &Node, target: Ternary) -> biodivine_lib_bdd::Bdd {
    match target {
        Ternary::True => node.t.and(&node.x.not()),
        Ternary::False => node.t.not().and(&node.x.not()),
        Ternary::Unknown => node.t.not().and(&node.x),
    }
}
