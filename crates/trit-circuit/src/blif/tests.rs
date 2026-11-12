//! blif 的单元测试（从 src/blif.rs 拆出，控制单文件行数预算）。

use super::*;

#[test]
fn export_validates_structure_and_identifiers() {
    // 复审 L03：含空白的信号名曾直接拼进文本，导出不可解析。
    let mut bad_name = Circuit::new("m");
    bad_name.add_input(Signal::new("a b"));
    bad_name.add_output(Signal::new("y"));
    bad_name.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new("a b")],
        Signal::new("y"),
    ));
    let actual = to_blif(&bad_name);
    assert!(
        matches!(&actual, Err(BlifError::Name(name)) if name == "a b"),
        "空白名必须拒绝，实际 {actual:?}"
    );

    let mut dot_name = Circuit::new("m");
    dot_name.add_input(Signal::new(".names"));
    dot_name.add_output(Signal::new("y"));
    dot_name.add_gate(Gate::new(
        GateKind::Buf,
        vec![Signal::new(".names")],
        Signal::new("y"),
    ));
    assert!(matches!(to_blif(&dot_name), Err(BlifError::Name(_))));

    // 元数错误：AND 只给一个输入。
    let mut wrong_arity = Circuit::new("m");
    wrong_arity.add_input(Signal::new("a"));
    wrong_arity.add_output(Signal::new("y"));
    wrong_arity.add_gate(Gate::new(
        GateKind::And,
        vec![Signal::new("a")],
        Signal::new("y"),
    ));
    assert!(
        matches!(to_blif(&wrong_arity), Err(BlifError::Structure(_))),
        "非法元数必须拒绝"
    );
}

#[test]
fn parser_rejects_structurally_invalid_blif() {
    assert!(matches!(
        parse_blif(
            ".model m
.inputs a
.outputs y
.end
"
        ),
        Err(BlifError::Structure(_))
    ));
}

#[test]
fn budgets_reject_oversized_blif() {
    // 复审 M11：cover 行与网络/门数原先无预算。
    let text = ".model m\n.inputs a b\n.outputs y\n.names a b y\n11 1\n.end\n";
    assert!(parse_blif(text).is_ok(), "默认预算内必须能解析");
    let tiny_bytes = Limits {
        bytes: 8,
        ..Limits::DEFAULT
    };
    assert!(matches!(
        parse_blif_with_limits(text, tiny_bytes),
        Err(BlifError::Limit(_))
    ));
    let no_covers = Limits {
        cover_terms: 0,
        ..Limits::DEFAULT
    };
    assert!(matches!(
        parse_blif_with_limits(text, no_covers),
        Err(BlifError::Limit(_))
    ));
    let one_net = Limits {
        nets: 1,
        ..Limits::DEFAULT
    };
    assert!(matches!(
        parse_blif_with_limits(text, one_net),
        Err(BlifError::Limit(_))
    ));
}

use crate::circuit::propagate;
use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

fn check(circuit: &Circuit) {
    let text = to_blif(circuit).unwrap();
    let again = parse_blif(&text).unwrap();
    assert_eq!(again.inputs.len(), circuit.inputs.len());
    assert_eq!(again.outputs.len(), circuit.outputs.len());
    // BLIF 是二值格式：只对 0/1 激励做等价断言。
    let total = 1u64 << circuit.input_count();
    for combo in 0..total {
        let mut assignment = std::collections::BTreeMap::new();
        for (bit, input) in circuit.inputs.iter().enumerate() {
            let value = if (combo >> bit) & 1 == 0 {
                Ternary::False
            } else {
                Ternary::True
            };
            assignment.insert(input.name().to_string(), value);
        }
        let left = propagate(circuit, &assignment, Algebra::Kleene).unwrap();
        let right = propagate(&again, &assignment, Algebra::Kleene).unwrap();
        for output in &circuit.outputs {
            assert_eq!(
                left.signals.get(output.name()),
                right.signals.get(output.name()),
                "输出 {} combo {combo}\n{text}",
                output.name()
            );
        }
    }
}

fn gate(kind: GateKind, inputs: &[&str], output: &str) -> Circuit {
    let mut circuit = Circuit::new("m");
    for name in inputs {
        circuit.add_input(Signal::new(*name));
    }
    circuit.add_gate(Gate::new(
        kind,
        inputs.iter().map(|name| Signal::new(*name)).collect(),
        Signal::new(output),
    ));
    circuit.add_output(Signal::new(output));
    circuit
}

#[test]
fn parses_plain_names_cover() {
    let text = ".model m\n.inputs a b\n.outputs y\n.names a b y\n11 1\n.end\n";
    let circuit = parse_blif(text).unwrap();
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a".to_string(), Ternary::True);
    assignment.insert("b".to_string(), Ternary::True);
    let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&Ternary::True));
}

#[test]
fn round_trip_all_binary_gate_kinds() {
    check(&gate(GateKind::And, &["a", "b"], "y"));
    check(&gate(GateKind::Or, &["a", "b"], "y"));
    check(&gate(GateKind::Nand, &["a", "b"], "y"));
    check(&gate(GateKind::Nor, &["a", "b"], "y"));
    check(&gate(GateKind::XorClassic, &["a", "b"], "y"));
    check(&gate(GateKind::Not, &["a"], "y"));
    check(&gate(GateKind::Buf, &["a"], "y"));
}

#[test]
fn constants_and_dont_care_rows() {
    let text = ".model m\n.inputs a\n.outputs y z\n\
                    .names a y\n- 1\n\
                    .names z\n\
                    .end\n";
    let circuit = parse_blif(text).unwrap();
    let mut assignment = std::collections::BTreeMap::new();
    assignment.insert("a".to_string(), Ternary::Unknown);
    let values = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    assert_eq!(values.signals.get("y"), Some(&Ternary::True));
    assert_eq!(values.signals.get("z"), Some(&Ternary::False));
}

#[test]
fn rejects_latch_and_strong_xor() {
    let text = ".model m\n.inputs a\n.outputs y\n.latch a y\n.end\n";
    assert!(parse_blif(text).is_err());
    let circuit = gate(GateKind::Xor, &["a", "b"], "y");
    assert_eq!(
        to_blif(&circuit).unwrap_err(),
        BlifError::Unsupported(GateKind::Xor)
    );
}

#[test]
fn rejects_bad_cover_rows() {
    let text = ".model m\n.inputs a b\n.outputs y\n.names a b y\n110 1\n.end\n";
    assert!(matches!(
        parse_blif(text).unwrap_err(),
        BlifError::Line { .. }
    ));
    let text = ".model m\n.inputs a\n.outputs y\n.names a y\n2 1\n.end\n";
    assert!(matches!(
        parse_blif(text).unwrap_err(),
        BlifError::Line { .. }
    ));
}

#[test]
fn parse_rejects_names_that_to_blif_cannot_roundtrip() {
    // M8：parse 与 to_blif 用同一套标识符规则——控制字符 / 以 `.` 开头的名字 parse 也拒，
    // 否则会返回「能解析但导不回去」的电路。
    let ctrl = ".model m\n.inputs a\u{1}b\n.outputs y\n.names a\u{1}b y\n1 1\n.end\n";
    assert!(parse_blif(ctrl).is_err(), "控制字符标识符应被拒绝");
    let dot = ".model m\n.inputs .x\n.outputs y\n.names .x y\n1 1\n.end\n";
    assert!(parse_blif(dot).is_err(), "以 . 开头的标识符应被拒绝");
}
