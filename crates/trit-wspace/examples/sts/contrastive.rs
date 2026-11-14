//! 对比学习（in-batch negatives）的梯度，供 STS 训练示例共用。
//!
//! 目标：`L = -log softmax_j(cos(u_i, v_j)/τ)[i]` —— 正例是配对的 `v_i`，
//! 负例是同一批里的其它 `v_j`。这正是 sentence-transformers / MiniLM 的训练目标，
//! 与 `classifier_sts_train` 里用的余弦-MSE 不同：它对**相对排序**敏感，
//! 不要求余弦值本身逼近 `score/5`。

use super::bert::{Activation, BertLayer, Grads, accumulate};
use super::common::{Sentence, mean};

/// 余弦与其对两侧向量的梯度。
pub fn cosine_with_grad(u: &[f32], v: &[f32]) -> (f32, Vec<f32>, Vec<f32>) {
    let dot: f32 = u.iter().zip(v).map(|(a, b)| a * b).sum();
    let un: f32 = u.iter().map(|a| a * a).sum::<f32>().sqrt();
    let vn: f32 = v.iter().map(|b| b * b).sum::<f32>().sqrt();
    let denominator = (un * vn).max(1e-12);
    let cos = dot / denominator;
    let d_u: Vec<f32> = u
        .iter()
        .zip(v)
        .map(|(a, b)| b / denominator - cos * a / (un * un).max(1e-12))
        .collect();
    let d_v: Vec<f32> = u
        .iter()
        .zip(v)
        .map(|(a, b)| a / denominator - cos * b / (vn * vn).max(1e-12))
        .collect();
    (cos, d_u, d_v)
}

pub fn accumulate_grads(target: &mut Grads, source: &Grads) {
    accumulate(&mut target.wq, &source.wq);
    accumulate(&mut target.bq, &source.bq);
    accumulate(&mut target.wk, &source.wk);
    accumulate(&mut target.bk, &source.bk);
    accumulate(&mut target.wv, &source.wv);
    accumulate(&mut target.bv, &source.bv);
    accumulate(&mut target.wo, &source.wo);
    accumulate(&mut target.bo, &source.bo);
    accumulate(&mut target.ln1_w, &source.ln1_w);
    accumulate(&mut target.ln1_b, &source.ln1_b);
    accumulate(&mut target.ff1_w, &source.ff1_w);
    accumulate(&mut target.ff1_b, &source.ff1_b);
    accumulate(&mut target.ff2_w, &source.ff2_w);
    accumulate(&mut target.ff2_b, &source.ff2_b);
    accumulate(&mut target.ln2_w, &source.ln2_w);
    accumulate(&mut target.ln2_b, &source.ln2_b);
}

pub fn scale_grads(grads: &mut Grads, factor: f32) {
    for slot in [
        &mut grads.wq,
        &mut grads.bq,
        &mut grads.wk,
        &mut grads.bk,
        &mut grads.wv,
        &mut grads.bv,
        &mut grads.wo,
        &mut grads.bo,
        &mut grads.ln1_w,
        &mut grads.ln1_b,
        &mut grads.ff1_w,
        &mut grads.ff1_b,
        &mut grads.ff2_w,
        &mut grads.ff2_b,
        &mut grads.ln2_w,
        &mut grads.ln2_b,
    ] {
        for value in slot.iter_mut() {
            *value *= factor;
        }
    }
}

/// 一批句对的对比损失与梯度（in-batch negatives）。
pub fn contrastive_grads(
    layer: &BertLayer,
    pairs: &[(Sentence, Sentence)],
    tau: f32,
) -> (f32, Grads) {
    let n = pairs.len();
    if n < 2 {
        return (0.0, Grads::default());
    }
    let dim = pairs[0].0[0].len();

    let mut left_acts: Vec<Activation> = Vec::with_capacity(n);
    let mut left_pooled: Vec<Vec<f32>> = Vec::with_capacity(n);
    for (left, _) in pairs {
        let (out, act) = layer.forward_cached(left);
        left_pooled.push(mean(&out, dim));
        left_acts.push(act);
    }
    let mut right_acts: Vec<Activation> = Vec::with_capacity(n);
    let mut right_pooled: Vec<Vec<f32>> = Vec::with_capacity(n);
    for (_, right) in pairs {
        let (out, act) = layer.forward_cached(right);
        right_pooled.push(mean(&out, dim));
        right_acts.push(act);
    }

    // 余弦矩阵与 softmax 梯度
    let mut grad_cos = vec![vec![0.0f32; n]; n];
    let mut loss = 0.0f32;
    for i in 0..n {
        let logits: Vec<f32> = (0..n)
            .map(|j| cosine(&left_pooled[i], &right_pooled[j]) / tau)
            .collect();
        let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = logits.iter().map(|value| (value - max).exp()).collect();
        let total: f32 = exps.iter().sum();
        let probs: Vec<f32> = exps.iter().map(|value| value / total).collect();
        loss += -probs[i].max(1e-12).ln();
        for j in 0..n {
            let mut grad = probs[j];
            if j == i {
                grad -= 1.0;
            }
            grad_cos[i][j] = grad / tau / n as f32;
        }
    }

    // 反传到 pooled，再按 token 广播
    let mut grad_left = vec![vec![0.0f32; dim]; n];
    let mut grad_right = vec![vec![0.0f32; dim]; n];
    for i in 0..n {
        for j in 0..n {
            let weight = grad_cos[i][j];
            if weight == 0.0 {
                continue;
            }
            let (_, d_u, d_v) = cosine_with_grad(&left_pooled[i], &right_pooled[j]);
            for k in 0..dim {
                grad_left[i][k] += weight * d_u[k];
                grad_right[j][k] += weight * d_v[k];
            }
        }
    }

    let mut grads = Grads::default();
    for i in 0..n {
        let count = left_acts[i].input.len() as f32;
        let token_grad: Sentence = (0..left_acts[i].input.len())
            .map(|_| grad_left[i].iter().map(|value| value / count).collect())
            .collect();
        let (piece, _) = layer.backward(&left_acts[i], &token_grad);
        accumulate_grads(&mut grads, &piece);
    }
    for j in 0..n {
        let count = right_acts[j].input.len() as f32;
        let token_grad: Sentence = (0..right_acts[j].input.len())
            .map(|_| grad_right[j].iter().map(|value| value / count).collect())
            .collect();
        let (piece, _) = layer.backward(&right_acts[j], &token_grad);
        accumulate_grads(&mut grads, &piece);
    }

    (loss / n as f32, grads)
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let ln: f32 = left.iter().map(|a| a * a).sum::<f32>().sqrt();
    let rn: f32 = right.iter().map(|b| b * b).sum::<f32>().sqrt();
    if ln == 0.0 || rn == 0.0 {
        0.0
    } else {
        dot / (ln * rn)
    }
}
