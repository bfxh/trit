//! SDF 的 per-instance 面：`(INSTANCE ...)` 层次里的实例路径怎么折叠成 celltype 延迟，
//! 以及 `timing_model_from_sdf_for` 按电路门名匹配的那部分回归。
//!
//! 从 `sdf.rs` 拆出来：文件顶层已经分了两组测试（`tests` 与 `instance_tests`），
//! 后者讲的是**实例级**语义，与"一个 celltype 取最坏值"的解析主路是两件事。测试搬家，公开面不变。

use super::*;
use crate::timing::analyze_timing;

const TWO_LEAVES: &str = "module leaf(input a, input b, output y);\n\
    assign y = a & b;\n\
    endmodule\n\
    module top(input a, input b, input c, input d, output y1, output y2);\n\
    leaf u1 (.a(a), .b(b), .y(y1));\n\
    leaf u2 (.a(c), .b(d), .y(y2));\n\
    endmodule\n";

const TWO_LEAVES_SDF: &str = r#"
    (DELAYFILE
      (SDFVERSION "3.0")
      (DESIGN "top")
      (CELL (CELLTYPE "AND2") (INSTANCE u1)
        (DELAY (ABSOLUTE (IOPATH a y (2.0) (2.0)))))
      (CELL (CELLTYPE "AND2") (INSTANCE u2)
        (DELAY (ABSOLUTE (IOPATH a y (5.0) (5.0)))))
    )
"#;

#[test]
fn parses_per_instance_delays() {
    let instances = parse_sdf_instances(TWO_LEAVES_SDF).unwrap();
    assert_eq!(instances.len(), 2);
    assert!(
        instances
            .iter()
            .any(|item| item.instance == "u1" && item.delay == 2)
    );
    assert!(
        instances
            .iter()
            .any(|item| item.instance == "u2" && item.delay == 5)
    );
}

#[test]
fn instance_delays_override_celltype_in_sta() {
    let circuit = crate::parse_verilog(TWO_LEAVES).unwrap();
    assert_eq!(
        circuit
            .gates
            .iter()
            .find(|gate| gate.output.name() == "y1")
            .unwrap()
            .instance
            .as_deref(),
        Some("u1")
    );
    let model = timing_model_from_sdf_for(TWO_LEAVES_SDF, &circuit).unwrap();
    assert_eq!(model.instance_delays["y1"], 2);
    assert_eq!(model.instance_delays["y2"], 5);
    let report = analyze_timing(&circuit, model, None).unwrap();
    let arrival = |name: &str| {
        report
            .outputs
            .iter()
            .find(|output| output.name == name)
            .unwrap()
            .arrival
    };
    assert_eq!(arrival("y1"), 2);
    assert_eq!(arrival("y2"), 5);
    assert_eq!(report.critical_path, 5);
}

#[test]
fn instance_path_accepts_a_design_prefix() {
    assert!(instance_matches("top.u1", "u1"));
    assert!(instance_matches("top/u1", "u1"));
    assert!(!instance_matches("top.u10", "u1"));
}

const CLASSIC_XOR: &str = "module leaf(input a, input b, output y);\n\
    assign y = a ^ b;\n\
    endmodule\n\
    module top(input a, input b, output y);\n\
    leaf u9 (.a(a), .b(b), .y(y));\n\
    endmodule\n";

#[test]
fn sdf_instance_delay_applies_to_classic_xor() {
    // 复审 H11 复现：Verilog `xor` 实例配 `XOR2` 延迟 100，修复前关键路径仍是默认 1。
    let circuit = crate::parse_verilog(CLASSIC_XOR).unwrap();
    assert!(
        circuit
            .gates
            .iter()
            .any(|gate| gate.kind == GateKind::XorClassic
                && gate.instance.as_deref() == Some("u9")),
        "经典 XOR 实例没有展开成 XorClassic/u9"
    );
    let sdf = r#"
    (DELAYFILE
      (CELL (CELLTYPE "XOR2") (INSTANCE u9)
        (DELAY (ABSOLUTE (IOPATH a y (100.0) (100.0)))))
    )
    "#;
    let model = timing_model_from_sdf_for(sdf, &circuit).unwrap();
    assert_eq!(model.instance_delays["y"], 100);
    let report = analyze_timing(&circuit, model, None).unwrap();
    assert_eq!(report.critical_path, 100);
}
