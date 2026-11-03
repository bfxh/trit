//! 参数化模块（`#(parameter ...)`）的回归：默认值、命名覆盖、深度上限、父参流向子覆盖、特化分离。
//!
//! 从 `hierarchy_tests.rs` 拆出来——那一半讲的是**层次连接**（实例、端口、总线跨方向映射），
//! 这一半讲的是**参数**（宽度怎么算、什么时候拒）。两边都只碰测试，公开面不变。

use super::*;
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
