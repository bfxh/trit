//! 与模型实现无关的抽样审计（任意大小）。
//!
//! ## 三级判据
//!
//! | 级别 | 做法 | 能回答 | 代价 |
//! |---|---|---|---|
//! | 精确 | 穷举等价（`equiv` / M0 的 `DIFF=0`） | 「有没有任何输入让两者不同」 | 只当输入可枚举 |
//! | **抽样** | **本模块** | 「在这批样本上差多少」 | 与模型大小无关 |
//! | 不可判定 | —— | 「任意模型在算什么」没有算法（Rice 定理） | —— |
//!
//! 这一层的接口只要 [`BlackBox`]（给输入、返回输出），所以卷积、浮点、
//! 上千维输入的模型都能跑；代价是结论只对样本集成立，不是等价证明。
//! 报告里必须同时记录样本来源与 `tol`，否则读数不可比。

/// 任意模型的最小接口。
pub trait BlackBox {
    /// 一次前向：输入向量 → 输出向量。
    fn forward(&self, input: &[f32]) -> Vec<f32>;
}

/// 抽样一致性读数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Consistency {
    /// 样本数。
    pub samples: usize,
    /// 至少有一维差异超过 `tol` 的样本数。
    pub changed: usize,
    /// 所有维度上的最大绝对差。
    pub max_abs: f32,
    /// 所有维度上的平均绝对差。
    pub mean_abs: f32,
}

/// 比较两个模型在同一批样本上的输出差异。
pub fn consistency(
    a: &dyn BlackBox,
    b: &dyn BlackBox,
    samples: &[Vec<f32>],
    tol: f32,
) -> Result<Consistency, String> {
    let mut changed = 0usize;
    let mut max_abs = 0.0f32;
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for (i, sample) in samples.iter().enumerate() {
        let ya = a.forward(sample);
        let yb = b.forward(sample);
        if ya.len() != yb.len() {
            return Err(format!(
                "样本 {i} 的输出维度不同：{} vs {}",
                ya.len(),
                yb.len()
            ));
        }
        let mut sample_changed = false;
        for (x, y) in ya.iter().zip(yb.iter()) {
            if !x.is_finite() || !y.is_finite() {
                return Err(format!("样本 {i} 含非有限输出"));
            }
            let d = (x - y).abs();
            max_abs = max_abs.max(d);
            sum += f64::from(d);
            count += 1;
            if d > tol {
                sample_changed = true;
            }
        }
        if sample_changed {
            changed += 1;
        }
    }
    Ok(Consistency {
        samples: samples.len(),
        changed,
        max_abs,
        mean_abs: if count == 0 {
            0.0
        } else {
            (sum / count as f64) as f32
        },
    })
}

/// 扰动效应读数：同一个模型在「原样」与「被扰动」两种状态下，输出差多少。
///
/// 典型用法：`perturbed` 是「把某一层/通道清零或加噪」的包装，
/// 于是读数回答「这个单元承重多少」——与 `intervene` 的手写网络版同一个问法，
/// 只是这里不要求网络是离散小网。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effect {
    /// 样本数。
    pub samples: usize,
    /// 输出被改变的样本数（任一维超过 `tol`）。
    pub changed: usize,
    /// 最大绝对差。
    pub max_abs: f32,
    /// 平均绝对差。
    pub mean_abs: f32,
}

/// 计算扰动效应。
pub fn perturbation(
    base: &dyn BlackBox,
    perturbed: &dyn BlackBox,
    samples: &[Vec<f32>],
    tol: f32,
) -> Result<Effect, String> {
    let c = consistency(base, perturbed, samples, tol)?;
    Ok(Effect {
        samples: c.samples,
        changed: c.changed,
        max_abs: c.max_abs,
        mean_abs: c.mean_abs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Identity;

    impl BlackBox for Identity {
        fn forward(&self, input: &[f32]) -> Vec<f32> {
            input.to_vec()
        }
    }

    struct Scaled(f32);

    impl BlackBox for Scaled {
        fn forward(&self, input: &[f32]) -> Vec<f32> {
            input.iter().map(|x| x * self.0).collect()
        }
    }

    struct WrongDim;

    impl BlackBox for WrongDim {
        fn forward(&self, _input: &[f32]) -> Vec<f32> {
            vec![0.0]
        }
    }

    fn samples() -> Vec<Vec<f32>> {
        vec![vec![0.0, 1.0], vec![2.0, -3.0], vec![0.5, 0.25]]
    }

    #[test]
    fn identical_models_have_zero_difference() {
        let c = consistency(&Identity, &Identity, &samples(), 1e-6).unwrap();
        assert_eq!(c.samples, 3);
        assert_eq!(c.changed, 0);
        assert_eq!(c.max_abs, 0.0);
        assert_eq!(c.mean_abs, 0.0);
    }

    #[test]
    fn scaled_model_is_flagged() {
        // 1% 缩放：最大差出现在 -3.0 → 0.03。
        let c = consistency(&Identity, &Scaled(1.01), &samples(), 1e-3).unwrap();
        assert_eq!(c.changed, 3, "三个样本都应有维度超过 1e-3");
        assert!((c.max_abs - 0.03).abs() < 1e-4, "max_abs = {}", c.max_abs);
        assert!(c.mean_abs > 0.0);
    }

    #[test]
    fn tight_tolerance_flags_everything() {
        let c = consistency(&Identity, &Scaled(1.0001), &samples(), 1e-9).unwrap();
        assert_eq!(c.changed, 3);
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let err = consistency(&Identity, &WrongDim, &samples(), 1e-6).unwrap_err();
        assert!(err.contains("维度不同"), "{err}");
    }

    #[test]
    fn perturbation_wraps_consistency() {
        let e = perturbation(&Identity, &Scaled(2.0), &samples(), 1e-6).unwrap();
        assert_eq!(e.samples, 3);
        assert_eq!(e.changed, 3);
        assert!((e.max_abs - 3.0).abs() < 1e-5, "max_abs = {}", e.max_abs);
    }
}
