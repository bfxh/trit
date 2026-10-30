//! 损失与梯度：层级加权 CE（k 条平面）+ 0.5·BCE（valid），与 Python 训练同口径。
//!
//! `logits` 是 NHWC（每像素 `k*3+1` 通道）：平面 `i` 的三个 logit 在 `i*3..i*3+3`，
//! valid 的 logit 在 `k*3`。CE / BCE 都是对全部像素取均值的 reduction。

/// 一次损失计算的结果。
pub struct LossOut {
    pub loss: f32,
    /// `∂L/∂logits`，布局与输入相同。
    pub grad: Vec<f32>,
    /// 每条平面在**部件像素**上的 argmax 准确率。
    pub plane_acc: Vec<f32>,
    /// valid 通道在全部像素上的准确率。
    pub valid_acc: f32,
    /// 深度平面在深度有效像素上的 argmax 准确率（没开深度头时为空）。
    pub depth_acc: Vec<f32>,
}

/// 深度头单独的损失结果。
pub struct DepthOut {
    pub loss: f32,
    pub grad: Vec<f32>,
    pub acc: Vec<f32>,
}

/// 深度平面损失（独立深度头的输出，布局 `hw × planes*3`）。
pub fn depth_loss_grad(
    logits: &[f32],
    planes: &[Vec<u8>],
    valid: &[u8],
    w: &[f32],
    weight: f32,
) -> DepthOut {
    let hw = valid.len();
    let dp = planes.len();
    assert_eq!(logits.len(), hw * dp * 3);
    let mut grad = vec![0.0f32; logits.len()];
    let mut hit = vec![0usize; dp];
    let mut seen = 0usize;
    let mut loss = 0f64;
    let dinv = 1.0 / valid.iter().filter(|&&v| v == 1).count().max(1) as f32;
    for p in 0..hw {
        if valid[p] != 1 {
            continue;
        }
        seen += 1;
        let base = p * dp * 3;
        for i in 0..dp {
            let z = [
                logits[base + i * 3],
                logits[base + i * 3 + 1],
                logits[base + i * 3 + 2],
            ];
            let m = z[0].max(z[1]).max(z[2]);
            let e = [(z[0] - m).exp(), (z[1] - m).exp(), (z[2] - m).exp()];
            let sum = e[0] + e[1] + e[2];
            let t = planes[i][p] as usize;
            loss += f64::from(weight * w[i] * -(e[t] / sum).max(1e-12).ln());
            for c in 0..3 {
                let onehot = if c == t { 1.0 } else { 0.0 };
                grad[base + i * 3 + c] += weight * w[i] * (e[c] / sum - onehot) * dinv;
            }
            let mut best = 0usize;
            for c in 1..3 {
                if z[c] > z[best] {
                    best = c;
                }
            }
            if best == t {
                hit[i] += 1;
            }
        }
    }
    DepthOut {
        loss: (loss * f64::from(dinv)) as f32,
        grad,
        acc: hit
            .iter()
            .map(|&h| {
                if seen > 0 {
                    h as f32 / seen as f32
                } else {
                    0.0
                }
            })
            .collect(),
    }
}

/// 深度平面监督：`planes` 是额外通道上的三值平面，`w` 是这些平面的归一化权重。
pub struct DepthSupervision<'a> {
    pub planes: &'a [Vec<u8>],
    pub valid: &'a [u8],
    pub w: &'a [f32],
    /// 深度项在总损失里的权重。
    pub weight: f32,
}

/// 层级权重：`w_i ∝ 1/(1+i)` 归一化（高位平面权重更大），与 Python 一致。
pub fn plane_weights(k: usize) -> Vec<f32> {
    let mut w: Vec<f32> = (0..k).map(|i| 1.0 / (1.0 + i as f32)).collect();
    let sum: f32 = w.iter().sum();
    for v in &mut w {
        *v /= sum;
    }
    w
}

/// 计算总损失、`∂L/∂logits` 与准确率。
pub fn loss_and_grad(
    logits: &[f32],
    planes: &[Vec<u8>],
    valid: &[u8],
    hw: usize,
    k: usize,
    w: &[f32],
) -> LossOut {
    loss_and_grad_full(logits, planes, valid, hw, k, w, None)
}

/// 带可选深度平面的完整损失。
pub fn loss_and_grad_full(
    logits: &[f32],
    planes: &[Vec<u8>],
    valid: &[u8],
    hw: usize,
    k: usize,
    w: &[f32],
    depth: Option<&DepthSupervision>,
) -> LossOut {
    let ch = k * 3 + 1;
    let dplanes = depth.map(|d| d.planes.len()).unwrap_or(0);
    assert_eq!(logits.len() % hw, 0);
    let stride = logits.len() / hw;
    assert!(
        stride >= ch + dplanes * 3,
        "通道不足：{stride} < {}",
        ch + dplanes * 3
    );
    assert_eq!(planes.len(), k);
    let inv = 1.0f32 / hw as f32;
    let mut grad = vec![0.0f32; logits.len()];
    let mut loss = 0.0f64;
    let mut hit = vec![0usize; k];
    let mut seen = 0usize;
    let mut vhit = 0usize;

    for p in 0..hw {
        let base = p * stride;
        if valid[p] == 1 {
            seen += 1;
        }
        for i in 0..k {
            let z = [
                logits[base + i * 3],
                logits[base + i * 3 + 1],
                logits[base + i * 3 + 2],
            ];
            let m = z[0].max(z[1]).max(z[2]);
            let e = [(z[0] - m).exp(), (z[1] - m).exp(), (z[2] - m).exp()];
            let sum = e[0] + e[1] + e[2];
            let sm = [e[0] / sum, e[1] / sum, e[2] / sum];
            let t = planes[i][p] as usize;
            loss += f64::from(w[i]) * f64::from(-sm[t].max(1e-12).ln());
            for c in 0..3 {
                let onehot = if c == t { 1.0 } else { 0.0 };
                grad[base + i * 3 + c] += w[i] * (sm[c] - onehot) * inv;
            }
            if valid[p] == 1 {
                let mut best = 0usize;
                for c in 1..3 {
                    if z[c] > z[best] {
                        best = c;
                    }
                }
                if best == t {
                    hit[i] += 1;
                }
            }
        }

        let v = logits[base + k * 3];
        let t = f32::from(valid[p]);
        // BCEWithLogits：max(v,0) − v·t + ln(1+e^{−|v|})
        loss += 0.5 * f64::from(v.max(0.0) - v * t + (1.0 + (-v.abs()).exp()).ln());
        let s = 1.0 / (1.0 + (-v).exp());
        grad[base + k * 3] = 0.5 * (s - t) * inv;
        if (s > 0.5) == (valid[p] == 1) {
            vhit += 1;
        }
    }

    let mut depth_hit = vec![0usize; dplanes];
    let mut depth_seen = 0usize;
    if let Some(d) = depth {
        // 先数有效像素，梯度用固定归一化（逐像素变化的 1/n 会把前几个像素放大上千倍）。
        let dcount = d.valid.iter().filter(|&&v| v == 1).count();
        let dinv = 1.0 / dcount.max(1) as f32;
        for p in 0..hw {
            if d.valid[p] != 1 {
                continue;
            }
            depth_seen += 1;
            let base = p * stride + ch;
            for i in 0..dplanes {
                let z = [
                    logits[base + i * 3],
                    logits[base + i * 3 + 1],
                    logits[base + i * 3 + 2],
                ];
                let m = z[0].max(z[1]).max(z[2]);
                let e = [(z[0] - m).exp(), (z[1] - m).exp(), (z[2] - m).exp()];
                let sum = e[0] + e[1] + e[2];
                let t = d.planes[i][p] as usize;
                let prob = e[t] / sum;
                loss += f64::from(d.weight * d.w[i] * -prob.max(1e-12).ln());
                for c in 0..3 {
                    let onehot = if c == t { 1.0 } else { 0.0 };
                    grad[base + i * 3 + c] += d.weight * d.w[i] * (e[c] / sum - onehot) * dinv;
                }
                let mut best = 0usize;
                for c in 1..3 {
                    if z[c] > z[best] {
                        best = c;
                    }
                }
                if best == t {
                    depth_hit[i] += 1;
                }
            }
        }
    }
    let depth_acc = depth_hit
        .iter()
        .map(|&h| {
            if depth_seen > 0 {
                h as f32 / depth_seen as f32
            } else {
                0.0
            }
        })
        .collect();

    let plane_acc = hit
        .iter()
        .map(|&h| {
            if seen > 0 {
                h as f32 / seen as f32
            } else {
                0.0
            }
        })
        .collect();
    LossOut {
        // 与梯度同口径：两项都是对像素取均值
        loss: (loss * f64::from(inv)) as f32,
        grad,
        plane_acc,
        valid_acc: vhit as f32 / hw as f32,
        depth_acc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(seed: &mut u64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                ((*seed % 2001) as f32 - 1000.0) / 900.0
            })
            .collect()
    }

    #[test]
    fn grad_matches_finite_difference() {
        let (k, hw) = (2usize, 5usize);
        let ch = k * 3 + 1;
        let mut seed = 0xfeed_face_1234_5678u64;
        let logits = fill(&mut seed, hw * ch);
        let planes: Vec<Vec<u8>> = (0..k)
            .map(|i| (0..hw).map(|p| ((p + i) % 3) as u8).collect())
            .collect();
        let valid: Vec<u8> = (0..hw).map(|p| if p % 3 == 0 { 1 } else { 0 }).collect();
        let w = plane_weights(k);
        let out = loss_and_grad(&logits, &planes, &valid, hw, k, &w);

        let h = 1e-3;
        for i in 0..logits.len() {
            let mut up = logits.clone();
            up[i] += h;
            let mut dn = logits.clone();
            dn[i] -= h;
            let l_up = loss_and_grad(&up, &planes, &valid, hw, k, &w).loss as f64;
            let l_dn = loss_and_grad(&dn, &planes, &valid, hw, k, &w).loss as f64;
            let num = (l_up - l_dn) / (2.0 * f64::from(h));
            let got = f64::from(out.grad[i]);
            assert!((num - got).abs() < 2e-3, "grad[{i}]: {got} vs {num}");
        }
    }

    #[test]
    fn weights_are_normalized() {
        let w = plane_weights(5);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        for pair in w.windows(2) {
            assert!(pair[0] >= pair[1]);
        }
    }
}
