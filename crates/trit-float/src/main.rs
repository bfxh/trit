//! `trit-float`：三值浮点的精度读数与「同比特」对比。

use std::process::ExitCode;

use trit_float::{Format, LOG2_3, RoundMode, TritFloat, relative_error_bound};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("stats") => stats(&args[1..]),
        Some("range") => range(&args[1..]),
        Some("rounding") => rounding(&args[1..]),
        _ => Err("用法：trit-float <stats|range|rounding> [--trials N]".to_string()),
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
    fn unit(&mut self) -> f64 {
        (self.next() % 10_000) as f64 / 10_000.0
    }
}

fn stats(args: &[String]) -> Result<(), String> {
    let trials = match args.iter().position(|arg| arg == "--trials") {
        Some(index) => args
            .get(index + 1)
            .ok_or("--trials 缺少参数")?
            .parse::<usize>()
            .map_err(|_| "--trials 不是整数".to_string())?,
        None => 2000,
    };
    if trials == 0 {
        return Err("--trials 必须大于 0".to_string());
    }
    let mut rows = Vec::new();
    for digits in [6u32, 12, 18, 24] {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ u64::from(digits));
        let mut max_error = 0.0f64;
        let mut sum_error = 0.0f64;
        for _ in 0..trials {
            let mantissa = 1.0 + rng.unit() * 2.0;
            let exponent = (rng.next() % 21) as i32 - 10;
            let sign = if rng.next().is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let value = mantissa * 3f64.powi(exponent) * sign;
            let quantized = TritFloat::from_f64(value, digits).expect("有限值");
            let error = ((quantized.to_f64() - value) / value).abs();
            max_error = max_error.max(error);
            sum_error += error;
        }
        let bits = f64::from(digits) * LOG2_3;
        let bound = relative_error_bound(digits);
        rows.push(serde_json::json!({
            "digits": digits,
            "bits": bits,
            "measured_max_relative_error": max_error,
            "measured_mean_relative_error": sum_error / trials as f64,
            "ternary_bound": bound,
            "binary_bound_same_bits": 2f64.powf(-bits),
            "ternary_over_binary": bound / 2f64.powf(-bits),
        }));
    }
    // 要匹配 f64（53 bit 精度：相对误差 ≤ 2^-53），三值需要多少位？
    let needed = (1.0 + 53.0 / LOG2_3).ceil();
    let needed_bits = needed * LOG2_3;
    let obj = serde_json::json!({
        "trials": trials,
        "rows": rows,
        "f64_equivalent": {
            "binary_bits": 53,
            "ternary_digits": needed,
            "ternary_bits": needed_bits,
            "overhead_bits": needed_bits - 53.0,
        },
    });
    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    Ok(())
}

/// 同比特下三进制格式与 IEEE 动态范围 / 精度对比。
///
/// 公平性按"分配对齐"：三进制的一部分格式**匹配 IEEE 的尾数位**，
/// 另一部分**匹配 IEEE 的指数位**；另给一个"范围优先"的极端值作对照。
fn range(_args: &[String]) -> Result<(), String> {
    let log10_2 = std::f64::consts::LOG10_2;
    // (名字, 总比特, 尾数位, 指数位, log10 最大/最小)
    let ieee = [
        (
            "IEEE half",
            16.0,
            10u32,
            5u32,
            65504f64.log10(),
            -24.0 * log10_2,
        ),
        (
            "IEEE single",
            32.0,
            23,
            8,
            3.402_823_466e38f64.log10(),
            -149.0 * log10_2,
        ),
        (
            "IEEE double",
            64.0,
            52,
            11,
            308.254_715_157_4,
            -1074.0 * log10_2,
        ),
    ];
    let mut rows = Vec::new();
    for (name, bits, mantissa_bits, exponent_bits, max_log10, min_log10) in ieee {
        rows.push(serde_json::json!({
            "format": name,
            "bits": bits,
            "mantissa_bits": mantissa_bits,
            "exponent_bits": exponent_bits,
            "log10_range": max_log10 - min_log10,
            "precision_digits": f64::from(mantissa_bits) * log10_2,
        }));
    }
    for bits in [16u32, 32, 64] {
        let trits = ((f64::from(bits) - 1.0) / LOG2_3).floor() as u32;
        let (mantissa_bits, exponent_bits) = match bits {
            16 => (10u32, 5u32),
            32 => (23, 8),
            _ => (52, 11),
        };
        let p_matched = ((f64::from(mantissa_bits)) / LOG2_3).round() as u32;
        let e_matched = ((f64::from(exponent_bits)) / LOG2_3).ceil() as u32;
        let variants = [
            ("匹配尾数位", p_matched.min(trits.saturating_sub(1)), 0u32),
            ("匹配指数位", trits.saturating_sub(e_matched).max(1), 0),
            ("范围优先", trits / 2, 0),
        ];
        for (label, precision, taper) in variants {
            let exponent_digits = trits.saturating_sub(precision);
            let format = Format::new(precision, exponent_digits, taper);
            rows.push(serde_json::json!({
                "format": format!("三值 {label}（p={precision}, E={exponent_digits}, taper={taper}）"),
                "bits": format.total_bits(),
                "budget_bits": bits,
                "max_exponent": format.max_exponent(),
                "log10_range": format.log10_range(),
                "precision_digits": format.log10_error(),
            }));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "rows": rows })).unwrap_or_default()
    );
    Ok(())
}

/// 平局处理方式的方向偏差：只在"恰好中点"的样本上才看得出差别。
fn rounding(_args: &[String]) -> Result<(), String> {
    let mut rows = Vec::new();
    for precision in [4u32, 8, 12] {
        let limit = (3i128.pow(precision) - 1) / 2;
        for mode in [RoundMode::HalfAway, RoundMode::HalfEven] {
            let mut tie_signed = 0.0f64;
            let mut tie_abs = 0.0f64;
            let mut tie_n = 0usize;
            for exponent in -4i32..=4 {
                let scale = 3f64.powi(exponent);
                for m in -limit..limit {
                    let value = (m as f64 + 0.5) * scale;
                    if value == 0.0 {
                        continue;
                    }
                    let Some(q) = TritFloat::from_f64_with(value, precision, mode) else {
                        continue;
                    };
                    let rel = (q.to_f64() - value) / value;
                    tie_signed += rel;
                    tie_abs += rel.abs();
                    tie_n += 1;
                }
            }
            // 对照：随机（非中点）样本。
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ u64::from(precision));
            let mut uniform_signed = 0.0f64;
            let mut uniform_n = 0usize;
            for _ in 0..5000 {
                let mantissa = 1.0 + rng.unit() * 2.0;
                let exponent = (rng.next() % 13) as i32 - 6;
                let sign = if rng.next().is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                };
                let value = mantissa * 3f64.powi(exponent) * sign;
                let Some(q) = TritFloat::from_f64_with(value, precision, mode) else {
                    continue;
                };
                uniform_signed += (q.to_f64() - value) / value;
                uniform_n += 1;
            }
            rows.push(serde_json::json!({
                "precision": precision,
                "mode": match mode {
                    RoundMode::HalfAway => "half-away",
                    RoundMode::HalfEven => "half-even",
                },
                "tie_samples": tie_n,
                "tie_mean_signed_relative_error": tie_signed / tie_n.max(1) as f64,
                "tie_mean_abs_relative_error": tie_abs / tie_n.max(1) as f64,
                "uniform_samples": uniform_n,
                "uniform_mean_signed_relative_error": uniform_signed / uniform_n.max(1) as f64,
            }));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "rows": rows })).unwrap_or_default()
    );
    Ok(())
}
