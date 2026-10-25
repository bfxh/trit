//! 三值量化与直通估计器（STE）。
//!
//! QAT 的常用形态：latent 权重是 f32，前向用 `q = clamp(round(w / scale), -1, 1)`，
//! 反向把梯度**直通**回 latent。这里多给两个变体是因为直通并不唯一：
//! 饱和区的梯度该不该归零、边界附近该不该软化，直接影响收敛（见 README 的对照）。

/// 直通估计器变体。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ste {
    /// 经典 STE：梯度原样穿过量化，不设门。
    Identity,
    /// 截断 STE：`|w/scale|` 超过 `ratio` 时梯度归零（已饱和的权重不再动）。
    Clipped {
        /// 归零阈值（以 scale 为单位）。
        ratio: f32,
    },
    /// 软 STE：梯度按 `max(0, 1 - (w/scale)^2)` 衰减，边界连续。
    Soft,
}

impl Ste {
    /// 把上游对量化值 `q` 的梯度换成对 latent `w` 的梯度。
    pub fn backward(self, latent: f32, scale: f32, grad_q: f32) -> f32 {
        let scale = scale.max(f32::MIN_POSITIVE);
        let ratio = latent / scale;
        let gate = match self {
            Ste::Identity => 1.0,
            Ste::Clipped { ratio: limit } => {
                if ratio.abs() <= limit {
                    1.0
                } else {
                    0.0
                }
            }
            Ste::Soft => (1.0 - ratio * ratio).max(0.0),
        };
        grad_q * gate
    }
}

/// 一行的三值量化结果。
#[derive(Clone, Debug, PartialEq)]
pub struct Quantized {
    /// 每个元素的 `-1/0/+1`。
    pub q: Vec<i8>,
    /// 该行的 scale。
    pub scale: f32,
}

/// absmean 量化：`scale = mean(|w|)`，`q = clamp(round(w / scale), -1, 1)`。
///
/// 全零行返回 `scale = 0`、`q` 全零（调用方不应拿它做除法）。
pub fn ternarize(row: &[f32]) -> Quantized {
    if row.is_empty() {
        return Quantized {
            q: Vec::new(),
            scale: 0.0,
        };
    }
    let scale = row.iter().map(|w| w.abs()).sum::<f32>() / row.len() as f32;
    if scale == 0.0 {
        return Quantized {
            q: vec![0; row.len()],
            scale: 0.0,
        };
    }
    Quantized {
        q: quantize_with_scale(row, scale),
        scale,
    }
}

/// 用给定 scale 量化（scale 由外部学习时用）。
pub fn quantize_with_scale(row: &[f32], scale: f32) -> Vec<i8> {
    let scale = scale.max(f32::MIN_POSITIVE);
    row.iter()
        .map(|w| (w / scale).round().clamp(-1.0, 1.0) as i8)
        .collect()
}

/// scale 的梯度：`y = scale · Σ q·x` ⇒ `∂L/∂scale = dy · Σ q·x`。
pub fn scale_grad(q: &[i8], x: &[f32]) -> f32 {
    q.iter().zip(x).map(|(&qi, &xi)| f32::from(qi) * xi).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ternarize_signs_and_scale() {
        let row = [0.9, -0.4, 0.05, -1.2];
        let got = ternarize(&row);
        // mean(|w|) = (0.9+0.4+0.05+1.2)/4 = 0.6375
        assert!((got.scale - 0.6375).abs() < 1e-6);
        assert_eq!(got.q, vec![1, -1, 0, -1]);
    }

    #[test]
    fn zero_row_is_all_zero() {
        let got = ternarize(&[0.0, 0.0]);
        assert_eq!(got.scale, 0.0);
        assert_eq!(got.q, vec![0, 0]);
    }

    #[test]
    fn identity_passes_gradient() {
        let g = Ste::Identity.backward(5.0, 0.5, 0.25);
        assert_eq!(g, 0.25);
    }

    #[test]
    fn clipped_kills_saturated_gradient() {
        let ste = Ste::Clipped { ratio: 1.0 };
        assert_eq!(ste.backward(0.4, 0.5, 1.0), 1.0);
        assert_eq!(ste.backward(0.6, 0.5, 1.0), 0.0);
        assert_eq!(ste.backward(-0.6, 0.5, 1.0), 0.0);
    }

    #[test]
    fn soft_decays_towards_zero() {
        let ste = Ste::Soft;
        assert_eq!(ste.backward(0.0, 1.0, 1.0), 1.0);
        let near = ste.backward(0.9, 1.0, 1.0);
        let far = ste.backward(1.0, 1.0, 1.0);
        assert!(near > 0.0 && near < 1.0);
        assert_eq!(far, 0.0);
    }

    #[test]
    fn scale_grad_matches_formula() {
        let q = [1, 0, -1];
        let x = [0.5, -2.0, 3.0];
        assert!((scale_grad(&q, &x) - (-2.5)).abs() < 1e-6);
    }
}
