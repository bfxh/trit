//! A357 的复算尺（改用 `trit-bench`）：u64 导轨（`trit-dot`）与 u32 导轨（`trit-gpu` 的 CPU 镜像）
//! 在**交错轮次**下的实测差。判据来自 `docs/LAYERS.md` §4：第二实现要带数字才允许并存。
//!
//! 跑法（release）：`cargo run --release --manifest-path benchmarks/dot_variants/Cargo.toml`
//!
//! 计时口径：数据生成与打包留在计时外；计时段只做点积，闭包返回校验和由 `black_box` 兜住。
//! 四路必须先给**同一个 i64**，否则这轮不作数。读数应与换成 trit-bench 之前一致 ——
//! 底件只允许改代码形状，不允许改尺子。

use std::collections::HashSet;

use trit_bench::{Bench, SeedRng, Spread};
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;

const ROUNDS: usize = 9;

fn pack_u64(values: &[i32]) -> (Vec<u64>, Vec<u64>) {
    let blocks = values.len().div_ceil(64);
    let mut pos = vec![0u64; blocks];
    let mut neg = vec![0u64; blocks];
    for (index, value) in values.iter().enumerate() {
        let slot = index / 64;
        let bit = 1u64 << (index % 64);
        match value {
            1 => pos[slot] |= bit,
            -1 => neg[slot] |= bit,
            _ => {}
        }
    }
    (pos, neg)
}

/// 匹配族 `(x0∧y0)|…`，声明顺序「所有 x 再所有 y」：BDD 的坏顺序，DPLL 吃得下。
fn matching_family(pairs: usize) -> (Expr, Vec<String>, HashSet<String>) {
    let xs: Vec<String> = (0..pairs).map(|index| format!("x{index}")).collect();
    let ys: Vec<String> = (0..pairs).map(|index| format!("y{index}")).collect();
    let mut source = String::new();
    for (x, y) in xs.iter().zip(&ys) {
        let term = format!("({x} & {y})");
        source = if source.is_empty() { term } else { format!("({source} | {term})") };
    }
    let declared: Vec<String> = xs.iter().chain(ys.iter()).cloned().collect();
    let binary: HashSet<String> = declared.iter().cloned().collect();
    (Parser::parse(&source).expect("匹配族应可解析"), declared, binary)
}

fn main() {
    println!("A357 复算尺（trit-bench，交错 {ROUNDS} 轮 + 计时段内重复，打包不计入计时）");
    println!("{:>9} {:>12} {:>12} {:>12} {:>12}", "trits", "u64 派发", "u64 便携", "u32 镜像", "i32 标量");
    let mut report = Vec::new();
    for trits in [8_192usize, 131_072, 2_097_152] {
        let a = SeedRng::new(0x9E37_79B9_7F4A_7C15).trits(trits);
        let b = SeedRng::new(0x1234_5678_9ABC_DEF1).trits(trits);
        let (a64p, a64n) = pack_u64(&a);
        let (b64p, b64n) = pack_u64(&b);
        let (a32p, a32n) = trit_gpu::pack_dual_rail(&a);
        let (b32p, b32n) = trit_gpu::pack_dual_rail(&b);

        let want = trit_gpu::dot_scalar(&a, &b);
        assert_eq!(want, trit_dot::dot_words(&a64p, &a64n, &b64p, &b64n).unwrap());
        assert_eq!(want, trit_dot::dot_words_portable(&a64p, &a64n, &b64p, &b64n).unwrap());
        assert_eq!(want, trit_gpu::dot_words(&a32p, &a32n, &b32p, &b32n));

        let mut bench = Bench::new(trits).reps(if trits > 100_000 { 30 } else { 400 });
        bench.add("u64 派发", || trit_dot::dot_words(&a64p, &a64n, &b64p, &b64n).unwrap() as u64);
        bench.add("u64 便携", || trit_dot::dot_words_portable(&a64p, &a64n, &b64p, &b64n).unwrap() as u64);
        bench.add("u32 镜像", || trit_gpu::dot_words(&a32p, &a32n, &b32p, &b32n) as u64);
        bench.add("i32 标量", || trit_gpu::dot_scalar(&a, &b) as u64);
        let rows = bench.measure(ROUNDS);
        let cells: Vec<String> =
            rows.iter().map(|(_, spread)| format!("{:>12.2}", spread.ps_per_unit(trits))).collect();
        println!("{:>9} {}", trits, cells.join(""));
        for (label, spread) in &rows {
            println!(
                "           {label:<8} {}",
                Bench::format(spread, trits)
            );
        }
        let ps = |name: &str| -> f64 {
            rows.iter()
                .find(|(label, _)| label == name)
                .map(|(_, spread)| spread.ps_per_unit(trits))
                .unwrap()
        };
        report.push((trits, ps("u64 派发"), ps("u32 镜像"), ps("i32 标量")));
        assert!(
            ps("u64 派发") * 2.5 < ps("u32 镜像"),
            "u64 应显著快于 u32 镜像：{} vs {}",
            ps("u64 派发"),
            ps("u32 镜像")
        );
        assert!(ps("u32 镜像") * 2.0 < ps("i32 标量"), "位并行应显著快于标量");
    }

    println!("\nDPLL 侧的规模档（trit-count 三方对拍，中位数口径）");
    for pairs in [12usize, 20, 32] {
        let (expr, declared, binary) = matching_family(pairs);
        let mut bench = Bench::new(1);
        bench.add("dpll", || {
            trit_count::count_models(&expr, &declared, &binary, Algebra::Kleene, Ternary::True).count
                as u64
        });
        let rows: Vec<(String, Spread)> = bench.measure(ROUNDS);
        let dpll = &rows[0].1;
        println!(
            "{pairs:>3} 对  dpll min={:.3} ms  med={:.3} ms  max={:.3} ms",
            dpll.min_ns / 1e6,
            dpll.median_ns / 1e6,
            dpll.max_ns / 1e6
        );
    }
}
