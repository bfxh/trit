//! 把 [`GroupedLayer`] 接进 [`Oracle`] 的采样路径。
//!
//! 本仓现有的验证流程（`agree` / `check` / `Oracle::sampled_behavior`）都接受 `Oracle`：
//! 一组 `(输入, 期望输出)` 对，配合 `Oracle::outputs_of(net)` 做行为判等。
//!
//! `GroupedLayer` 不是 `Net`（它的 scale 是跨行分组的，无法无损退化成 `Layer` 的 per-row
//! scale），所以不能直接喂给 `Oracle::sampled_behavior(&Net, ..)`。这里补一条**从层自身
//! 采样生成 `Oracle`** 的通道：
//!
//! ```text
//! let oracle = layer.sampled_oracle(vars, samples, seed)?;
//! // 之后可以像普通 Oracle 一样使用：oracle.outputs()、oracle.check_arch(..) 等
//! ```
//!
//! 采样是**确定性**的（LCG），同一 `seed` 必得同一批输入，便于复现与对照。

use crate::error::{Error, Result};
use crate::grouped::GroupedLayer;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 采样上限：避免一次生成过大的 `Oracle`。
pub const MAX_SAMPLES: usize = 1 << 16;

fn sign_trit(value: f32) -> Trit {
    if value > 1e-9 {
        Trit::Pos
    } else if value < -1e-9 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

impl GroupedLayer {
    /// 按确定性种子采样 `vars` 维 trit 输入，生成可直接参与本仓验证流程的 [`Oracle`]。
    ///
    /// - `vars` 必须落在 `1..=cols()`；
    /// - `samples` 必须落在 `1..=MAX_SAMPLES`；
    /// - 期望输出取该层前向的**符号**（与仓库"行为判等只看符号"的约定一致）。
    pub fn sampled_oracle(&self, vars: usize, samples: usize, seed: u64) -> Result<Oracle> {
        if vars == 0 || vars > self.cols() {
            return Err(Error::Shape {
                what: "oracle vars",
                expected: self.cols(),
                got: vars,
            });
        }
        if samples == 0 || samples > MAX_SAMPLES {
            return Err(Error::DomainTooLarge {
                vars: samples,
                limit: MAX_SAMPLES,
            });
        }
        let mut state = seed;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 33) % 3) as u8
        };
        let mut inputs = Vec::with_capacity(samples);
        let mut outputs = Vec::with_capacity(samples);
        for _ in 0..samples {
            let input: Vec<Trit> = (0..vars)
                .map(|_| match next() {
                    0 => Trit::Neg,
                    1 => Trit::Zero,
                    _ => Trit::Pos,
                })
                .collect();
            let features: Vec<f32> = input.iter().map(|trit| f32::from(trit.to_i8())).collect();
            let output = self.forward(&features)?;
            outputs.push(output.iter().map(|value| sign_trit(*value)).collect());
            inputs.push(input);
        }
        Oracle::new(inputs, outputs)
    }
}
