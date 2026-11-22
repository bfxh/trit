//! ECC 基准（harness = false，零新依赖：只靠 `std::time` + `std::hint`）。
//!
//! 口径：encode / syndrome / decode 吞吐，Hamming r=3（n=13）与 r=4（n=40）
//! 两档 + Golay [11,6,5] + 帧层；每组 10 万次逐次计时取中位。
//! 另测一次 r=3 全码字真值表枚举（59049 码字 × 单 trit 错全纠）的总耗时。
//!
//! 注意：Windows `Instant` 分辨率约 100 ns——单次耗时低于它的操作（n=13/11 的
//! 块级三件套）读数按 100 ns 台阶取整，只作相对比较；帧层（几十 µs）不受影响。
//!
//! ```text
//! cargo bench -p trit-ops --bench ecc
//! ```

use std::hint::black_box;
use std::time::Instant;

use trit_ops::gf3::{HammingCode, HammingDecode};
use trit_ops::golay::Golay11;
use trit_ops::trit::Trit;
use trit_ops::{FrameCode, decode_line, encode_line};

const ITERS: usize = 100_000;
const WARMUP: usize = 1_000;

/// 逐次计时取中位（纳秒/次）。预热后开测，样本排序取中点。
fn median_ns(mut f: impl FnMut()) -> u128 {
    for _ in 0..WARMUP {
        f();
    }
    let mut samples = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t0 = Instant::now();
        f();
        samples.push(t0.elapsed().as_nanos());
    }
    samples.sort_unstable();
    samples[ITERS / 2]
}

fn fixed_message(k: usize) -> Vec<Trit> {
    (0..k)
        .map(|i| match i % 3 {
            0 => Trit::POS,
            1 => Trit::NEG,
            _ => Trit::ZERO,
        })
        .collect()
}

fn report(name: &str, ns: u128, extra: &str) {
    println!("{name:<44} {ns:>8} ns/次  {extra}");
}

fn main() {
    println!("ECC 基准：{} 次取中位（预热 {}）\n", ITERS, WARMUP);

    // ---- Hamming r=3 [13,10,3] ----
    let r3 = HammingCode::r3();
    let m3 = fixed_message(r3.k());
    let c3 = r3.encode(&m3);
    let mut bad3 = c3.clone();
    bad3[3] = trit_ops::gf3::add(bad3[3], Trit::NEG);
    let mut out = vec![Trit::ZERO; r3.n()];
    report(
        "hamming13 encode_into (n=13)",
        median_ns(|| r3.encode_into(black_box(&m3), &mut out)),
        "10 消息 trit → 13",
    );
    report(
        "hamming13 syndrome (n=13)",
        median_ns(|| {
            black_box(r3.syndrome(black_box(&c3)));
        }),
        "0 = 干净",
    );
    report(
        "hamming13 decode_into (n=13, 纠单错)",
        median_ns(|| {
            let v = r3.decode_into(black_box(&bad3), &mut out);
            black_box(v);
        }),
        "单错定位+纠正",
    );

    // ---- Hamming r=4 [40,36,3] ----
    let r4 = HammingCode::r4();
    let m4 = fixed_message(r4.k());
    let c4 = r4.encode(&m4);
    let mut bad4 = c4.clone();
    bad4[7] = trit_ops::gf3::add(bad4[7], Trit::NEG);
    let mut out4 = vec![Trit::ZERO; r4.n()];
    report(
        "hamming40 encode_into (n=40)",
        median_ns(|| r4.encode_into(black_box(&m4), &mut out4)),
        "36 消息 trit → 40",
    );
    report(
        "hamming40 syndrome (n=40)",
        median_ns(|| {
            black_box(r4.syndrome(black_box(&c4)));
        }),
        "0 = 干净",
    );
    report(
        "hamming40 decode_into (n=40, 纠单错)",
        median_ns(|| {
            let v = r4.decode_into(black_box(&bad4), &mut out4);
            black_box(v);
        }),
        "单错定位+纠正",
    );

    // ---- Golay [11,6,5] ----
    let g = Golay11::new();
    let mg = fixed_message(g.k());
    let cg = g.encode(&mg);
    let mut badg = cg.clone();
    badg[2] = trit_ops::gf3::add(badg[2], Trit::NEG);
    let mut outg = vec![Trit::ZERO; g.n()];
    report(
        "golay11 encode_into (n=11)",
        median_ns(|| g.encode_into(black_box(&mg), &mut outg)),
        "6 消息 trit → 11",
    );
    report(
        "golay11 syndrome (n=11)",
        median_ns(|| {
            black_box(g.syndrome(black_box(&cg)));
        }),
        "243 伴随式查表",
    );
    report(
        "golay11 decode_into (n=11, 纠单错)",
        median_ns(|| {
            let v = g.decode_into(black_box(&badg), &mut outg);
            black_box(v);
        }),
        "纠 1 / 拒 2 确定语义",
    );

    // ---- 帧层（trace_store 的消费形态）----
    let payload: Vec<u8> = (0..413u32).map(|i| (i * 7 + 11) as u8).collect();
    let line = encode_line(FrameCode::Golay11, &payload);
    report(
        "frame encode_line (Golay11, 413B 负载)",
        median_ns(|| {
            let l = encode_line(FrameCode::Golay11, black_box(&payload));
            black_box(l);
        }),
        &format!("→ {} 字节行", line.len()),
    );
    report(
        "frame decode_line (Golay11, 干净帧)",
        median_ns(|| {
            let r = decode_line(black_box(&line));
            black_box(r);
        }),
        "解析+全块译码+帧校验",
    );

    // ---- 全码字真值表枚举耗时（r=3，59049 码字 × 单 trit 错全纠）----
    let t0 = Instant::now();
    let mut cases = 0u64;
    for raw in 0..3u32.pow(10) {
        let message: Vec<Trit> = (0..10)
            .map(|i| match ((raw / 3u32.pow(i as u32)) % 3) as u8 {
                0 => Trit::ZERO,
                1 => Trit::POS,
                _ => Trit::NEG,
            })
            .collect();
        let word = r3.encode(&message);
        black_box(&word);
        let (verdict, fixed) = r3.decode(&word);
        assert!(matches!(verdict, HammingDecode::Clean));
        assert_eq!(fixed, word);
        cases += 1;
        for pos in 0..r3.n() {
            for mag in 1..=2u8 {
                let mut bad = word.clone();
                bad[pos] = trit_ops::gf3::add(bad[pos], trit_ops::gf3::from_digit(mag));
                let (verdict, fixed) = r3.decode(&bad);
                assert!(matches!(verdict, HammingDecode::Corrected { .. }));
                assert_eq!(fixed, word);
                cases += 1;
            }
        }
    }
    let table_ms = t0.elapsed().as_millis();
    println!();
    println!(
        "r=3 全码字真值表枚举：{cases} 例（59049 码字 × [1 干净 + 26 单错]）耗时 {table_ms} ms"
    );
    println!("完成。");
}
