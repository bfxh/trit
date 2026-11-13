//! `project` 的默认健康检查：随机输入上，原网络与投影网络的一致率。
//!
//! 回答「这次压缩能不能接受」——不必再手动串 `probe` / `certify`。

use serde_json::{Value, json};

use crate::net::{Layer, Net};
use crate::trit::Trit;

/// 采样 `samples` 个随机三值输入，把原输入按 `basis` 投到 k 维后分别前向，返回一致率。
pub fn agree(original: &Net, projected: &Net, basis: &Layer, samples: usize) -> Value {
    let cols = basis.cols();
    let k = basis.rows();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let (mut same, mut total) = (0usize, 0usize);
    for _ in 0..samples {
        let input: Vec<Trit> = (0..cols)
            .map(|_| match next() % 3 {
                0 => Trit::Neg,
                1 => Trit::Zero,
                _ => Trit::Pos,
            })
            .collect();
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
                if acc > 0 {
                    Trit::Pos
                } else if acc < 0 {
                    Trit::Neg
                } else {
                    Trit::Zero
                }
            })
            .collect();
        let Ok(right) = projected.forward(&reduced) else {
            continue;
        };
        total += 1;
        if left == right {
            same += 1;
        }
    }
    json!({
        "samples": total,
        "same": same,
        "rate": if total == 0 { 0.0 } else { same as f64 / total as f64 },
    })
}
