//! `trit-dot`：位并三点积的实测读数。

use std::process::ExitCode;
use std::time::Instant;

use trit_dot::{
    BLOCK_BITS, OPS, TernaryBits, bipolar_dot, bipolar_dot_portable, dot_words_unrolled, float_dot,
    float_dot_4acc, pack_bipolar, scalar_dot,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bench") => bench(&args[1..]),
        _ => Err("用法：trit-dot bench [--len N] [--iters N]".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn ternary(&mut self) -> i32 {
        (self.next() % 3) as i32 - 1
    }
}

fn number(args: &[String], name: &str, default: usize) -> Result<usize, String> {
    match args.iter().position(|arg| arg == name) {
        Some(index) => args
            .get(index + 1)
            .ok_or_else(|| format!("{name} 缺少参数"))?
            .parse()
            .map_err(|_| format!("{name} 不是整数")),
        None => Ok(default),
    }
}

fn time<T>(iters: usize, mut body: impl FnMut() -> T) -> u128 {
    // 预热一次，避免把首次分配的抖动算进去。
    std::hint::black_box(body());
    let start = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(body());
    }
    start.elapsed().as_nanos() / iters as u128
}

fn bench(args: &[String]) -> Result<(), String> {
    let len = number(args, "--len", 1 << 20)?;
    let iters = number(args, "--iters", 20)?;
    if len == 0 || iters == 0 {
        return Err("--len/--iters 必须大于 0".to_string());
    }

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let a: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
    let b: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
    // 90% 为零的稀疏版本。
    let sparse_a: Vec<i32> = a
        .iter()
        .map(|value| {
            if rng.next().is_multiple_of(10) {
                *value
            } else {
                0
            }
        })
        .collect();
    let sparse_b: Vec<i32> = b
        .iter()
        .map(|value| {
            if rng.next().is_multiple_of(10) {
                *value
            } else {
                0
            }
        })
        .collect();

    // 块级稀疏：整块 64 个元素一起清零（这才是位并行能跳过的东西）。
    let mut block_sparse = a.clone();
    for block in 0..block_sparse.len().div_ceil(BLOCK_BITS) {
        if !rng.next().is_multiple_of(10) {
            let start = block * BLOCK_BITS;
            let end = (start + BLOCK_BITS).min(block_sparse.len());
            block_sparse[start..end].fill(0);
        }
    }

    let bits_a = TernaryBits::from_trits(&a);
    let bits_b = TernaryBits::from_trits(&b);
    let sparse_bits_a = TernaryBits::from_trits(&sparse_a);
    let sparse_bits_b = TernaryBits::from_trits(&sparse_b);

    let bipolar_a: Vec<i32> = a.iter().map(|v| if *v >= 0 { 1 } else { -1 }).collect();
    let bipolar_b: Vec<i32> = b.iter().map(|v| if *v >= 0 { 1 } else { -1 }).collect();
    let packed_a = pack_bipolar(&bipolar_a);
    let packed_b = pack_bipolar(&bipolar_b);

    let block_bits_a = TernaryBits::from_trits(&block_sparse);
    let block_bits_b = TernaryBits::from_trits(&b);
    let float_a: Vec<f32> = a.iter().map(|v| *v as f32).collect();
    let float_b: Vec<f32> = b.iter().map(|v| *v as f32).collect();

    let expected = scalar_dot(&a, &b);
    let dense = bits_a.dot(&bits_b).expect("同长度");
    let sparse = sparse_bits_a.dot_sparse(&sparse_bits_b).expect("同长度");
    let sparse_expected = scalar_dot(&sparse_a, &sparse_b);
    let block_expected = scalar_dot(&block_sparse, &b);
    let bipolar = bipolar_dot(&packed_a, &packed_b, len);
    let bipolar_expected = scalar_dot(&bipolar_a, &bipolar_b);
    let block_sparse_result = block_bits_a.dot_sparse(&block_bits_b).expect("同长度");
    if dense != expected
        || sparse != sparse_expected
        || block_sparse_result != block_expected
        || bipolar != bipolar_expected
    {
        return Err(format!(
            "结果不一致：dense={dense}/{expected} sparse={sparse}/{sparse_expected} bipolar={bipolar}/{bipolar_expected}"
        ));
    }

    let ns_ternary = time(iters, || bits_a.dot(&bits_b).expect("同长度"));
    let ns_portable = time(iters, || bits_a.dot_portable(&bits_b).expect("同长度"));
    let ns_ternary_naive = time(iters, || bits_a.dot_naive(&bits_b).expect("同长度"));
    let ns_unrolled = time(iters, || {
        dot_words_unrolled(bits_a.pos(), bits_a.neg(), bits_b.pos(), bits_b.neg()).expect("同长度")
    });
    let ns_sparse = time(iters, || {
        sparse_bits_a.dot_sparse(&sparse_bits_b).expect("同长度")
    });
    let ns_block_sparse = time(iters, || {
        block_bits_a.dot_sparse(&block_bits_b).expect("同长度")
    });
    let ns_bipolar = time(iters, || bipolar_dot(&packed_a, &packed_b, len));
    let ns_bipolar_portable = time(iters, || bipolar_dot_portable(&packed_a, &packed_b, len));
    let ns_float = time(iters, || float_dot(&float_a, &float_b));
    let ns_float4 = time(iters, || float_dot_4acc(&float_a, &float_b));
    let per = |ns: u128| ns as f64 / len as f64;

    let obj = serde_json::json!({
        "len": len,
        "iters": iters,
        "blocks": len.div_ceil(BLOCK_BITS),
        "instruction_counts_per_block": {
            "bipolar_xnor_popc": OPS.bipolar,
            "ternary_4and_4popc": OPS.ternary,
            "sparse_probe": OPS.sparse_probe,
        },
        "theoretical_ops_per_trit": {
            "bipolar": OPS.bipolar as f64 / BLOCK_BITS as f64,
            "ternary": OPS.ternary as f64 / BLOCK_BITS as f64,
            "f32_scalar": 2.0,
        },
        "timings_ns": {
            "ternary_dense": ns_ternary,
            "ternary_portable": ns_portable,
            "ternary_dense_naive": ns_ternary_naive,
            "ternary_unrolled": ns_unrolled,
            "ternary_element_sparse_90pct": ns_sparse,
            "ternary_block_sparse_90pct": ns_block_sparse,
            "bipolar": ns_bipolar,
            "bipolar_portable": ns_bipolar_portable,
            "f32_scalar_naive": ns_float,
            "f32_scalar_4acc": ns_float4,
        },
        "ns_per_element": {
            "ternary_dense": per(ns_ternary),
            "ternary_portable": per(ns_portable),
            "ternary_dense_naive": per(ns_ternary_naive),
            "ternary_unrolled": per(ns_unrolled),
            "ternary_element_sparse_90pct": per(ns_sparse),
            "ternary_block_sparse_90pct": per(ns_block_sparse),
            "bipolar": per(ns_bipolar),
            "bipolar_portable": per(ns_bipolar_portable),
            "f32_scalar_naive": per(ns_float),
            "f32_scalar_4acc": per(ns_float4),
        },
        "two_popc_over_four": ns_ternary_naive as f64 / ns_ternary as f64,
        "portable_over_dispatch": ns_portable as f64 / ns_ternary as f64,
        "unrolled_over_loop": ns_ternary as f64 / ns_unrolled as f64,
        "nonzero_blocks": {
            "element_sparse_a": sparse_bits_a.nonzero_blocks(),
            "element_sparse_b": sparse_bits_b.nonzero_blocks(),
            "block_sparse_a": block_bits_a.nonzero_blocks(),
            "total": sparse_bits_a.pos().len(),
        },
        "checks": {
            "ternary_equals_scalar": true,
            "element_sparse_equals_scalar": true,
            "block_sparse_equals_scalar": true,
            "bipolar_equals_scalar": true,
        },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}
