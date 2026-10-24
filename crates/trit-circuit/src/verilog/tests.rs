use std::collections::BTreeSet;

use crate::circuit::{Bus, Circuit, Gate, GateKind, Signal};
use trit_core::value::Ternary;

use super::*;
use crate::circuit::analyze;
use crate::netlist::{from_json, parse_netlist, to_json};
use trit_core::algebra::Algebra;

#[test]
fn parses_primitives_and_assign() {
    let text = "module m(input a, input b, output y, output z);\n\
                    wire n;\n\
                    and g1 (n, a, b);\n\
                    assign y = n;\n\
                    assign z = ~a | b;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.input_count(), 2);
    assert_eq!(circuit.outputs.len(), 2);
    let report = analyze(&circuit, Algebra::Kleene).unwrap();
    assert!(report.gate_count >= 3);
}

#[test]
fn round_trip_preserves_faults() {
    let circuit = parse_netlist(
        "input a\ninput b\nconst 1 one\nnot a na\ngate AND na b n\ngate OR n one y\noutput y\n",
    )
    .unwrap();
    let text = to_verilog(&circuit).unwrap();
    assert!(text.contains("module netlist"), "{text}");
    let again = parse_verilog(&text).unwrap();
    let left = analyze(&circuit, Algebra::Kleene).unwrap();
    let right = analyze(&again, Algebra::Kleene).unwrap();
    assert_eq!(left.faults, right.faults);
    assert_eq!(left.output_domains, right.output_domains);
}

#[test]
fn parses_xor_primitive() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    let text = "module m(input a, input b, output y);
  xor g1 (y, a, b);
endmodule
";
    let circuit = parse_verilog(text).unwrap();
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("xor g0"), "{rendered}");
    let again = parse_verilog(&rendered).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a".to_string(), Ternary::True);
    assignment.insert("b".to_string(), Ternary::True);
    let left = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    let right = propagate(&again, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(left.signals.get("y"), Some(&Ternary::False));
    assert_eq!(right.signals.get("y"), Some(&Ternary::False));
}

fn eval_assign(rhs: &str, values: &[(&str, Ternary)]) -> Ternary {
    eval_assign_with(rhs, values, Algebra::Kleene)
}

fn eval_assign_with(rhs: &str, values: &[(&str, Ternary)], alg: Algebra) -> Ternary {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    let text = format!(
        "module m(input a, input b, input c, output y);
  assign y = {rhs};
endmodule
"
    );
    let circuit = parse_verilog(&text).unwrap();
    let mut assignment: BTreeMap<String, Ternary> = circuit
        .inputs
        .iter()
        .map(|signal| (signal.name().to_string(), Ternary::False))
        .collect();
    for (name, value) in values {
        assignment.insert(name.to_string(), *value);
    }
    let result = propagate(&circuit, &assignment, alg).unwrap();
    *result.signals.get("y").unwrap()
}

#[test]
fn assign_xor_is_classic_xor() {
    use trit_core::value::Ternary::*;
    assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", True)]), False);
    assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", False)]), True);
    assert_eq!(eval_assign("a ~^ b", &[("a", True), ("b", True)]), True);
    assert_eq!(eval_assign("a ^~ b", &[("a", False), ("b", False)]), True);
    assert_eq!(
        eval_assign("a ^ b", &[("a", Unknown), ("b", True)]),
        Unknown
    );
    assert!(eval_assign("a ^ b", &[("a", Unknown), ("b", False)]).is_unknown());
}

#[test]
fn assign_xor_is_not_lukasiewicz_strong_sum() {
    use trit_core::value::Ternary::*;
    // Ł3 的强加 1 ⊕ 1 = 1，经典异或必须是 0。
    assert_eq!(
        eval_assign_with("a ^ b", &[("a", True), ("b", True)], Algebra::Lukasiewicz),
        False
    );
}

#[test]
fn assign_precedence_matches_verilog() {
    use trit_core::value::Ternary::*;
    // & 高于 ^ 高于 |
    assert_eq!(
        eval_assign("a ^ b & c", &[("a", True), ("b", True), ("c", False)]),
        True
    );
    assert_eq!(
        eval_assign("a & b | c", &[("a", True), ("b", False), ("c", False)]),
        False
    );
    assert_eq!(
        eval_assign("~(a | b) & c", &[("a", False), ("b", False), ("c", True)]),
        True
    );
}

#[test]
fn assign_literals_and_parentheses() {
    use trit_core::value::Ternary::*;
    assert_eq!(eval_assign("1'b1", &[]), True);
    assert_eq!(eval_assign("1'b0 | a", &[("a", False)]), False);
    assert_eq!(eval_assign("1'bx & a", &[("a", True)]), Unknown);
    assert_eq!(
        eval_assign("(a | b) & !c", &[("a", False), ("b", True), ("c", False)]),
        True
    );
}

#[test]
fn rejects_reversed_parentheses_without_panicking() {
    for text in [
        "module m;
and ) (;
endmodule
",
        "module m)(;
endmodule
",
        "module m;
and g1 (y, a) trailing;
endmodule
",
    ] {
        assert!(parse_verilog(text).is_err(), "{text}");
    }
}

#[test]
fn multi_input_nand_nor_are_not_left_folded() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    use trit_core::value::Ternary::*;

    let nand = parse_verilog(
        "module m(input a, input b, input c, output y);
  nand g(y, a, b, c);
endmodule
",
    )
    .unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a".to_string(), False);
    assignment.insert("b".to_string(), True);
    assignment.insert("c".to_string(), True);
    let values = propagate(&nand, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&True), "NAND3 应先与后非");

    let nor = parse_verilog(
        "module m(input a, input b, input c, output y);
  nor g(y, a, b, c);
endmodule
",
    )
    .unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a".to_string(), True);
    assignment.insert("b".to_string(), False);
    assignment.insert("c".to_string(), False);
    let values = propagate(&nor, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&False), "NOR3 应先或后非");
}

#[test]
fn xnor_has_a_single_driver() {
    use crate::circuit::propagate;
    use trit_core::value::Ternary::*;
    let circuit = parse_verilog(
        "module m(input a, input b, output y);
  xnor g(y, a, b);
endmodule
",
    )
    .unwrap();
    let outputs: Vec<&str> = circuit.gates.iter().map(|g| g.output.name()).collect();
    let mut sorted = outputs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(outputs.len(), sorted.len(), "门输出不能重复驱动");
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a".to_string(), True);
    assignment.insert("b".to_string(), True);
    let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&True));
}

#[test]
fn temp_names_are_module_unique() {
    use std::collections::BTreeSet;
    let circuit = parse_verilog(
        "module m(input a, input b, output y, output z);
  assign y = ~(a & b);
  assign z = ~(a | b);
endmodule
",
    )
    .unwrap();
    let mut seen = BTreeSet::new();
    for gate in &circuit.gates {
        assert!(
            seen.insert(gate.output.name()),
            "重复驱动 {}",
            gate.output.name()
        );
    }
    crate::circuit::validate(&circuit).expect("合法模块不能产生双驱动");
}

#[test]
fn strong_xor_export_is_rejected() {
    let mut circuit = Circuit::new("m");
    circuit.add_input(Signal::new("a"));
    circuit.add_input(Signal::new("b"));
    circuit.add_gate(Gate::new(
        GateKind::Xor,
        vec![Signal::new("a"), Signal::new("b")],
        Signal::new("y"),
    ));
    circuit.add_output(Signal::new("y"));
    assert!(to_verilog(&circuit).is_err(), "强加 Xor 不能导出为经典 xor");
}

#[test]
fn rejects_sequential() {
    let text = "module m(input a, output y);\nreg r;\nalways @(posedge a) r <= a;\nendmodule\n";
    assert!(parse_verilog(text).is_err());
}

#[test]
fn parses_vector_ports_and_vectorized_bitwise_assign() {
    let text = r#"
module m(input [3:0] a, input [3:0] b, output [3:0] y);
  assign y = a & b;
endmodule
"#;
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.inputs.len(), 8);
    assert_eq!(circuit.outputs.len(), 4);
    assert_eq!(circuit.gates.len(), 4);
    assert_eq!(
        circuit.inputs.iter().map(Signal::name).collect::<Vec<_>>(),
        vec![
            "a[3]", "a[2]", "a[1]", "a[0]", "b[3]", "b[2]", "b[1]", "b[0]"
        ]
    );
    assert_eq!(
        circuit.outputs.iter().map(Signal::name).collect::<Vec<_>>(),
        vec!["y[3]", "y[2]", "y[1]", "y[0]"]
    );

    let mut assignment = std::collections::BTreeMap::new();
    for (name, value) in [
        ("a[3]", Ternary::True),
        ("a[2]", Ternary::False),
        ("a[1]", Ternary::True),
        ("a[0]", Ternary::False),
        ("b[3]", Ternary::True),
        ("b[2]", Ternary::True),
        ("b[1]", Ternary::False),
        ("b[0]", Ternary::True),
    ] {
        assignment.insert(name.to_string(), value);
    }
    let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(propagated.signals.get("y[3]"), Some(&Ternary::True));
    assert_eq!(propagated.signals.get("y[2]"), Some(&Ternary::False));
    assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::False));
    assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::False));
}

#[test]
fn parses_bit_selects_and_concatenation() {
    let text = r#"
module m(input [1:0] a, input b, output [2:0] y);
  assign y = {a, b};
endmodule
"#;
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.outputs.len(), 3);
    // {a[1], a[0], b} -> y[2], y[1], y[0]
    assert_eq!(circuit.gates.len(), 3);
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a[1]".to_string(), Ternary::True);
    assignment.insert("a[0]".to_string(), Ternary::False);
    assignment.insert("b".to_string(), Ternary::True);
    let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(propagated.signals.get("y[2]"), Some(&Ternary::True));
    assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::False));
    assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::True));

    let selects = r#"
module m(input [1:0] a, output [1:0] y);
  assign y[0] = a[1];
  assign y[1] = ~a[0];
endmodule
"#;
    let circuit = parse_verilog(selects).unwrap();
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a[1]".to_string(), Ternary::True);
    assignment.insert("a[0]".to_string(), Ternary::False);
    let propagated = crate::circuit::propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(propagated.signals.get("y[0]"), Some(&Ternary::True));
    assert_eq!(propagated.signals.get("y[1]"), Some(&Ternary::True));
}

#[test]
fn vector_round_trip_regroups_ports() {
    let text = r#"
module m(input [3:0] a, input [3:0] b, output [3:0] y);
  assign y = a & b;
endmodule
"#;
    let circuit = parse_verilog(text).unwrap();
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("input [3:0] a;"), "{rendered}");
    assert!(rendered.contains("input [3:0] b;"), "{rendered}");
    assert!(rendered.contains("output [3:0] y;"), "{rendered}");
    let again = parse_verilog(&rendered).unwrap();
    assert_eq!(again, circuit);
}

#[test]
fn vector_errors_are_classified() {
    for text in [
        // 只支持 [N:0] 或 [0:N]，非零下界拒绝
        "module m(input [2:5] a, output y);\nassign y = a[2];\nendmodule\n",
        // 不支持复制语法
        "module m(input a, output [1:0] y);\nassign y = {2{a}};\nendmodule\n",
        // 位宽不匹配
        "module m(input [3:0] a, output [1:0] y);\nassign y = a;\nendmodule\n",
        // 标量目标不能接向量
        "module m(input [1:0] a, output y);\nassign y = a;\nendmodule\n",
        // 位选越界
        "module m(input [1:0] a, output [1:0] y);\nassign y[2] = a[0];\nendmodule\n",
    ] {
        assert!(parse_verilog(text).is_err(), "应拒绝：{text}");
    }

    // 手工电路里只有 t[1]，无法导出为连续向量
    let mut circuit = Circuit::new("m");
    circuit.add_input(Signal::new("t[1]"));
    circuit.add_output(Signal::new("t[1]"));
    assert!(to_verilog(&circuit).is_err(), "非连续向量位必须拒绝导出");
}

#[test]
fn deep_rhs_is_rejected() {
    let deep_parens = format!(
        "module m(input a, output y);\nassign y = {}a{};\nendmodule\n",
        "(".repeat(300),
        ")".repeat(300)
    );
    assert!(parse_verilog(&deep_parens).is_err());
    let deep_nots = format!(
        "module m(input a, output y);\nassign y = {}a;\nendmodule\n",
        "!".repeat(300)
    );
    assert!(parse_verilog(&deep_nots).is_err());
    let ok = format!(
        "module m(input a, output y);\nassign y = {}a{};\nendmodule\n",
        "(".repeat(100),
        ")".repeat(100)
    );
    assert!(parse_verilog(&ok).is_ok(), "合理嵌套必须仍可解析");
}

#[test]
fn vector_width_and_input_size_are_bounded() {
    let too_wide = "module m(input [5000:0] a, output y);\nassign y = a[0];\nendmodule\n";
    assert!(parse_verilog(too_wide).is_err(), "超宽向量必须拒绝");

    let mut oversized = String::from("module m(input a, output y);\nassign y = a;\nendmodule\n");
    oversized.push_str(&" ".repeat(super::parse::MAX_VERILOG_BYTES));
    assert!(parse_verilog(&oversized).is_err(), "超大文本必须拒绝");
}

#[test]
fn errors_carry_kind_and_line() {
    let err = parse_verilog(
        "module m(input a, output y);
  assign y = ;
endmodule
",
    )
    .unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Syntax, "{err:?}");
    assert_eq!(err.line, Some(2), "{err:?}");

    // body 声明在第二行：预算错误应带行号。
    let err = parse_verilog(
        "module m(a, y);
  input [5000:0] a;
  output y;
  assign y = a[0];
endmodule
",
    )
    .unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Limit, "{err:?}");
    assert_eq!(err.line, Some(2), "{err:?}");

    let err = parse_verilog(
        "module m(input [2:5] a, output y);
assign y = a[2];
endmodule
",
    )
    .unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Unsupported, "{err:?}");

    let mut circuit = Circuit::new("m");
    circuit.add_output(Signal::new("y"));
    assert_eq!(
        to_verilog(&circuit).unwrap_err().kind,
        VerilogErrorKind::Export
    );
}

#[test]
fn vector_declarations_are_recorded_as_buses() {
    let text = "module m(input [3:0] a, output [1:0] y);\n\
                assign y = {a[1], a[0]};\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("a"), Some(&Bus::new("a", 4)));
    assert_eq!(circuit.buses.get("y"), Some(&Bus::new("y", 2)));
    assert_eq!(circuit.buses.len(), 2);
}

#[test]
fn bus_metadata_survives_json_round_trip() {
    let text = "module m(input [3:0] a, output [1:0] y);\n\
                assign y = {a[1], a[0]};\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let json = to_json(&circuit).unwrap();
    let back = from_json(&json).unwrap();
    assert_eq!(back, circuit);
}

fn output_domains(text: &str, outputs: &[(&str, bool)]) {
    let circuit = parse_verilog(text).unwrap();
    let report = analyze(&circuit, Algebra::Kleene).unwrap();
    for (name, expected) in outputs {
        let expected = if *expected {
            Ternary::True
        } else {
            Ternary::False
        };
        assert_eq!(
            report.output_domains[*name],
            BTreeSet::from([expected]),
            "{name}: {text}"
        );
    }
}

#[test]
fn sized_literals_preserve_bit_order() {
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'b1010;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", false),
            ("y[3]", true),
        ],
    );
}

#[test]
fn hex_decimal_and_underscored_literals() {
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'hA;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", false),
            ("y[3]", true),
        ],
    );
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'd9;\nendmodule\n",
        &[
            ("y[0]", true),
            ("y[1]", false),
            ("y[2]", false),
            ("y[3]", true),
        ],
    );
    // 8 位二进制：截断到低 4 位 = 0110。
    output_domains(
        "module m(input a, output [3:0] y);\nassign y = 4'b1010_0110;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", true),
            ("y[3]", false),
        ],
    );
}

#[test]
fn sized_literal_zero_extends_to_wider_vector() {
    output_domains(
        "module m(input a, output [7:0] y);\nassign y = 8'hA;\nendmodule\n",
        &[
            ("y[0]", false),
            ("y[1]", true),
            ("y[2]", false),
            ("y[3]", true),
            ("y[7]", false),
        ],
    );
}

#[test]
fn logical_and_or_are_rejected_instead_of_bitwise() {
    // 复审 M02：`&&` / `||` 曾被静默当成逐位 `&` / `|`，向量场景语义直接错。
    for text in [
        "module m(input a, input b, output y);
assign y = a && b;
endmodule
",
        "module m(input a, input b, output y);
assign y = a || b;
endmodule
",
    ] {
        let err = parse_verilog(text).unwrap_err();
        assert_eq!(err.kind, VerilogErrorKind::Unsupported, "{err:?}");
        assert!(err.to_string().contains("逻辑"), "{err}");
    }
    // 逐位运算符仍然可用。
    let circuit = parse_verilog(
        "module m(input a, input b, output y);
assign y = a & b;
endmodule
",
    )
    .expect("逐位与必须仍可解析");
    assert!(circuit.gates.iter().any(|gate| gate.kind == GateKind::And));
    let circuit = parse_verilog(
        "module m(input a, input b, output y);
assign y = a | b;
endmodule
",
    )
    .expect("逐位或必须仍可解析");
    assert!(circuit.gates.iter().any(|gate| gate.kind == GateKind::Or));
}

#[test]
fn unsupported_literal_base_is_reported() {
    let err =
        parse_verilog("module m(input a, output y);\nassign y = 4'sd3;\nendmodule\n").unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Unsupported, "{err:?}");
}

#[test]
fn export_uses_declared_bus_width_for_partial_wire() {
    let mut circuit = Circuit::new("m");
    circuit.add_input(Signal::new("a"));
    circuit.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new("a")],
        Signal::new("t[3]"),
    ));
    circuit.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new("t[3]")],
        Signal::new("y"),
    ));
    circuit.add_output(Signal::new("y"));
    circuit.buses.insert("t".to_string(), Bus::new("t", 8));
    let text = to_verilog(&circuit).unwrap();
    assert!(text.contains("wire [7:0] t;"), "{text}");
    assert!(text.contains("buf g0 (t[3], a)"), "{text}");
}

#[test]
fn deeply_nested_concat_returns_limit_not_stack_overflow() {
    // 复审 H01：拼接是递归文法分支，2 万层嵌套曾直接栈溢出（abort 134）。
    let mut text = String::from("module m(input a, output y);\nassign y = ");
    for _ in 0..20_000 {
        text.push('{');
    }
    text.push('a');
    for _ in 0..20_000 {
        text.push('}');
    }
    text.push_str(";\nendmodule\n");
    let err = parse_verilog(&text).unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Limit, "{err:?}");
}

#[test]
fn long_operator_chain_returns_limit_not_stack_overflow() {
    // 复审 H01：平链构造成左深 AST，合成阶段递归遍历曾栈溢出。
    // 用「上限 +1」项：护栏阈值被放大或禁用的任何变异都会立刻变绿暴露，
    // 且这个深度本身能安全综合（不依赖栈溢出 abort 才判红）。
    let chain = std::iter::repeat_n("a & ", super::expr::MAX_RHS_OPS + 1).collect::<String>();
    let text = format!("module m(input a, output y);\nassign y = {chain}a;\nendmodule\n");
    let err = parse_verilog(&text).unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Limit, "{err:?}");
}

#[test]
fn chains_within_the_op_budget_still_parse() {
    // 护栏不该拦住正常宽度的链：刚好不超限的 & 链要能解析并综合。
    let chain = std::iter::repeat_n("a & ", super::expr::MAX_RHS_OPS - 1).collect::<String>();
    let text = format!("module m(input a, output y);\nassign y = {chain}a;\nendmodule\n");
    let circuit = parse_verilog(&text).unwrap();
    // N 个运算符恰好综合出 N 个与门。
    assert_eq!(circuit.gates.len(), super::expr::MAX_RHS_OPS - 1);
}
