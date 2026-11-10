//! 双导轨 Verilog 导出（M2）：每个三值信号 → `_p` / `_n` 两根位线。
//!
//! 编码：`False = (0,1)`、`Unknown = (0,0)`、`True = (1,0)`——未知就是"两根都没拉高"。
//! K3 用手写布尔式（最少门）；Ł3/B3 从该语义的 3×3 真值表**合成 DNF**——两轨仍是
//! 两根布尔线，但蕴含/常量/毒值规则由代数决定，不再"猜"成 K3。
//!
//! 生成的是**门级结构**（每节点一对 wire），不是查表 ROM：这样读者能对上逻辑式。

use anyhow::Result;
use trit_core::value::Ternary;
use trit_core::{Algebra, Expr};

struct Gen {
    assigns: Vec<String>,
    wires: usize,
    alg: Algebra,
}

impl Gen {
    fn new(alg: Algebra) -> Self {
        Self {
            assigns: Vec::new(),
            wires: 0,
            alg,
        }
    }

    fn wire(&mut self, pos: String, neg: String) -> (String, String) {
        let id = self.wires;
        self.wires += 1;
        let (pos_name, neg_name) = (format!("t{id}_p"), format!("t{id}_n"));
        self.assigns.push(format!("  assign {pos_name} = {pos};"));
        self.assigns.push(format!("  assign {neg_name} = {neg};"));
        (pos_name, neg_name)
    }

    /// 某根三值输入线取指定值时的选择信号（合法编码下三者互斥）。
    fn select(rail: &(String, String), value: Ternary) -> String {
        match value {
            Ternary::False => rail.1.clone(),
            Ternary::True => rail.0.clone(),
            Ternary::Unknown => format!("~({} | {})", rail.0, rail.1),
        }
    }

    /// 按该语义的真值表，把二元算子的两根输出轨写成布尔 DNF。
    fn synthesize(
        alg: Algebra,
        op: &Expr,
        left: &(String, String),
        right: &(String, String),
    ) -> (String, String) {
        let values = [Ternary::False, Ternary::Unknown, Ternary::True];
        let mut pos_terms = Vec::new();
        let mut neg_terms = Vec::new();
        for a in values {
            for b in values {
                let result = match op {
                    Expr::And(..) => alg.and(a, b),
                    Expr::Or(..) => alg.or(a, b),
                    Expr::Xor(..) => alg.strong_add(a, b),
                    Expr::Implies(..) => alg.implies(a, b),
                    Expr::Equiv(..) => alg.equiv(a, b),
                    _ => unreachable!("synthesize 只收二元算子"),
                };
                let term = format!("({} & {})", Self::select(left, a), Self::select(right, b));
                match result {
                    Ternary::True => pos_terms.push(term),
                    Ternary::False => neg_terms.push(term),
                    Ternary::Unknown => {}
                }
            }
        }
        let join = |terms: Vec<String>| {
            if terms.is_empty() {
                "1'b0".to_string()
            } else {
                terms.join(" | ")
            }
        };
        (join(pos_terms), join(neg_terms))
    }

    fn binary(
        &mut self,
        expr: &Expr,
        left: &(String, String),
        right: &(String, String),
    ) -> (String, String) {
        if self.alg != Algebra::Kleene {
            let rails = Self::synthesize(self.alg, expr, left, right);
            return self.wire(rails.0, rails.1);
        }
        let (lp, ln) = (left.0.clone(), left.1.clone());
        let (rp, rn) = (right.0.clone(), right.1.clone());
        match expr {
            Expr::And(..) => self.wire(format!("({lp} & {rp})"), format!("({ln} | {rn})")),
            Expr::Or(..) => self.wire(format!("({lp} | {rp})"), format!("({ln} & {rn})")),
            // `Expr::Xor` 在本仓是**强加法 ⊕**，不是布尔异或：K3/B3 下任意一侧 Unknown
            // 就出 Unknown（`F ⊕ U = U`，而格运算 `F ∧ U = F`）。所以负轨要先排除未知。
            Expr::Xor(..) => {
                let truth = format!("({lp} & {rp})");
                self.wire(
                    truth.clone(),
                    format!("((~{truth}) & ({lp} | {ln}) & ({rp} | {rn}))"),
                )
            }
            // K3 蕴含 `a ⇒ b ≡ ¬a ∨ b`。
            Expr::Implies(..) => self.wire(format!("({ln} | {rp})"), format!("({lp} & {rn})")),
            // K3 等价 `(a ∧ b) ∨ (¬a ∧ ¬b)`。
            Expr::Equiv(..) => self.wire(
                format!("(({lp} & {rp}) | ({ln} & {rn}))"),
                format!("(({lp} & {rn}) | ({ln} & {rp}))"),
            ),
            _ => unreachable!("binary 只收二元算子"),
        }
    }

    fn node(&mut self, expr: &Expr) -> Result<(String, String)> {
        Ok(match expr {
            Expr::Const(Ternary::True) => ("1'b1".into(), "1'b0".into()),
            Expr::Const(Ternary::False) => ("1'b0".into(), "1'b1".into()),
            Expr::Const(Ternary::Unknown) => ("1'b0".into(), "1'b0".into()),
            Expr::Var(name) => (format!("{name}_p"), format!("{name}_n")),
            Expr::Not(inner) => {
                let (pos, neg) = self.node(inner)?;
                (neg, pos)
            }
            Expr::And(left, right) => {
                let left = self.node(left)?;
                let right = self.node(right)?;
                self.binary(expr, &left, &right)
            }
            Expr::Or(left, right) => {
                let left = self.node(left)?;
                let right = self.node(right)?;
                self.binary(expr, &left, &right)
            }
            Expr::Xor(left, right) => {
                let left = self.node(left)?;
                let right = self.node(right)?;
                self.binary(expr, &left, &right)
            }
            Expr::Implies(left, right) => {
                let left = self.node(left)?;
                let right = self.node(right)?;
                self.binary(expr, &left, &right)
            }
            Expr::Equiv(left, right) => {
                let left = self.node(left)?;
                let right = self.node(right)?;
                self.binary(expr, &left, &right)
            }
        })
    }
}

/// 生成双导轨模块；`module` 是模块名，`vars` 决定端口顺序。
pub fn emit(expr: &Expr, vars: &[String], module: &str, alg: Algebra) -> Result<String> {
    let mut generator = Gen::new(alg);
    let (pos, neg) = generator.node(expr)?;
    let mut text = String::new();
    text.push_str(&format!(
        "// generated by trit-build (two-rail, {})\n",
        alg.as_str()
    ));
    text.push_str(&format!("module {module}(\n"));
    for var in vars {
        text.push_str(&format!("  input wire {var}_p, {var}_n,\n"));
    }
    text.push_str("  output wire y_p, y_n\n);\n\n");
    // 内部 wire 声明：表达式本身就是变量/常量时没有内部节点，不声明空 wire。
    if generator.wires > 0 {
        text.push_str("  wire t0_p, t0_n");
        for id in 1..generator.wires {
            text.push_str(&format!(", t{id}_p, t{id}_n"));
        }
        text.push_str(";\n\n");
    }
    for line in &generator.assigns {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&format!(
        "\n  assign y_p = {pos};\n  assign y_n = {neg};\nendmodule\n"
    ));
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitplan;
    use std::collections::BTreeMap;
    use trit_circuit::{parse_verilog, propagate};
    use trit_core::Parser;

    const ALGEBRAS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];

    fn rail(value: Ternary) -> (Ternary, Ternary) {
        match value {
            Ternary::False => (Ternary::False, Ternary::True),
            Ternary::Unknown => (Ternary::False, Ternary::False),
            Ternary::True => (Ternary::True, Ternary::False),
        }
    }

    fn decode(pos: Ternary, neg: Ternary) -> (Ternary, bool) {
        (
            if pos == Ternary::True {
                Ternary::True
            } else if neg == Ternary::True {
                Ternary::False
            } else {
                Ternary::Unknown
            },
            pos == Ternary::True && neg == Ternary::True,
        )
    }

    /// 生成的 `design.v` 必须能被独立的 Verilog 前端读回，并逐行复现真值表。
    fn check_export(source: &str, alg: Algebra) {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let module = emit(&expr, &vars, "m", alg).expect("导出");
        assert!(
            module.contains(&format!("two-rail, {}", alg.as_str())),
            "{module}"
        );
        let circuit = parse_verilog(&module).expect("回读 design.v");
        let expected = bitplan::table(&expr, &vars, alg, 1).expect("真值表");
        for (code, want) in expected.iter().enumerate() {
            let mut assignment = BTreeMap::new();
            let mut rest = code;
            for var in &vars {
                let digit = rest % 3;
                rest /= 3;
                let (pos, neg) = rail(match digit {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                });
                assignment.insert(format!("{var}_p"), pos);
                assignment.insert(format!("{var}_n"), neg);
            }
            let propagation = propagate(&circuit, &assignment, alg).expect("仿真");
            let pos = propagation.outputs["y_p"];
            let neg = propagation.outputs["y_n"];
            let (got, illegal) = decode(pos, neg);
            assert!(
                !illegal,
                "{source} @ {alg:?} code={code} 出现 (1,1) 非法导轨"
            );
            assert_eq!(got, *want, "{source} @ {alg:?} code={code}");
        }
    }

    #[test]
    fn every_algebra_round_trips_through_the_verilog_frontend() {
        for alg in ALGEBRAS {
            for source in [
                "(a & b) | !c",
                "!(a | (b & !c))",
                "a => b",
                "a <=> b",
                "a ^ b",
                "(a => b) & (b => a)",
                "!((a | b) & (c => a))",
                "X => a",
                "1 & a",
                "0 | b",
            ] {
                check_export(source, alg);
            }
        }
    }

    #[test]
    fn bare_variable_has_no_internal_wires_for_any_algebra() {
        for alg in ALGEBRAS {
            let expr = Parser::parse("a").unwrap();
            let module = emit(&expr, &["a".into()], "m", alg).unwrap();
            assert!(!module.contains("wire t"), "{alg:?}: {module}");
            assert!(module.contains("assign y_p = a_p;"), "{module}");
            assert!(module.contains("assign y_n = a_n;"), "{module}");
        }
    }

    #[test]
    fn lukasiewicz_implication_is_not_kleene_material() {
        // Ł3 下 U ⇒ U = 1；K3 下是 U。导出的两轨必须跟着代数走。
        for (alg, expected) in [
            (Algebra::Kleene, Ternary::Unknown),
            (Algebra::Lukasiewicz, Ternary::True),
            (Algebra::Bochvar, Ternary::Unknown),
        ] {
            let expr = Parser::parse("a => b").unwrap();
            let module = emit(&expr, &["a".into(), "b".into()], "m", alg).unwrap();
            let circuit = parse_verilog(&module).unwrap();
            let mut assignment = BTreeMap::new();
            for var in ["a", "b"] {
                assignment.insert(format!("{var}_p"), Ternary::False);
                assignment.insert(format!("{var}_n"), Ternary::False);
            }
            let propagation = propagate(&circuit, &assignment, alg).unwrap();
            let (got, illegal) = decode(propagation.outputs["y_p"], propagation.outputs["y_n"]);
            assert!(!illegal);
            assert_eq!(got, expected, "{alg:?}");
        }
    }
}
