//! `project` 的默认健康检查：随机输入或真实 oracle 输入上，原网络与投影网络的一致率。
//! 读取 `--samples` / `--agree-from`；不一致样本只留前 `max_mismatches` 个。

use serde_json::{Value, json};

use crate::net::{Layer, Net};
use crate::trit::Trit;

use super::{Parsed, load_oracle, value};

fn sign_of(acc: i32) -> Trit {
    match acc.signum() {
        1 => Trit::Pos,
        -1 => Trit::Neg,
        _ => Trit::Zero,
    }
}

/// 采样 `samples` 个输入，比较原网络与投影网络的输出。
pub fn agree(
    original: &Net,
    projected: &Net,
    basis: &Layer,
    parsed: &Parsed,
    max_mismatches: usize,
) -> Result<Value, String> {
    let samples = value(parsed, "samples")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(64usize);
    let from = value(parsed, "agree-from")
        .ok()
        .map(load_oracle)
        .transpose()?;
    let (cols, k) = (basis.cols(), basis.rows());
    let real = from.as_ref().map(|oracle| oracle.inputs());
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let (mut same, mut total) = (0usize, 0usize);
    let mut mismatches: Vec<Value> = Vec::new();
    for index in 0..samples {
        let input: Vec<Trit> = match real {
            Some(inputs) if !inputs.is_empty() => inputs[index % inputs.len()].clone(),
            _ => (0..cols)
                .map(|_| match next() % 3 {
                    0 => Trit::Neg,
                    1 => Trit::Zero,
                    _ => Trit::Pos,
                })
                .collect(),
        };
        let Ok(left) = original.forward(&input) else {
            continue;
        };
        let reduced: Vec<Trit> = (0..k)
            .map(|row| {
                let mut acc: i32 = 0;
                for (col, value) in input.iter().enumerate() {
                    let w = basis.trit(row, col).map(|t| t.to_i8()).unwrap_or(0);
                    acc += i32::from(w) * i32::from(value.to_i8());
                }
                sign_of(acc)
            })
            .collect();
        let Ok(right) = projected.forward(&reduced) else {
            continue;
        };
        total += 1;
        if left == right {
            same += 1;
        } else if mismatches.len() < max_mismatches {
            let bits = |values: &[Trit]| values.iter().map(|t| t.to_i8()).collect::<Vec<_>>();
            mismatches.push(json!({
                "input": bits(&input),
                "original": bits(&left),
                "projected": bits(&right),
            }));
        }
    }
    Ok(json!({
        "samples": total,
        "same": same,
        "rate": if total == 0 { 0.0 } else { same as f64 / total as f64 },
        "source": if real.is_some() { "oracle" } else { "random" },
        "mismatches": mismatches,
    }))
}
