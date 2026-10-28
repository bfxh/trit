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
    let ch = k * 3 + 1;
    assert_eq!(logits.len(), hw * ch);
    assert_eq!(planes.len(), k);
    let inv = 1.0f32 / hw as f32;
    let mut grad = vec![0.0f32; logits.len()];
    let mut loss = 0.0f64;
    let mut hit = vec![0usize; k];
    let mut seen = 0usize;
    let mut vhit = 0usize;

    for p in 0..hw {
        let base = p * ch;
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
