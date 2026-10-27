//! 激活函数及其导数。
//!
//! 约定：导数由**激活后的输出** `y` 给出（`tanh` 的导数是 `1 - y²`），
//! 这样调用方只需缓存前向输出，不必另存 pre-activation。

/// 激活函数（前向 + 由输出求导）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// `tanh`：三值训练的默认选择（有界、对称，和 `±1` 标签同一量纲）。
    Tanh,
    /// `ReLU`：`max(0, x)`；导数在 0 处按次梯度约定取 0。
    Relu,
    /// `sigmoid`：`1 / (1 + e^(-x))`；用于把输出压到 `(0, 1)`。
    Sigmoid,
}

impl Activation {
    /// 前向。
    pub fn forward(self, x: f32) -> f32 {
        match self {
            Self::Tanh => x.tanh(),
            Self::Relu => x.max(0.0),
            Self::Sigmoid => 1.0 / (1.0 + (-x).exp()),
        }
    }

    /// 由激活后的输出 `y` 求 `dL/d(pre-activation)` 的系数。
    pub fn backward(self, y: f32) -> f32 {
        match self {
            Self::Tanh => 1.0 - y * y,
            Self::Relu => {
                if y > 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Sigmoid => y * (1.0 - y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 数值导数（中心差分）。
    fn numeric(activation: Activation, x: f32) -> f32 {
        let h = 1e-3;
        (activation.forward(x + h) - activation.forward(x - h)) / (2.0 * h)
    }

    #[test]
    fn forward_values() {
        assert!((Activation::Tanh.forward(0.0)).abs() < 1e-7);
        assert_eq!(Activation::Relu.forward(-2.0), 0.0);
        assert_eq!(Activation::Relu.forward(3.0), 3.0);
        assert!((Activation::Sigmoid.forward(0.0) - 0.5).abs() < 1e-7);
        assert!((Activation::Sigmoid.forward(20.0) - 1.0).abs() < 1e-6);
        assert!((Activation::Sigmoid.forward(-20.0)).abs() < 1e-6);
    }

    #[test]
    fn backward_matches_numeric_derivative() {
        for activation in [Activation::Tanh, Activation::Relu, Activation::Sigmoid] {
            for x in [-2.5, -0.7, 0.3, 1.9] {
                let y = activation.forward(x);
                let analytic = activation.backward(y);
                let numeric = numeric(activation, x);
                assert!(
                    (analytic - numeric).abs() < 1e-3,
                    "{activation:?} 在 x={x}：解析 {analytic} vs 数值 {numeric}"
                );
            }
        }
    }

    #[test]
    fn tanh_output_is_bounded() {
        for x in [-30.0, -1.0, 0.0, 1.0, 30.0] {
            let y = Activation::Tanh.forward(x);
            assert!(y.abs() <= 1.0);
        }
    }
}
