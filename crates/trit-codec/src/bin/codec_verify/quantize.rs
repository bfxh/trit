//! 有损量化审计：TWN 三值量化的误差界与同比特预算的二值基线。
//!
//! 这是本库**唯一有损**的编码器，往返只到 `{−scale, 0, +scale}`，不能按“可逆”口径审。
//! 这里给出两类可复算读数：
//!
//! - 误差界（库不变式）：`|w − dequant(q(w))| ≤ scale·max(beta, 1−beta)`；
//!   `Unknown` 项另有 `|w| ≤ beta·scale`。逐元素检查，失败留首个反例。
//! - 重建误差（统计读数）：同一批权重上算三元 MSE、1-bit 符号基线、2-bit 四电平基线。
//!   三元的实际存储是双导轨 2 bit/trit，因此 2-bit 基线是**同比特预算**；
//!   打包后的 1.6 bit/trit 另给每比特折算值，不混为一谈。
//!
//! 归类见 `docs/research/TERNARY-AUDIT.md`：这是 **`[B]`**（二进制宿主上的模拟税），
//! 不是 `[A]` 的原生红利。

use trit_codec::quantize::{self, dequantize};
use trit_core::value::Ternary;

use super::corpus::Rng;
use super::report::{Check, Report};

const KINDS: [&str; 6] = [
    "uniform", "normal", "sparse", "outlier", "positive", "zeros",
];
const BETAS: [f32; 4] = [0.3, 0.5, 0.7, 0.9];

pub fn run(report: &mut Report, seed: u64, quick: bool) {
    let sizes: &[usize] = if quick {
        &[32, 257]
    } else {
        &[32, 257, 1024, 4096]
    };
    let mut bound = Check::new("quantize.error_bound");
    let mut idem = Check::new("quantize.idempotent");
    let mut nan = Check::new("quantize.nan_policy");
    let mut values_total = 0u64;
    let mut unknown_total = 0u64;
    let mut max_ratio = 0.0f64;
    let mut mse = [0.0f64; 3];
    let mut nrmse = 0.0f64;
    let mut datasets = 0u64;
    let mut rng = Rng::new(seed);

    for kind in KINDS {
        for &size in sizes {
            let values = generate(kind, size, &mut rng);
            for beta in BETAS {
                let q = quantize::quantize_with_beta(&values, beta);
                let dq = dequantize(&q.trits, q.scale);
                let scale = f64::from(q.scale.abs());
                let ratio_bound = f64::from(beta.max(1.0 - beta));
                let mut bad = None;
                for (index, (&w, &r)) in values.iter().zip(dq.iter()).enumerate() {
                    let error = f64::from((w - r).abs());
                    let limit = scale * ratio_bound + scale * 1e-6 + 1e-9;
                    if error > limit {
                        bad = Some(format!(
                            "{kind}[{size}] beta={beta} i={index} w={w} r={r} err={error} limit={limit}"
                        ));
                        break;
                    }
                    if scale > 0.0 {
                        max_ratio = max_ratio.max(error / scale);
                    }
                    if q.trits[index] == Ternary::Unknown {
                        let unknown_limit = f64::from(beta.abs()) * scale + scale * 1e-6 + 1e-9;
                        if f64::from(w.abs()) > unknown_limit {
                            bad = Some(format!(
                                "{kind}[{size}] beta={beta} i={index} Unknown 超出阈值：w={w}"
                            ));
                            break;
                        }
                    }
                }
                bound.case(bad.is_none(), || bad.unwrap_or_default());

                let again = quantize::quantize_with_beta(&dq, beta);
                idem.case(again.trits == q.trits, || {
                    format!("{kind}[{size}] beta={beta} 再量化 trit 不稳定")
                });

                values_total += values.len() as u64;
                unknown_total += q.trits.iter().filter(|t| **t == Ternary::Unknown).count() as u64;
                let (mt, m1, m2) = mse_of(&values, &dq, f64::from(q.scale.abs()));
                for (slot, value) in mse.iter_mut().zip([mt, m1, m2]) {
                    *slot += value;
                }
                if scale > 0.0 {
                    nrmse += (mt / values.len() as f64).sqrt() / scale;
                    datasets += 1;
                }
            }
        }
    }

    let values = vec![f32::NAN, 1.0, -1.0, 0.0];
    let q = quantize::quantize(&values);
    nan.case(q.trits[0] == Ternary::Unknown, || {
        format!("NaN 未压成 Unknown: {:?}", q.trits)
    });

    let mut deterministic = Check::new("quantize.metrics_deterministic");
    let mut replay = Rng::new(seed);
    let mut replay_mse = 0.0f64;
    for kind in KINDS {
        for &size in sizes {
            let values = generate(kind, size, &mut replay);
            for beta in BETAS {
                let q = quantize::quantize_with_beta(&values, beta);
                let dq = dequantize(&q.trits, q.scale);
                replay_mse += mse_of(&values, &dq, f64::from(q.scale.abs())).0;
            }
        }
    }
    deterministic.case(replay_mse.to_bits() == mse[0].to_bits(), || {
        format!(
            "同种子重算 MSE 不一致：{:.9e} vs {:.9e}",
            replay_mse, mse[0]
        )
    });

    report.finish(bound);
    report.finish(idem);
    report.finish(nan);
    report.finish(deterministic);
    report.metric("quantize.values", values_total);
    report.metric("quantize.unknown_share", ratio(unknown_total, values_total));
    report.metric("quantize.max_error_ratio", format!("{max_ratio:.6}"));
    report.metric("quantize.nrmse.ternary", ratio_f(nrmse, datasets as f64));
    report.metric(
        "quantize.mse.ternary",
        format!("{:.6}", mse[0] / values_total as f64),
    );
    report.metric(
        "quantize.mse.binary_1bit",
        format!("{:.6}", mse[1] / values_total as f64),
    );
    report.metric(
        "quantize.mse.binary_2bit",
        format!("{:.6}", mse[2] / values_total as f64),
    );
    report.metric(
        "quantize.mse_ratio.ternary_over_binary_2bit",
        ratio_f(mse[0], mse[2]),
    );
    report.metric("quantize.bits.ternary_dual_rail", "2");
    report.metric("quantize.bits.ternary_packed", "1.6");
    report.metric("quantize.bits.binary_1bit", "1");
    report.metric("quantize.bits.binary_2bit", "2");
    report.metric(
        "quantize.mse_per_bit.ternary_dual_rail",
        ratio_f(mse[0], 2.0 * values_total as f64),
    );
    report.metric(
        "quantize.mse_per_bit.ternary_packed",
        ratio_f(mse[0], 1.6 * values_total as f64),
    );
    report.metric(
        "quantize.mse_per_bit.binary_2bit",
        ratio_f(mse[2], 2.0 * values_total as f64),
    );
}

/// 返回（三元 MSE、1-bit 符号基线 MSE、2-bit 四电平基线 MSE）。
fn mse_of(values: &[f32], dq: &[f32], scale: f64) -> (f64, f64, f64) {
    let mut sum = [0.0f64; 3];
    for (index, &w) in values.iter().enumerate() {
        let ideal = f64::from(w);
        sum[0] += (ideal - f64::from(dq[index])).powi(2);
        // 1 bit：符号 ±scale（0 归正，避免偷偷多出第三档）。
        let b1 = if w >= 0.0 { scale } else { -scale };
        sum[1] += (ideal - b1).powi(2);
        // 2 bit：四电平 {−scale, −scale/3, +scale/3, +scale}。
        let step = 2.0 * scale / 3.0;
        let level = if step == 0.0 {
            0.0
        } else {
            ((ideal + scale) / step).round().clamp(0.0, 3.0)
        };
        let b2 = -scale + level * step;
        sum[2] += (ideal - b2).powi(2);
    }
    (sum[0], sum[1], sum[2])
}

fn generate(kind: &str, size: usize, rng: &mut Rng) -> Vec<f32> {
    let mut out = Vec::with_capacity(size);
    for index in 0..size {
        let unit = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let value = match kind {
            "uniform" => (unit * 8.0 - 4.0) as f32,
            "normal" => {
                let mut sum = 0.0;
                for _ in 0..12 {
                    sum += (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
                }
                (sum - 6.0) as f32
            }
            "sparse" => {
                if rng.below(100) < 3 {
                    (unit * 4.0 - 2.0) as f32
                } else {
                    0.0
                }
            }
            "outlier" => {
                if index % 97 == 0 {
                    if unit < 0.5 { -100.0 } else { 100.0 }
                } else {
                    (unit * 0.1 - 0.05) as f32
                }
            }
            "positive" => (unit * 4.0) as f32,
            _ => 0.0,
        };
        out.push(value);
    }
    out
}

fn ratio(part: u64, total: u64) -> String {
    if total == 0 {
        "0.000000".to_string()
    } else {
        format!("{:.6}", part as f64 / total as f64)
    }
}

fn ratio_f(part: f64, total: f64) -> String {
    if total == 0.0 {
        "0.000000".to_string()
    } else {
        format!("{:.6}", part / total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_bound_holds_on_tiny_hand_case() {
        let values = [0.1f32, -0.2, 0.0];
        let q = quantize::quantize_with_beta(&values, 0.7);
        let dq = dequantize(&q.trits, q.scale);
        for (w, r) in values.iter().zip(dq.iter()) {
            assert!((w - r).abs() <= q.scale * 0.7 + 1e-6);
        }
    }
}
