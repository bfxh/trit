//! f32 微调差分 → 三值最小补丁。
//!
//! 输入是同一个张量的 base / tuned 两个 f32 工件（例如 HF 预训练与微调后的
//! safetensors）。先把两者按 absmean 三值化，再只对**三值发生变化**的坐标生成
//! `SetTrit` / `SetBias`；`trim` 按原始 f32 差分幅度保留 top 比例。scale 不迁移，
//! 保留 base 的 scale（行为判据只看 trit/bias）。

use crate::analyze::edit_key;
use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::safetensors::Weights;

/// 差分统计。
#[derive(Clone, Debug, PartialEq)]
pub struct DiffStats {
    /// 三值发生变化的 trit 数（trim 前）。
    pub changed_trits: usize,
    /// 三值发生变化的 bias 数（trim 前）。
    pub changed_bias: usize,
    /// 保留的 trit 数。
    pub kept_trits: usize,
    /// 保留的 bias 数。
    pub kept_bias: usize,
    /// 实际使用的 trim 比例。
    pub trim: f32,
}

impl DiffStats {
    /// trim 前的变化总数。
    #[must_use]
    pub fn changed(&self) -> usize {
        self.changed_trits + self.changed_bias
    }

    /// trim 后保留的编辑数。
    #[must_use]
    pub fn kept(&self) -> usize {
        self.kept_trits + self.kept_bias
    }
}

/// 把 base/tuned 的 f32 权重差转成三值补丁；返回补丁、base 的三值网络与统计。
///
/// `trim=1.0` 保留全部变化，`trim=0.5` 按 `|tuned - base|` 保留 top 50%。
pub fn diff_weights(
    base: &Weights,
    tuned: &Weights,
    base_bias: Option<&[f32]>,
    tuned_bias: Option<&[f32]>,
    trim: f32,
) -> Result<(Patch, Net, DiffStats)> {
    if base.rows != tuned.rows || base.cols != tuned.cols {
        return Err(Error::Wire(format!(
            "base/tuned 形状不一致：{}×{} vs {}×{}",
            base.rows, base.cols, tuned.rows, tuned.cols
        )));
    }
    let base_layer = base.to_layer_with_bias(base_bias)?;
    let tuned_layer = tuned.to_layer_with_bias(tuned_bias)?;
    let base_net = Net::new(vec![base_layer])?;
    let tuned_net = Net::new(vec![tuned_layer])?;
    let mut candidates: Vec<(f32, Edit)> = Vec::new();
    for row in 0..base.rows {
        for col in 0..base.cols {
            let from = base_net.layers()[0].trit(row, col).expect("trit");
            let to = tuned_net.layers()[0].trit(row, col).expect("trit");
            if from == to {
                continue;
            }
            let index = row * base.cols + col;
            let importance = (tuned.values[index] - base.values[index]).abs();
            if !importance.is_finite() {
                continue;
            }
            candidates.push((
                importance,
                Edit::SetTrit {
                    layer: 0,
                    row,
                    col,
                    to,
                },
            ));
        }
        let from = base_net.layers()[0].bias_trit(row).expect("bias");
        let to = tuned_net.layers()[0].bias_trit(row).expect("bias");
        if from == to {
            continue;
        }
        let importance = match (base_bias, tuned_bias) {
            (Some(left), Some(right)) => (right[row] - left[row]).abs(),
            _ => 1.0,
        };
        if !importance.is_finite() {
            continue;
        }
        candidates.push((importance, Edit::SetBias { layer: 0, row, to }));
    }
    let changed_trits = candidates
        .iter()
        .filter(|(_, edit)| matches!(edit, Edit::SetTrit { .. }))
        .count();
    let changed_bias = candidates.len() - changed_trits;
    candidates.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| edit_key(&left.1).cmp(&edit_key(&right.1)))
    });
    let trim = if trim.is_finite() {
        trim.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let keep = ((candidates.len() as f32) * trim).ceil() as usize;
    let selected: Vec<Edit> = candidates
        .iter()
        .take(keep)
        .map(|(_, edit)| edit.clone())
        .collect();
    let kept_trits = selected
        .iter()
        .filter(|edit| matches!(edit, Edit::SetTrit { .. }))
        .count();
    let kept_bias = selected.len() - kept_trits;
    Ok((
        Patch::new(selected),
        base_net,
        DiffStats {
            changed_trits,
            changed_bias,
            kept_trits,
            kept_bias,
            trim,
        },
    ))
}
