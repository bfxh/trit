//! 区间算术基准（harness = false，零新依赖：只靠 `std::time` + `std::hint`）。
//!
//! 口径：
//! - 五种运算单次耗时：Windows `Instant` 分辨率约 100 ns，单次运算远低于它
//!   （逐次计时实测读 0）——按**批计量**：每个样本计时 64 次连续运算，10 万
//!   个样本取中位再除 64（`median_ns_per_op`），读数才落在分辨率之内；
//! - 10 万次连续混合运算吞吐（整块计时 → ns/次）；
//! - 加宽开销占比：本文件内联 `raw_*` 对照路径（同区间算术标准定义的
//!   min/max 端点、**不做** 1 ulp 加宽），同一输入同一循环分别计时，如实报
//!   占比。加宽是正确性代价（见 `interval.rs` LANDMINE 注释），占多少是多少
//!   ——输入分布（幅度、宽度）会浮动占比，数字附口径。
//!
//! ```text
//! cargo bench -p trit-ops --bench interval
//! ```

use std::hint::black_box;
use std::time::Instant;

use trit_ops::interval::Interval;

const ITERS: usize = 100_000;
/// 单次耗时的批大小：64 次运算/样本 ≈ 数百 ns，高于 Windows 计时分辨率。
const BATCH: usize = 64;
const WARMUP_BATCHES: usize = 128;
/// 预生成输入集大小：循环索引轮换取用，防常量折叠与分支预测贴合单一值。
const SET: usize = 1024;

/// 批计量取中位：每个样本计时 `BATCH` 次运算（闭包内自旋转），中位除以批
/// 大小 = ns/次。数据依赖链（旋转索引 + 累积值）防整链消去。
fn median_ns_per_op(mut f: impl FnMut(usize)) -> u128 {
    for _ in 0..WARMUP_BATCHES {
        f(BATCH);
    }
    let mut samples = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t0 = Instant::now();
        f(BATCH);
        samples.push(t0.elapsed().as_nanos());
    }
    samples.sort_unstable();
    samples[ITERS / 2] / BATCH as u128
}

/// raw 对照：标准区间定义的端点 min/max，无加宽（仅基准对照用）。
/// 返回端点二元组，不走 `Interval` 构造收口。
fn raw_add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 + b.0, a.1 + b.1)
}

/// raw 对照乘法：四积 min/max，无加宽。
fn raw_mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let c = [a.0 * b.0, a.0 * b.1, a.1 * b.0, a.1 * b.1];
    let lo = c[0].min(c[1]).min(c[2]).min(c[3]);
    let hi = c[0].max(c[1]).max(c[2]).max(c[3]);
    (lo, hi)
}

/// 确定 xorshift64*（生成基准输入，不引依赖）。
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// 生成基准输入集：混合幅度/宽度的区间对（有限域内，无溢出）；
/// 除数集同号非零（div 有定义）；非负集（sqrt 有定义）。
struct Inputs {
    a: Vec<Interval>,
    b: Vec<Interval>,
    d: Vec<Interval>,
    sq: Vec<Interval>,
    raw_a: Vec<(f64, f64)>,
    raw_b: Vec<(f64, f64)>,
}

impl Inputs {
    fn build() -> Self {
        let mut rng = Rng(0xDEAD_BEEF_1234_0001);
        let mut a = Vec::with_capacity(SET);
        let mut b = Vec::with_capacity(SET);
        let mut d = Vec::with_capacity(SET);
        let mut sq = Vec::with_capacity(SET);
        let mut raw_a = Vec::with_capacity(SET);
        let mut raw_b = Vec::with_capacity(SET);
        for i in 0..SET {
            // 幅度 2^±k 混合：k ∈ [−6, 6]；宽度取量级的 2^−3 档，间或点区间。
            let mag = 2f64.powi(((rng.next() % 13) as i32) - 6);
            let wide = !i.is_multiple_of(8);
            let w = if wide { mag / 8.0 } else { 0.0 };
            let alo = -mag + rng.next() as f64 / u64::MAX as f64 * w;
            let ahi = alo + w;
            let blo = mag / 16.0 + rng.next() as f64 / u64::MAX as f64 * w;
            let bhi = blo + w;
            a.push(Interval::new(alo, ahi));
            b.push(Interval::new(blo, bhi));
            raw_a.push((alo, ahi));
            raw_b.push((blo, bhi));
            // 除数：同号非零（[1+…, 2+…] 或负的对应）。
            let sign = if rng.next().is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let d1 = sign * (1.0 + rng.next() as f64 / u64::MAX as f64);
            let d2 = sign * (1.5 + rng.next() as f64 / u64::MAX as f64);
            d.push(Interval::new(d1.min(d2), d1.max(d2)));
            // 非负区间（sqrt 域）：[2^k·(1+…), 2^k·(2+…)]，k ∈ [−8, 4]。
            let smag = 2f64.powi(((rng.next() % 13) as i32) - 8);
            let s1 = smag * (1.0 + rng.next() as f64 / u64::MAX as f64);
            let s2 = smag * (2.0 + rng.next() as f64 / u64::MAX as f64);
            sq.push(Interval::new(s1, s2));
        }
        Self {
            a,
            b,
            d,
            sq,
            raw_a,
            raw_b,
        }
    }
}

fn report(name: &str, ns: u128, extra: &str) {
    println!("{name:<44} {ns:>8} ns/次  {extra}");
}

fn main() {
    println!(
        "区间算术基准：单次 = 批计量（{BATCH} 次/样本 × {ITERS} 样本取中位）；吞吐/对照为整块计时\n"
    );
    let inp = Inputs::build();

    // ---- 单次耗时（批计量）----
    let a = &inp.a;
    let b = &inp.b;
    let d = &inp.d;
    let sq = &inp.sq;
    let mut i0 = 0usize;
    let mut i1 = 0usize;
    let mut i2 = 0usize;
    let mut i3 = 0usize;
    let mut i4 = 0usize;
    let mut i5 = 0usize;
    report(
        "interval add（点区间对）",
        median_ns_per_op(|n| {
            for _ in 0..n {
                i0 = i0.wrapping_add(1);
                let j = i0 & (SET - 1);
                black_box(Interval::point(1.0 + j as f64).add(Interval::point(2.0 - j as f64)));
            }
        }),
        "退化区间，无端点组合",
    );
    report(
        "interval add（一般区间对）",
        median_ns_per_op(|n| {
            for _ in 0..n {
                i1 = i1.wrapping_add(1);
                let j = i1 & (SET - 1);
                black_box(a[j].add(b[j]));
            }
        }),
        "Empty 检查+端点和+加宽",
    );
    report(
        "interval sub（一般区间对）",
        median_ns_per_op(|n| {
            for _ in 0..n {
                i2 = i2.wrapping_add(1);
                let j = i2 & (SET - 1);
                black_box(a[j].sub(b[j]));
            }
        }),
        "",
    );
    report(
        "interval mul（一般区间对）",
        median_ns_per_op(|n| {
            for _ in 0..n {
                i3 = i3.wrapping_add(1);
                let j = i3 & (SET - 1);
                black_box(a[j].mul(b[j]));
            }
        }),
        "四积 min/max+加宽",
    );
    report(
        "interval div（同号非零除数）",
        median_ns_per_op(|n| {
            for _ in 0..n {
                i4 = i4.wrapping_add(1);
                let j = i4 & (SET - 1);
                black_box(a[j].div(d[j]));
            }
        }),
        "含零分母检查+四商 min/max",
    );
    report(
        "interval sqrt（非负域）",
        median_ns_per_op(|n| {
            for _ in 0..n {
                i5 = i5.wrapping_add(1);
                let j = i5 & (SET - 1);
                black_box(sq[j].sqrt());
            }
        }),
        "定义域检查+两端开方",
    );

    // ---- 10 万次连续吞吐（整块计时）----
    let t0 = Instant::now();
    let mut acc = Interval::point(0.5);
    for i in 0..ITERS {
        let j = i & (SET - 1);
        acc = acc.add(b[j]);
        acc = acc.mul(a[j]);
        acc = acc.sub(d[j]);
        black_box(&acc);
        // 吞吐链里出现 Empty 就重置（加宽溢出在混合幅度下偶发）。
        if acc.is_empty() {
            acc = Interval::point(0.5);
        }
    }
    let mixed_ns = t0.elapsed().as_nanos() / ITERS as u128;
    println!();
    report(
        "吞吐：add+mul+sub 混合链（10 万步）",
        mixed_ns,
        "3 次运算/步，含 Empty 重置分支",
    );

    // ---- 加宽开销占比（同输入，加宽 vs 不加宽，如实报数）----
    let raw = &inp.raw_a;
    let rb = &inp.raw_b;
    let t0 = Instant::now();
    let mut acc = (0.5f64, 0.5f64);
    for i in 0..ITERS {
        let j = i & (SET - 1);
        acc = raw_add(acc, rb[j]);
        acc = raw_mul(acc, raw[j]);
        black_box(&acc);
    }
    let raw_ns = t0.elapsed().as_nanos() / ITERS as u128;
    let t0 = Instant::now();
    let mut acc = Interval::point(0.5);
    for i in 0..ITERS {
        let j = i & (SET - 1);
        acc = acc.add(b[j]);
        acc = acc.mul(a[j]);
        black_box(&acc);
        if acc.is_empty() {
            acc = Interval::point(0.5);
        }
    }
    let wide_ns = t0.elapsed().as_nanos() / ITERS as u128;
    let overhead_pct = (wide_ns as f64 - raw_ns as f64) / raw_ns as f64 * 100.0;
    println!();
    report(
        "对照：add+mul 不加宽（raw 端点）",
        raw_ns,
        "无 Empty 检查/加宽/收口",
    );
    report(
        "对照：add+mul 加宽（interval）",
        wide_ns,
        "含构造收口与 Empty 分支",
    );
    println!(
        "加宽开销占比：{overhead_pct:.1}%（add+mul 链、本输入分布下；占比随幅度/宽度浮动，\n  加宽是正确性代价不是精度特性——删掉它保守性断言会静默失效）"
    );
    println!("完成。");
}
