//! 层级实例 / 参数化 Verilog 的回归（从 `tests.rs` 拆出，见模块预算门）。

use crate::circuit::Bus;
use trit_core::value::Ternary;

use super::*;
use crate::circuit::analyze;
use crate::netlist::{from_json, to_json};
use trit_core::algebra::Algebra;

fn hier_signal(text: &str, values: &[(&str, Ternary)], name: &str) -> Ternary {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    let circuit = parse_verilog(text).unwrap();
    let assignment: BTreeMap<String, Ternary> = values
        .iter()
        .map(|(key, value)| ((*key).to_string(), *value))
        .collect();
    let propagation = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    *propagation
        .signals
        .get(name)
        .unwrap_or_else(|| panic!("缺少信号 {name}"))
}

#[test]
fn hierarchical_named_connections_can_be_reordered() {
    let text = "module top(input a, input b, output y);
  wire n;
  and g0 (n, a, b);
  child u1 (.q(y), .p(n));
endmodule
module child(input p, output q);
  not g0 (q, p);
endmodule
";
    assert_eq!(
        hier_signal(text, &[("a", Ternary::True), ("b", Ternary::True)], "y"),
        Ternary::False
    );
    assert_eq!(
        hier_signal(text, &[("a", Ternary::False), ("b", Ternary::True)], "y"),
        Ternary::True
    );
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.input_count(), 2);
    assert_eq!(circuit.gates.len(), 2);
}

#[test]
fn hierarchical_positional_connections_bind_by_port_order() {
    let text = "module top(input a, input b, output y, output z);
  child u0 (a, y);
  child u1 (b, z);
endmodule
module child(input p, output q);
  not g (q, p);
endmodule
";
    let y = hier_signal(text, &[("a", Ternary::True), ("b", Ternary::False)], "y");
    let z = hier_signal(text, &[("a", Ternary::True), ("b", Ternary::False)], "z");
    assert_eq!((y, z), (Ternary::False, Ternary::True));
}

#[test]
fn hierarchical_instances_nest_across_levels() {
    let text = "module top(input a, output y);
  mid u0 (a, y);
endmodule
module mid(input p, output q);
  leaf u0 (p, q);
endmodule
module leaf(input i, output o);
  buf g (o, i);
endmodule
";
    assert_eq!(
        hier_signal(text, &[("a", Ternary::True)], "y"),
        Ternary::True
    );
    assert_eq!(
        hier_signal(text, &[("a", Ternary::Unknown)], "y"),
        Ternary::Unknown
    );
}

#[test]
fn hierarchical_unknown_module_is_rejected() {
    let err =
        parse_verilog("module top(input a, output y);\n  child u0 (.p(a), .q(y));\nendmodule\n")
            .unwrap_err();
    assert!(err.to_string().contains("未定义的模块"), "{err}");
}

#[test]
fn hierarchical_recursive_or_rootless_design_is_rejected() {
    let err =
        parse_verilog("module top(input a, output y);\n  top u0 (.a(a), .y(y));\nendmodule\n")
            .unwrap_err();
    assert!(err.to_string().contains("找不到顶层模块"), "{err}");
}

#[test]
fn hierarchical_multiple_roots_are_rejected() {
    let err = parse_verilog(
        "module a(input x, output y);\n  buf g (y, x);\nendmodule\n\
         module b(input x, output y);\n  buf g (y, x);\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("多个顶层模块"), "{err}");
}

#[test]
fn hierarchical_missing_port_connection_is_rejected() {
    let err = parse_verilog(
        "module top(input a, output y);\n  child u0 (.p(a));\nendmodule\n\
         module child(input p, output q);\n  buf g (q, p);\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("没有全部连接"), "{err}");
}

#[test]
fn hierarchical_vector_ports_connect_whole_bus() {
    let text = "module top(input [3:0] a, output [3:0] y);
  child u0 (.p(a), .q(y));
endmodule
module child(input [3:0] p, output [3:0] q);
  assign q = ~p;
endmodule
";
    let values = [
        ("a[0]", Ternary::False),
        ("a[1]", Ternary::True),
        ("a[2]", Ternary::False),
        ("a[3]", Ternary::True),
    ];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[2]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[3]"), Ternary::False);

    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("a").map(|bus| bus.width), Some(4));
    assert_eq!(circuit.buses.get("y").map(|bus| bus.width), Some(4));
    let report = analyze(&circuit, Algebra::Kleene).unwrap();
    assert_eq!(report.input_count, 4);
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("input [3:0] a"), "{rendered}");
    assert!(rendered.contains("output [3:0] y"), "{rendered}");
}

#[test]
fn hierarchical_vector_positional_connections_work() {
    let text = "module top(input [1:0] a, output [1:0] y);
  child u0 (a, y);
endmodule
module child(input [1:0] p, output [1:0] q);
  assign q = p;
endmodule
";
    let values = [("a[0]", Ternary::True), ("a[1]", Ternary::False)];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::False);
}

#[test]
fn hierarchical_vector_bit_order_is_preserved() {
    // 输入 bus 与输出 bus 分别映射；再用一个标量输出观察 p[0]，
    // 避免“输入反向 + 输出反向”互相抵消而掩盖位序错误。
    let text = "module top(input [3:0] a, output y0, output [3:0] y);
  child u0 (.p(a), .q0(y0), .q(y));
endmodule
module child(input [3:0] p, output q0, output [3:0] q);
  assign q0 = p[0];
  assign q = p;
endmodule
";
    let values = [
        ("a[0]", Ternary::True),
        ("a[1]", Ternary::False),
        ("a[2]", Ternary::False),
        ("a[3]", Ternary::False),
    ];
    assert_eq!(hier_signal(text, &values, "y0"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    let values = [
        ("a[0]", Ternary::False),
        ("a[1]", Ternary::True),
        ("a[2]", Ternary::False),
        ("a[3]", Ternary::False),
    ];
    assert_eq!(hier_signal(text, &values, "y0"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::True);
}

#[test]
fn hierarchical_vector_and_scalar_ports_mix() {
    let text = "module top(input [1:0] a, input c, output [1:0] y, output d);
  child u0 (.p(a), .c(c), .q(y), .d(d));
endmodule
module child(input [1:0] p, input c, output [1:0] q, output d);
  assign q[0] = p[0] & c;
  assign q[1] = p[1] | c;
  assign d = ~c;
endmodule
";
    let values = [
        ("a[0]", Ternary::True),
        ("a[1]", Ternary::False),
        ("c", Ternary::False),
    ];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "d"), Ternary::True);
    let values = [
        ("a[0]", Ternary::True),
        ("a[1]", Ternary::False),
        ("c", Ternary::True),
    ];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::True);
    assert_eq!(hier_signal(text, &values, "d"), Ternary::False);
}

#[test]
fn hierarchical_internal_bus_connects_to_child_bus() {
    let text = "module top(input [1:0] a, output [1:0] y);
  wire [1:0] n;
  assign n = ~a;
  child u0 (.p(n), .q(y));
endmodule
module child(input [1:0] p, output [1:0] q);
  assign q = p;
endmodule
";
    let values = [("a[0]", Ternary::True), ("a[1]", Ternary::False)];
    assert_eq!(hier_signal(text, &values, "y[0]"), Ternary::False);
    assert_eq!(hier_signal(text, &values, "y[1]"), Ternary::True);
}

#[test]
fn hierarchical_vector_width_mismatch_is_rejected() {
    let err = parse_verilog(
        "module top(input [1:0] a, output y);\n  child u0 (.p(a), .q(y));\nendmodule\n\
         module child(input [3:0] p, output q);\n  assign q = p[0];\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("需要 4 位"), "{err}");
}

#[test]
fn hierarchical_vector_port_rejects_single_bit() {
    let err = parse_verilog(
        "module top(input [3:0] a, output y);\n  child u0 (.p(a[0]), .q(y));\nendmodule\n\
         module child(input [3:0] p, output q);\n  assign q = p[0];\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("不能连接单比特"), "{err}");
}

#[test]
fn hierarchical_scalar_port_rejects_whole_bus() {
    let err = parse_verilog(
        "module top(input [3:0] a, output y);\n  child u0 (.p(a), .q(y));\nendmodule\n\
         module child(input p, output q);\n  buf g (q, p);\nendmodule\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("标量，不能连接 4 位向量"), "{err}");
}

#[test]
fn hierarchical_vector_round_trips_through_verilog_export() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module top(input [1:0] a, output [1:0] y);
  child u0 (.p(a), .q(y));
endmodule
module child(input [1:0] p, output [1:0] q);
  assign q = ~p;
endmodule
";
    let circuit = parse_verilog(text).unwrap();
    let rendered = to_verilog(&circuit).unwrap();
    let again = parse_verilog(&rendered).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    assignment.insert("a[1]".to_string(), Ternary::False);
    let left = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    let right = propagate(&again, &assignment, Algebra::Kleene).unwrap();
    for bit in ["y[0]", "y[1]"] {
        assert_eq!(left.signals.get(bit), right.signals.get(bit), "{bit}");
    }
}

#[test]
fn hierarchical_generated_names_do_not_collide_with_user_signals() {
    // child 的内部 x 会展开成 u0__x；顶层已有同名线网时不能静默复用它。
    let err = parse_verilog(
        "module top(input a, output y, output z);
  wire u0__x;
  buf g0 (y, u0__x);
  child u0 (.p(a), .q(z));
endmodule
module child(input p, output q);
  wire x;
  buf g0 (x, p);
  buf g1 (q, x);
endmodule
",
    )
    .unwrap_err();
    assert!(err.to_string().contains("冲突"), "{err}");
}

#[test]
fn ascending_ranges_parse_export_and_json_round_trip() {
    let text = "module m(input [0:3] a, output [0:3] y);\nassign y = ~a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("a"), Some(&Bus::new_ascending("a", 4)));
    assert_eq!(circuit.buses.get("y"), Some(&Bus::new_ascending("y", 4)));
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("input [0:3] a;"), "{rendered}");
    assert!(rendered.contains("output [0:3] y;"), "{rendered}");
    assert_eq!(parse_verilog(&rendered).unwrap(), circuit);

    let json = to_json(&circuit).unwrap();
    assert!(json.contains("\"format\": 2"), "{json}");
    assert!(json.contains("\"ascending\": true"), "{json}");
    assert_eq!(from_json(&json).unwrap(), circuit);
}

#[test]
fn ascending_vector_bit_order_matches_verilog() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module m(input [0:3] a, output [0:3] y);\nassign y = ~a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    for index in 1..4 {
        assignment.insert(format!("a[{index}]"), Ternary::False);
    }
    let result = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    // [0:3] 的 a[0] 是 MSB；取反后 y[0] 仍是 MSB。
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::False));
    assert_eq!(result.signals.get("y[1]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::True));
}

#[test]
fn ascending_to_descending_assignment_preserves_positions() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module m(input [0:3] a, output [3:0] y);\nassign y = a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    for index in 1..4 {
        assignment.insert(format!("a[{index}]"), Ternary::False);
    }
    let result = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    // 两边都是左界 MSB：a[0] ↔ y[3]，a[3] ↔ y[0]。
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::False));
}

#[test]
fn literal_assignment_to_ascending_vector_keeps_verilog_order() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module m(output [0:3] y);\nassign y = 4'b1010;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let result = propagate(&circuit, &BTreeMap::new(), Algebra::Kleene).unwrap();
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[1]"), Some(&Ternary::False));
    assert_eq!(result.signals.get("y[2]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::False));
}

#[test]
fn ascending_internal_wire_keeps_range_on_export() {
    let text = "module m(input [0:1] a, output [0:1] y);\n\
                wire [0:1] n;\n\
                assign n = ~a;\n\
                assign y = n;\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses.get("n"), Some(&Bus::new_ascending("n", 2)));
    let rendered = to_verilog(&circuit).unwrap();
    assert!(rendered.contains("wire [0:1] n;"), "{rendered}");
    assert_eq!(parse_verilog(&rendered).unwrap(), circuit);
}

#[test]
fn hierarchical_cross_direction_bus_maps_by_position() {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;

    let text = "module top(input [3:0] a, output z0, output [3:0] y);\n\
                  child u0 (.p(a), .q0(z0), .q(y));\n\
                endmodule\n\
                module child(input [0:3] p, output q0, output [0:3] q);\n\
                  assign q0 = p[0];\n\
                  assign q = p;\n\
                endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    let mut assignment = BTreeMap::new();
    assignment.insert("a[0]".to_string(), Ternary::True);
    for index in 1..4 {
        assignment.insert(format!("a[{index}]"), Ternary::False);
    }
    let result = propagate(&circuit, &assignment, Algebra::Kleene).unwrap();
    // p[0] 是升序 MSB，接到 a[3]（降序 MSB）；a[3]=0。
    assert_eq!(result.signals.get("z0"), Some(&Ternary::False));
    // q/p 都是升序；q[3]（LSB）↔ y[0]（降序 LSB），p[3]↔a[0]=1。
    assert_eq!(result.signals.get("y[0]"), Some(&Ternary::True));
    assert_eq!(result.signals.get("y[3]"), Some(&Ternary::False));
}

#[test]
fn parses_parameterized_module_defaults() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.name, "leaf");
    assert_eq!(circuit.buses["y"].width, 2);
    assert_eq!(circuit.gates.len(), 2);
}

#[test]
fn named_parameter_override_changes_width() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n\
                    module top(input [3:0] a, input [3:0] b, output [3:0] y);\n\
                    leaf #(.W(4)) u (.a(a), .b(b), .y(y));\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.name, "top");
    assert_eq!(circuit.buses["y"].width, 4);
    assert_eq!(circuit.gates.len(), 4);
}

#[test]
fn deep_parameter_expression_returns_limit_not_stack_overflow() {
    // 复审 H03：参数表达式的括号递归不设防，3 万层曾直接栈溢出。
    // 深括号放在**覆盖**里——默认值不求值，覆盖必经 eval_expr。
    let deep = "(".repeat(30_000) + "1" + &")".repeat(30_000);
    let text = format!(
        "module leaf #(parameter W = 2) (\n\
         input a, output y);\n\
         assign y = a;\n\
         endmodule\n\
         module top(input a, output y);\n\
         leaf #(.W({deep})) u (.a(a), .y(y));\n\
         endmodule\n"
    );
    let err = parse_verilog(&text).unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Limit, "{err:?}");
}

#[test]
fn parameter_depth_just_over_limit_returns_limit_error() {
    // `MAX_PARAM_DEPTH = 256` 的**有界**版本（同上：300 层超限但不爆栈）。
    let deep = "(".repeat(300) + "1" + &")".repeat(300);
    let text = format!(
        "module leaf #(parameter W = 2) (\n\
         input a, output y);\n\
         assign y = a;\n\
         endmodule\n\
         module top(input a, output y);\n\
         leaf #(.W({deep})) u (.a(a), .y(y));\n\
         endmodule\n"
    );
    let err = parse_verilog(&text).unwrap_err();
    assert_eq!(err.kind, VerilogErrorKind::Limit, "{err:?}");
}

#[test]
fn parent_parameter_flows_to_positional_child_override() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n\
                    module top #(parameter W = 3) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    leaf #(W) u (.a(a), .b(b), .y(y));\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses["y"].width, 3);
    assert_eq!(circuit.gates.len(), 3);
}

#[test]
fn two_parameter_instances_get_separate_specializations() {
    let text = "module leaf #(parameter W = 2) (\n\
                    input [W-1:0] a, input [W-1:0] b, output [W-1:0] y);\n\
                    assign y = a & b;\n\
                    endmodule\n\
                    module top(input [1:0] a2, input [1:0] b2, output [1:0] y2,\n\
                               input [2:0] a3, input [2:0] b3, output [2:0] y3);\n\
                    leaf #(.W(2)) u2 (.a(a2), .b(b2), .y(y2));\n\
                    leaf #(.W(3)) u3 (.a(a3), .b(b3), .y(y3));\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.gates.len(), 5);
    assert_eq!(circuit.buses["y2"].width, 2);
    assert_eq!(circuit.buses["y3"].width, 3);
}

#[test]
fn parameter_expression_uses_arithmetic() {
    let text = "module leaf #(parameter W = 4, parameter D = (W / 2) + 1) (\n\
                    input [W-1:0] a, input [D-1:0] b, output [W-1:0] y);\n\
                    assign y = a;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses["y"].width, 4);
    assert_eq!(circuit.buses["b"].width, 3);
}

#[test]
fn rejects_unknown_parameter_override_and_body_parameter() {
    let unknown = "module leaf #(parameter W = 2) (input [W-1:0] a, output [W-1:0] y);\n\
                   assign y = a;\n\
                   endmodule\n\
                   module top(input [1:0] a, output [1:0] y);\n\
                   leaf #(.Z(2)) u (.a(a), .y(y));\n\
                   endmodule\n";
    assert!(parse_verilog(unknown).is_err());

    let body = "module leaf (input a, output y);\n\
                parameter W = 2;\n\
                assign y = a;\n\
                endmodule\n";
    assert!(parse_verilog(body).is_err());
}

#[test]
fn parameter_integer_keyword_is_accepted() {
    let text = "module leaf #(parameter integer W = 2) (\n\
                    input [W-1:0] a, output [W-1:0] y);\n\
                    assign y = a;\n\
                    endmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.buses["y"].width, 2);
}

#[test]
fn empty_parameter_list_is_accepted() {
    let text = "module m #() (input a, output y);\nassign y = a;\nendmodule\n";
    let circuit = parse_verilog(text).unwrap();
    assert_eq!(circuit.name, "m");
    assert_eq!(circuit.input_count(), 1);
}
