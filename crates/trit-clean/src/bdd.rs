//! BDD 交叉校验：同一份 CNF 用 `biodivine-lib-bdd` 独立计数，与 DPLL 对拍。
//!
//! 为什么要两条链：DPLL 计数与 BDD 计数共享编码、但不共享算法与数据结构；
//! 两边必须给出同一个数，否则说明编码或计数器有 bug（响亮失败，不许静默）。
//! BDD 节点数超过 [`BDD_MAX_NODES`] 时**跳过**交叉校验并在报告里标 `None`——
//! “没校验到”与“校验通过”是两种状态，调用方必须能区分。

use biodivine_lib_bdd::BddVariableSet;

use crate::dsl::Rule;
use crate::repair::{Fill, RepairError, encode_plan};
use crate::value::Table;

/// BDD 节点预算：超过就跳过交叉校验（不是判错，是“没有校验到”）。
pub const BDD_MAX_NODES: usize = 200_000;

/// 交叉校验报告。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crosscheck {
    /// DPLL 计数。
    pub dpll: u128,
    /// BDD 计数；`None` = 预算内没算出来（没校验到）。
    pub bdd: Option<u128>,
    /// BDD 节点数（诊断用）。
    pub bdd_nodes: Option<usize>,
}

impl Crosscheck {
    /// 两条链是否一致；`None` = 没有校验到。
    #[must_use]
    pub fn agrees(&self) -> Option<bool> {
        self.bdd.map(|bdd| bdd == self.dpll)
    }
}

/// 对同一份编码跑 DPLL 与 BDD 两条计数链；不一致返回 [`RepairError::CrosscheckMismatch`]。
pub fn crosscheck(
    table: &Table,
    rules: &[Rule],
    fills: &[Fill],
) -> Result<Crosscheck, RepairError> {
    let (cnf, total) = encode_plan(table, rules, fills)?;
    let dpll = trit_count::dpll::count_cnf(&cnf).count;
    if dpll > total {
        return Err(RepairError::EncodingBroken {
            counted: dpll,
            total,
        });
    }
    if cnf.nvars == 0 {
        // 没有自由变量：结果由编码本身决定（空表为 1，含空子句为 0）。
        let bdd = u128::from(cnf.clauses.is_empty());
        if bdd != dpll {
            return Err(RepairError::CrosscheckMismatch { dpll, bdd });
        }
        return Ok(Crosscheck {
            dpll,
            bdd: Some(bdd),
            bdd_nodes: Some(0),
        });
    }
    let names: Vec<String> = (0..cnf.nvars).map(|i| format!("v{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let set = BddVariableSet::new(&refs);
    let mut acc = set.mk_true();
    for clause in &cnf.clauses {
        let mut disjunction = set.mk_false();
        for &literal in clause {
            let index = (literal.unsigned_abs() as usize) - 1;
            let variable = set
                .var_by_name(&names[index])
                .expect("BDD 变量按同一份名字表创建");
            let mut atom = set.mk_var(variable);
            if literal < 0 {
                atom = atom.not();
            }
            disjunction = disjunction.or(&atom);
        }
        acc = acc.and(&disjunction);
        if acc.size() > BDD_MAX_NODES {
            return Ok(Crosscheck {
                dpll,
                bdd: None,
                bdd_nodes: Some(acc.size()),
            });
        }
    }
    let cardinality = acc.exact_cardinality();
    let bdd = u128::try_from(cardinality).map_err(|_| RepairError::Overflow)?;
    if bdd > total {
        return Err(RepairError::EncodingBroken {
            counted: bdd,
            total,
        });
    }
    if bdd != dpll {
        return Err(RepairError::CrosscheckMismatch { dpll, bdd });
    }
    Ok(Crosscheck {
        dpll,
        bdd: Some(bdd),
        bdd_nodes: Some(acc.size()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::CmpOp;
    use crate::value::{Column, Ty, Value};

    fn table(rows: Vec<Vec<Value>>) -> Table {
        let mut t = Table::new(vec![
            Column::new("age", Ty::Int),
            Column::new("tag", Ty::Text),
        ]);
        for r in rows {
            t.push_row(r).unwrap();
        }
        t
    }

    #[test]
    fn bdd_and_dpll_agree_without_free_variables() {
        let t = table(vec![vec![Value::Int(1), Value::Text("a".into())]]);
        let rules = vec![Rule::NotNull { col: 0 }];
        let report = crosscheck(&t, &rules, &[]).unwrap();
        assert_eq!(report.dpll, 1);
        assert_eq!(report.agrees(), Some(true));
    }

    #[test]
    fn bdd_and_dpll_agree_on_check_and_unique() {
        let t = table(vec![
            vec![Value::Null, Value::Text("a".into())],
            vec![Value::Int(2), Value::Text("b".into())],
        ]);
        let rules = vec![
            Rule::Check {
                col: 0,
                op: CmpOp::Ge,
                lit: Value::Int(2),
            },
            Rule::Unique { col: 0 },
        ];
        let fill = Fill::new(0, 0, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let report = crosscheck(&t, &rules, std::slice::from_ref(&fill)).unwrap();
        assert_eq!(report.dpll, 1, "只有填 3 这一种一致完成");
        assert_eq!(report.agrees(), Some(true));
        assert!(report.bdd_nodes.is_some());
    }
}
