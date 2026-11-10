//! 小型基准：解析、时序、映射、ATPG。`cargo bench -p trit-circuit` 或 `scripts/bench.sh`。
//!
//! 不设阈值（墙钟受机器影响），只给出同机可复算的 per-iter 读数。

use std::hint::black_box;
use std::time::Instant;

use std::collections::BTreeMap;

use trit_circuit::{
    CellLibrary, GateKind, MapStrategy, TimingModel, analyze, analyze_timing, generate_tests,
    parse_netlist, parse_verilog, propagate_exact, propagate_exact_with_jobs, to_cells_with_load,
    to_nand_only,
};
use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

const CHAIN_GATES: usize = 32;

fn chain_netlist() -> String {
    let mut text = String::from("input a0\ninput b0\n");
    text.push_str("gate AND a0 b0 n0\n");
    for index in 1..CHAIN_GATES {
        text.push_str(&format!(
            "gate AND n{} n{} n{}\n",
            index - 1,
            index - 1,
            index
        ));
    }
    text.push_str(&format!("output n{}\n", CHAIN_GATES - 1));
    text
}

fn chain_verilog() -> String {
    let mut text = String::from("module bench(input a0, input b0, output y);\n  wire n0;\n");
    text.push_str("  assign n0 = a0 & b0;\n");
    for index in 1..CHAIN_GATES {
        text.push_str(&format!(
            "  assign n{index} = n{} & n{};\n",
            index - 1,
            index - 1
        ));
    }
    text.push_str(&format!("  assign y = n{};\nendmodule\n", CHAIN_GATES - 1));
    text
}

fn atpg_netlist() -> String {
    let mut text = String::new();
    for index in 0..6 {
        text.push_str(&format!("input a{index}\n"));
    }
    text.push_str("gate AND a0 a1 n0\n");
    for index in 1..5 {
        text.push_str(&format!(
            "gate AND n{} a{} n{}\n",
            index - 1,
            index + 1,
            index
        ));
    }
    text.push_str("output n4\n");
    text
}

/// `inputs` 个主输入的 40 门网表，全部主输入取 `X`，用来量精确传播。
fn exact_case(inputs: usize) -> (trit_circuit::Circuit, BTreeMap<String, Ternary>) {
    let mut text = String::new();
    for index in 0..inputs {
        text.push_str(&format!(
            "input i{index}
"
        ));
    }
    text.push_str(
        "gate AND i0 i1 g0
",
    );
    for index in 1..40 {
        text.push_str(&format!(
            "gate AND g{} i{} g{}
",
            index - 1,
            index % inputs,
            index
        ));
    }
    text.push_str(
        "output g39
",
    );
    let circuit = parse_netlist(&text).unwrap();
    let assignment = circuit
        .inputs
        .iter()
        .map(|signal| (signal.name().to_string(), Ternary::Unknown))
        .collect();
    (circuit, assignment)
}

fn bench<T>(name: &str, iters: u32, mut body: impl FnMut() -> T) {
    let start = Instant::now();
    for _ in 0..iters {
        black_box(body());
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed.as_secs_f64() * 1e6 / iters as f64;
    println!("{name:<24} {iters:>7}  {per_iter:>10.2} us/iter");
}

/// 分支可预测性探针：同一批求值，输入「随机」vs「固定」。
///
/// 二进制引擎里的 `if (state == 1) … else if (state == 0) …` 便宜或贵，取决于
/// **数据**：固定输入时分支预测器能学会，随机输入时学不会。三值查表没有这个
/// 变量——两种输入应当同样快。这张 2×2 表就是把「分支预测失败」这块钱量出来。
fn branch_probe() {
    const N: usize = 1 << 20;
    const KINDS: [GateKind; 6] = [
        GateKind::And,
        GateKind::Or,
        GateKind::Xor,
        GateKind::XorClassic,
        GateKind::Nand,
        GateKind::Nor,
    ];

    // RNG 改吃 trit-fuzz 的 SeedRng（同一条 xorshift 13/7/17 ⇒ 语料序列不变）。
    let mut rng = trit_fuzz::SeedRng::new(0x9E37_79B9_7F4A_7C15);
    let mut next = || rng.next_u64();
    // 第二项是 KINDS 的下标，避免在计时循环里再做一次门类型查找。
    let random: Vec<(usize, Ternary, Ternary)> = (0..N)
        .map(|i| {
            let kind = i % KINDS.len();
            let a = Ternary::from_index((next() % 3) as usize);
            let b = Ternary::from_index((next() % 3) as usize);
            (kind, a, b)
        })
        .collect();
    let fixed: Vec<(usize, Ternary, Ternary)> = (0..N)
        .map(|i| (i % KINDS.len(), Ternary::True, Ternary::False))
        .collect();

    // 与 Simulator 里 `CompiledGate.lut` 同构：每种门一张 `a*3+b` 的表。
    let luts: [[Ternary; 9]; 6] = std::array::from_fn(|kind| {
        let mut lut = [Ternary::False; 9];
        for (index, slot) in lut.iter_mut().enumerate() {
            *slot = KINDS[kind].eval(
                &[
                    Ternary::from_index(index / 3),
                    Ternary::from_index(index % 3),
                ],
                Algebra::Kleene,
            );
        }
        lut
    });

    fn per_op(iters: u32, mut body: impl FnMut() -> usize) -> f64 {
        black_box(body());
        let start = Instant::now();
        for _ in 0..iters {
            black_box(body());
        }
        start.elapsed().as_secs_f64() * 1e9 / f64::from(iters) / N as f64
    }

    let eval_match = |items: &[(usize, Ternary, Ternary)]| -> usize {
        items
            .iter()
            .map(|(kind, a, b)| KINDS[*kind].eval(&[*a, *b], Algebra::Kleene).index())
            .sum()
    };
    let eval_lut = |items: &[(usize, Ternary, Ternary)]| -> usize {
        items
            .iter()
            .map(|(kind, a, b)| luts[*kind][a.index() * 3 + b.index()].index())
            .sum()
    };

    let match_random = per_op(3, || eval_match(&random));
    let match_fixed = per_op(3, || eval_match(&fixed));
    let lut_random = per_op(3, || eval_lut(&random));
    let lut_fixed = per_op(3, || eval_lut(&fixed));

    println!("{:<24} {:>9}  {:>9}", "branch probe", "random", "fixed");
    println!(
        "{:<24} {match_random:>9.3}  {match_fixed:>9.3}   ns/op  (match 求值)",
        "by match"
    );
    println!(
        "{:<24} {lut_random:>9.3}  {lut_fixed:>9.3}   ns/op  (查表)",
        "by lut"
    );
    println!(
        "{:<24} {:>9.2}  {:>9.2}   x      (match/lut)",
        "speedup",
        match_random / lut_random,
        match_fixed / lut_fixed
    );
}

fn main() {
    let netlist = chain_netlist();
    let verilog = chain_verilog();
    let atpg_text = atpg_netlist();
    let circuit = parse_netlist(&netlist).unwrap();
    let atpg_circuit = parse_netlist(&atpg_text).unwrap();
    let library = CellLibrary::new([
        (trit_circuit::GateKind::And, 2),
        (trit_circuit::GateKind::Nand, 1),
    ]);

    println!("{:<24} {:>7}  {:>10}", "bench", "iters", "per iter");
    bench("netlist parse", 2_000, || parse_netlist(&netlist));
    bench("verilog parse", 2_000, || parse_verilog(&verilog));
    bench("timing analyze", 2_000, || {
        analyze_timing(&circuit, TimingModel::default(), Some(64))
    });
    bench("map nand-only", 500, || to_nand_only(&circuit));
    bench("map timing-driven", 500, || {
        to_cells_with_load(&circuit, &library, MapStrategy::Timing, 1)
    });
    bench("atpg (6 inputs)", 50, || {
        generate_tests(&atpg_circuit, Algebra::Kleene)
    });
    bench("analyze (6 inputs)", 50, || {
        analyze(&atpg_circuit, Algebra::Kleene)
    });

    branch_probe();

    let (exact8, assign8) = exact_case(8);
    let (exact12, assign12) = exact_case(12);
    bench("exact k=8 (serial)", 200, || {
        propagate_exact(&exact8, &assign8, Algebra::Kleene, 4096)
    });
    bench("exact k=12 (auto jobs)", 50, || {
        propagate_exact_with_jobs(&exact12, &assign12, Algebra::Kleene, 4096, 0)
    });
}
