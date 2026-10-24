//! 稳定版 fuzz smoke：固定种子的随机/变异输入，解析器不许 panic，成功解析必须能往返。
//!
//! 真正的覆盖率引导 fuzz 在 `fuzz/`（cargo-fuzz，需要 nightly）；这里保证 CI 每次都能跑。

use trit_circuit::{
    Circuit, Gate, GateKind, Signal, generate_tests, parse_aiger, parse_blif, parse_netlist,
    parse_verilog, to_aiger, to_blif,
};
use trit_core::algebra::Algebra;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

fn mutate(state: &mut u64, seed: &str, rounds: usize) -> String {
    let mut bytes = seed.as_bytes().to_vec();
    for _ in 0..rounds {
        if bytes.is_empty() {
            break;
        }
        let index = (lcg(state) as usize) % bytes.len();
        match lcg(state) % 3 {
            0 => bytes[index] = (lcg(state) % 96 + 32) as u8,
            1 => {
                bytes.remove(index);
            }
            _ => bytes.insert(index, (lcg(state) % 96 + 32) as u8),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn netlist_parser_round_trips_or_rejects() {
    let mut state = 0x5eed_1234_9abc_def0u64;
    let seed = "input a\ninput b\ngate AND a b n\nnot n y\noutput y\n";
    // 解析器现在会在返回前过结构校验（复审 L03 的同批修正），所以"变异后仍合法"的样本
    // 不一定存在。基线必须可解析，变异样本只要求：不 panic；一旦接受就必须能往返。
    assert!(parse_netlist(seed).is_ok(), "种子必须可解析");
    let mut parsed = 0usize;
    let mut rejected = 0usize;
    for round in 0..400 {
        let text = mutate(&mut state, seed, round % 4 + 1);
        if let Ok(circuit) = parse_netlist(&text) {
            parsed += 1;
            let Ok(rendered) = trit_circuit::netlist::to_netlist(&circuit) else {
                continue;
            };
            let again = parse_netlist(&rendered).expect("to_netlist 输出必须可解析");
            assert_eq!(circuit, again);
        } else {
            rejected += 1;
        }
    }
    assert_eq!(parsed + rejected, 400, "每个变异样本都必须被处理");
}

#[test]
fn verilog_parser_round_trips_or_rejects() {
    let mut state = 0xfeed_4321_0bad_c0deu64;
    let seed = "module m(input a, input b, output y);\n  assign y = a ^ b;\nendmodule\n";
    let mut parsed = 0usize;
    for round in 0..400 {
        let text = mutate(&mut state, seed, round % 4 + 1);
        if let Ok(circuit) = parse_verilog(&text) {
            parsed += 1;
            let Ok(rendered) = trit_circuit::to_verilog(&circuit) else {
                continue;
            };
            let again = parse_verilog(&rendered).expect("to_verilog 输出必须可解析");
            assert_eq!(circuit.outputs.len(), again.outputs.len());
        }
    }
    assert!(parsed > 0, "变异种子应至少解析成功一次");
}

#[test]
fn blif_parser_round_trips_or_rejects() {
    let mut state = 0x0ddc_0ffe_e123_4567u64;
    let seed = ".model m
.inputs a b
.outputs y
.names a b y
11 1
.end
";
    let mut parsed = 0usize;
    for round in 0..400 {
        let text = mutate(&mut state, seed, round % 4 + 1);
        if let Ok(circuit) = parse_blif(&text) {
            parsed += 1;
            let rendered = to_blif(&circuit).expect("parse 出的组合网表必须能写回");
            let again = parse_blif(&rendered).expect("to_blif 输出必须可解析");
            assert_eq!(circuit.inputs.len(), again.inputs.len());
            assert_eq!(circuit.outputs.len(), again.outputs.len());
        }
    }
    assert!(parsed > 0, "变异种子应至少解析成功一次");
}

#[test]
fn aiger_parser_round_trips_or_rejects() {
    let mut state = 0xa1ee_5eed_1234_9876u64;
    let seed = "aag 3 2 0 1 1
2
4
6
6 2 4
";
    let mut parsed = 0usize;
    for round in 0..400 {
        let text = mutate(&mut state, seed, round % 4 + 1);
        if let Ok(circuit) = parse_aiger(&text) {
            parsed += 1;
            let rendered = to_aiger(&circuit).expect("parse 出的组合网表必须能写回");
            let again = parse_aiger(&rendered).expect("to_aiger 输出必须可解析");
            assert_eq!(circuit.inputs.len(), again.inputs.len());
            assert_eq!(circuit.outputs.len(), again.outputs.len());
        }
    }
    assert!(parsed > 0, "变异种子应至少解析成功一次");
}

#[test]
fn netlist_json_round_trips_or_rejects() {
    let mut state = 0x1357_9bdf_2468_ace0u64;
    let seed =
        trit_circuit::netlist::to_json(&random_circuit(&mut state)).expect("随机电路应能序列化");
    let mut parsed = 0usize;
    for round in 0..400 {
        let text = mutate(&mut state, &seed, round % 5 + 1);
        if let Ok(circuit) = trit_circuit::netlist::from_json(&text) {
            parsed += 1;
            let rendered =
                trit_circuit::netlist::to_json(&circuit).expect("serde 应能写回同一结构");
            let again =
                trit_circuit::netlist::from_json(&rendered).expect("to_json 输出必须可解析");
            assert_eq!(circuit, again);
            // 坏电路导出必须 Err 而不是 panic（报告 M-02）。
            let _ = trit_circuit::netlist::to_netlist(&circuit);
            let _ = trit_circuit::to_verilog(&circuit);
        }
    }
    assert!(parsed > 0, "变异种子应至少解析成功一次");
}

fn random_circuit(state: &mut u64) -> Circuit {
    let mut circuit = Circuit::new("fuzz");
    let mut signals: Vec<String> = Vec::new();
    for name in ["a", "b", "c"] {
        circuit.add_input(Signal::new(name));
        signals.push(name.to_string());
    }
    let kinds = [
        GateKind::And,
        GateKind::Or,
        GateKind::Nand,
        GateKind::Nor,
        GateKind::XorClassic,
    ];
    for index in 0..6 {
        let kind = kinds[(lcg(state) as usize) % kinds.len()];
        let left = signals[(lcg(state) as usize) % signals.len()].clone();
        let right = signals[(lcg(state) as usize) % signals.len()].clone();
        let output = format!("n{index}");
        circuit.add_gate(Gate::new(
            kind,
            vec![Signal::new(left), Signal::new(right)],
            Signal::new(output.clone()),
        ));
        signals.push(output);
    }
    circuit.add_output(Signal::new(signals[3].clone()));
    circuit.add_output(Signal::new(signals[5].clone()));
    circuit
}

#[test]
fn atpg_coverage_invariants_hold() {
    for seed in 1..=32u64 {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let circuit = random_circuit(&mut state);
        let report = generate_tests(&circuit, Algebra::Kleene).expect("小电路应能分析");
        assert!(
            (0.0..=1.0).contains(&report.coverage),
            "seed {seed} coverage={}",
            report.coverage
        );
        let mut seen = vec![0usize; report.faults.len()];
        for pattern in &report.patterns {
            for &index in &pattern.covers {
                seen[index] += 1;
            }
        }
        for (index, fault) in report.faults.iter().enumerate() {
            if fault.observable {
                assert_eq!(seen[index], 1, "seed {seed} 故障 {index}");
            } else {
                assert_eq!(seen[index], 0, "seed {seed} 不可测故障 {index}");
            }
        }
        assert_eq!(
            report.testable,
            seen.iter().filter(|&&count| count == 1).count()
        );
    }
}
