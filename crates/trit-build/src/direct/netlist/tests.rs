//! 网表 lowering 的形状判据：Buf 别名、Xor2 单独成算子、只降选中输出、
//! 以及网表不合法时的显式报错。

use super::*;
use trit_circuit::parse_verilog;

fn vars(circuit: &Circuit) -> Vec<String> {
    circuit
        .inputs
        .iter()
        .map(|signal| signal.name().to_string())
        .collect()
}

#[test]
fn buf_is_an_alias_not_a_statement() {
    let circuit = parse_verilog(
        "module m(input a, input b, output y);\n  wire t;\n  buf g1 (t, a);\n  assign y = t & b;\nendmodule\n",
    )
    .expect("解析");
    let ops = lower(&circuit, "y", &vars(&circuit)).expect("降级");
    assert_eq!(ops, vec![Op::Var(0), Op::Var(1), Op::And(0, 1)], "{ops:?}");
}

#[test]
fn verilog_xor_becomes_classic_xor2() {
    let circuit =
        parse_verilog("module m(input a, input b, output y);\n  assign y = a ^ b;\nendmodule\n")
            .expect("解析");
    let ops = lower(&circuit, "y", &vars(&circuit)).expect("降级");
    assert_eq!(ops, vec![Op::Var(0), Op::Var(1), Op::Xor2(0, 1)], "{ops:?}");
}

#[test]
fn only_the_selected_output_cone_is_lowered() {
    let circuit = parse_verilog(
        "module m(input a, input b, output x, output y);\n  assign x = a & b;\n  assign y = a | b;\nendmodule\n",
    )
    .expect("解析");
    let vars = vars(&circuit);
    let x = lower(&circuit, "x", &vars).expect("降级 x");
    let y = lower(&circuit, "y", &vars).expect("降级 y");
    assert_eq!(x, vec![Op::Var(0), Op::Var(1), Op::And(0, 1)], "{x:?}");
    assert_eq!(y, vec![Op::Var(0), Op::Var(1), Op::Or(0, 1)], "{y:?}");
}

#[test]
fn duplicate_driver_is_rejected() {
    let mut circuit = Circuit::new("m");
    circuit.add_input(trit_circuit::Signal::new("a"));
    circuit.add_output(trit_circuit::Signal::new("y"));
    circuit.add_gate(Gate::new(
        GateKind::And,
        vec![
            trit_circuit::Signal::new("a"),
            trit_circuit::Signal::new("a"),
        ],
        trit_circuit::Signal::new("y"),
    ));
    circuit.add_gate(Gate::new(
        GateKind::Or,
        vec![
            trit_circuit::Signal::new("a"),
            trit_circuit::Signal::new("a"),
        ],
        trit_circuit::Signal::new("y"),
    ));
    let error = lower(&circuit, "y", &["a".to_string()]).unwrap_err();
    assert!(error.to_string().contains("多个驱动"), "{error}");
}

#[test]
fn undriven_signal_is_rejected() {
    let circuit =
        parse_verilog("module m(input a, output y);\n  assign y = a & missing;\nendmodule\n");
    // 解析器本身会拒绝未声明信号；这里只钉住 lower 的兜底错误。
    if let Ok(circuit) = circuit {
        let error = lower(&circuit, "y", &vars(&circuit)).unwrap_err();
        assert!(error.to_string().contains("没有驱动门"), "{error}");
    }
}
