//! 三值量化的 STE（直通估计）口径，本 crate 自带（不再依赖 `trit-learn`）。
//!
//! 与 `trit-learn::quant::Ste` 逐位一致：`backward(latent, scale, grad_q)` 只改**梯度**，
//! 不改前向的量化值（前向永远是 `q = round(w/scale).clamp(-1,1)`）。

/// 直通估计的三种门。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ste {
    /// 经典 STE：梯度原样穿过，不设门。
    Identity,
    /// 截断 STE：`|w/scale| > ratio` 的权重已饱和，梯度归零。
    Clipped {
        /// 归零阈值（以 scale 为单位）。
        ratio: f32,
    },
    /// 软 STE：梯度按 `max(0, 1 - (w/scale)^2)` 衰减，边界连续。
    Soft,
}

impl Ste {
    /// 把对量化值 `q` 的梯度换成对 latent `w` 的梯度。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_match_the_reference_formulas() {
        // scale=1.0、latent 取 0.5 / 1.5
        assert_eq!(Ste::Identity.backward(1.5, 1.0, 2.0), 2.0);
        // Clipped：|w/scale| <= 1 放行，超过就归零
        assert_eq!(Ste::Clipped { ratio: 1.0 }.backward(0.5, 1.0, 2.0), 2.0);
        assert_eq!(Ste::Clipped { ratio: 1.0 }.backward(1.5, 1.0, 2.0), 0.0);
        // Soft：1 − (w/scale)²
        assert!((Ste::Soft.backward(0.5, 1.0, 2.0) - 1.5).abs() < 1e-6);
        assert_eq!(Ste::Soft.backward(2.0, 1.0, 2.0), 0.0);
        // scale 为 0 时不除零（按 MIN_POSITIVE 处理）
        assert!(Ste::Identity.backward(0.0, 0.0, 1.0).is_finite());
    }
}
