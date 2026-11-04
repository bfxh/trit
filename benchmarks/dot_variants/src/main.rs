//! A357：同一能力的两份权威实现——u64 导轨（`trit-dot`）与 u32 导轨（`trit-gpu` 的 CPU 镜像）
//! 在**同机同输入交错测**下的实测差。判据来自 `docs/LAYERS.md` §4：各带实测差才允许并存。
//!
//! 跑法（release，调试构建测不出真东西）：
//!     cargo run --release --manifest-path benchmarks/dot_variants/Cargo.toml
//!
//! 计时口径：打包与数据生成**留在计时外**（一次性建好四种表示），计时段只做点积本身；
//! 每个规模档把 4 个实现**交错跑 9 轮**（本机热漂移 ±25%，不交错会把 32% 读成 13%），
//! 报最小值与中位数。四个实现对同一批三值必须给**同一个 i64**，否则这轮不作数。

use std::hint::black_box;
use std::time::Instant;

const ROUNDS: usize = 9;

/// 确定性三值序列：`{-1,0,+1}`，避开周期性模式（会被缓存/压缩假性加速）。
fn values(len: usize, seed: u64) -> Vec<i32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 3) as i32 - 1
        })
        .collect()
}

/// u64 双导轨打包（`trit-dot` 的 `Planes` 形状：pos = 非零位，neg = 负位）。
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

fn main() {
    println!("A357 点积实测｜release｜每档交错 {ROUNDS} 轮｜打包不计入计时");
    println!(
        "{:>10}  {:>13} {:>13} {:>13} {:>13}",
        "trits", "u64 dispatch", "u64 portable", "u32 (gpu 镜像)", "i32 标量"
    );

    for trits in [8_192usize, 131_072, 2_097_152] {
        let a = values(trits, 0x9E37_79B9_7F4A_7C15);
        let b = values(trits, 0x1234_5678_9ABC_DEF1);
        let (a64p, a64n) = pack_u64(&a);
        let (b64p, b64n) = pack_u64(&b);
        let (a32p, a32n) = trit_gpu::pack_dual_rail(&a);
        let (b32p, b32n) = trit_gpu::pack_dual_rail(&b);

        // 先钉正确性：四路必须同一个数。
        let want = trit_gpu::dot_scalar(&a, &b);
        let got_dot = trit_dot::dot_words(&a64p, &a64n, &b64p, &b64n).expect("planes");
        let got_port = trit_dot::dot_words_portable(&a64p, &a64n, &b64p, &b64n).expect("planes");
        let got_u32 = trit_gpu::dot_words(&a32p, &a32n, &b32p, &b32n);
        assert_eq!(want, got_dot, "u64 dispatch 与标量不一致");
        assert_eq!(want, got_port, "u64 portable 与标量不一致");
        assert_eq!(want, got_u32, "u32 与标量不一致");

        let mut samples: [Vec<f64>; 4] = [const { Vec::new() }; 4];
        for _ in 0..ROUNDS {
            let mut push = |slot: usize, secs: f64| samples[slot].push(secs * 1e12 / trits as f64);
            let start = Instant::now();
            black_box(trit_dot::dot_words(&a64p, &a64n, &b64p, &b64n).unwrap());
            push(0, start.elapsed().as_secs_f64());
            let start = Instant::now();
            black_box(trit_dot::dot_words_portable(&a64p, &a64n, &b64p, &b64n).unwrap());
            push(1, start.elapsed().as_secs_f64());
            let start = Instant::now();
            black_box(trit_gpu::dot_words(&a32p, &a32n, &b32p, &b32n));
            push(2, start.elapsed().as_secs_f64());
            let start = Instant::now();
            black_box(trit_gpu::dot_scalar(&a, &b));
            push(3, start.elapsed().as_secs_f64());
        }

        let cells: Vec<String> = samples
            .iter()
            .map(|rounds| {
                let mut sorted = rounds.clone();
                sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());
                format!("{:>13.1}", sorted[sorted.len() / 2])
            })
            .collect();
        println!("{:>10}  {}", trits, cells.join(" "));
        for (name, rounds) in [
            ("u64 dispatch", &samples[0]),
            ("u64 portable", &samples[1]),
            ("u32 镜像", &samples[2]),
            ("i32 标量", &samples[3]),
        ] {
            let mut sorted = rounds.clone();
            sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());
            println!(
                "         {name:<12} min={:>7.2} median={:>7.2} max={:>7.2} ps/trit",
                sorted[0],
                sorted[sorted.len() / 2],
                sorted[sorted.len() - 1]
            );
        }
    }
}
