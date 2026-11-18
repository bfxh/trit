//! 时间序列预测组件：SMA / EWMA / Holt 双参数平滑 + 误差指标。
//!
//! 服务防御侧的负载预测（`examples/ddos_shield` 的自适应预算）与任何需要
//! "预测下一个窗口"的实验层。设计口径：
//!
//! - **在线**：`Ewma`/`Holt` 是 O(1) 状态的流式更新器（每窗口一次 `update`），
//!   `forecast(h)` 多步外推（EWMA 平推，Holt 沿趋势线性外推）；
//! - **离线**：`sma` 对整段序列滚动平均；
//! - 指标 `mae`/`rmse`/`mape` 与训练侧的损失口径分开（评测用，不进梯度）。
//!
//! 边界：无季节项（Holt-Winters 未做）；f32 精度（与训练侧一致，
//! 跨平台用 f64 内部求值保证可复算的口径见 `activation` 的先例——本组件
//! 只有四则运算，无超越函数，f32 足够）。

/// 简单滚动平均：第 `i` 点 = 以 `i` 结尾的 `window` 窗口均值（前缀不足时
/// 用现有点）。`window = 0` 是错误用法，按 1 处理。
#[must_use]
pub fn sma(series: &[f32], window: usize) -> Vec<f32> {
    let w = window.max(1);
    let mut out = Vec::with_capacity(series.len());
    let mut sum = 0.0f32;
    for (i, &x) in series.iter().enumerate() {
        sum += x;
        if i >= w {
            sum -= series[i - w];
        }
        out.push(sum / (i + 1).min(w) as f32);
    }
    out
}

/// 指数加权移动平均（在线）。`alpha ∈ (0,1]`：越大越跟踪新数据。
#[derive(Clone, Debug)]
pub struct Ewma {
    /// 平滑系数。
    pub alpha: f32,
    state: Option<f32>,
}

impl Ewma {
    /// 新建；`alpha` 越界时取最近端（0.001 / 1.0）。
    #[must_use]
    pub fn new(alpha: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.001, 1.0),
            state: None,
        }
    }

    /// 吸收一个观测，返回当前平滑值。
    pub fn update(&mut self, x: f32) -> f32 {
        let next = match self.state {
            None => x,
            Some(s) => self.alpha * x + (1.0 - self.alpha) * s,
        };
        self.state = Some(next);
        next
    }

    /// 未来 `h` 步预测：无趋势，平推当前平滑值（未初始化返回 0）。
    #[must_use]
    pub fn forecast(&self, h: usize) -> Vec<f32> {
        vec![self.state.unwrap_or(0.0); h]
    }
}

/// Holt 双参数平滑（水平 + 线性趋势，在线）。
#[derive(Clone, Debug)]
pub struct Holt {
    /// 水平平滑系数。
    pub alpha: f32,
    /// 趋势平滑系数。
    pub beta: f32,
    level: Option<f32>,
    trend: f32,
}

impl Holt {
    /// 新建；系数越界取最近端。
    #[must_use]
    pub fn new(alpha: f32, beta: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.001, 1.0),
            beta: beta.clamp(0.001, 1.0),
            level: None,
            trend: 0.0,
        }
    }

    /// 吸收一个观测：先初始化（首两观测定水平与趋势），之后标准 Holt 递推。
    pub fn update(&mut self, x: f32) -> f32 {
        match self.level {
            None => {
                self.level = Some(x);
                x
            }
            Some(level) => {
                if self.trend == 0.0 && self.beta < 1.0 && self.level.is_some() && self.trend == 0.0
                {
                    // 第二个观测：以一次差分初始化趋势（仅一次）。
                    self.trend = x - level;
                    self.level = Some(x);
                    return x;
                }
                let prev_level = level;
                let new_level = self.alpha * x + (1.0 - self.alpha) * (level + self.trend);
                self.trend = self.beta * (new_level - prev_level) + (1.0 - self.beta) * self.trend;
                self.level = Some(new_level);
                new_level
            }
        }
    }

    /// 未来 `h` 步预测：`level + k·trend`。
    #[must_use]
    pub fn forecast(&self, h: usize) -> Vec<f32> {
        match self.level {
            None => vec![0.0; h],
            Some(level) => (1..=h).map(|k| level + k as f32 * self.trend).collect(),
        }
    }
}

/// 平均绝对误差。
#[must_use]
pub fn mae(actual: &[f32], predicted: &[f32]) -> f32 {
    actual
        .iter()
        .zip(predicted)
        .map(|(&a, &p)| (a - p).abs())
        .sum::<f32>()
        / actual.len().max(1) as f32
}

/// 均方根误差。
#[must_use]
pub fn rmse(actual: &[f32], predicted: &[f32]) -> f32 {
    let n = actual.len().max(1) as f32;
    (actual
        .iter()
        .zip(predicted)
        .map(|(&a, &p)| (a - p) * (a - p))
        .sum::<f32>()
        / n)
        .sqrt()
}

/// 平均绝对百分比误差（%）；真值为 0 的点跳过（全跳过时返回 0）。
#[must_use]
pub fn mape(actual: &[f32], predicted: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0usize;
    for (&a, &p) in actual.iter().zip(predicted) {
        if a.abs() > 1e-9 {
            sum += ((a - p) / a).abs();
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { 100.0 * sum / n as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sma_windows_and_prefix() {
        let s = [1.0, 2.0, 3.0, 4.0];
        let m1 = sma(&s, 1);
        assert_eq!(m1, s.to_vec(), "window=1 是恒等");
        let m2 = sma(&s, 2);
        assert!((m2[0] - 1.0).abs() < 1e-6, "前缀用现有点");
        assert!((m2[1] - 1.5).abs() < 1e-6);
        assert!((m2[3] - 3.5).abs() < 1e-6);
    }

    #[test]
    fn ewma_converges_to_constant_and_tracks_step() {
        let mut e = Ewma::new(0.5);
        for _ in 0..50 {
            e.update(10.0);
        }
        assert!((e.forecast(1)[0] - 10.0).abs() < 1e-3, "常数序列收敛到常值");
        e.update(20.0);
        let f = e.forecast(3);
        assert_eq!(f.len(), 3);
        assert!(
            f.iter().all(|&v| (v - 15.0).abs() < 1e-4),
            "阶跃后平推新平滑值"
        );
    }

    #[test]
    fn holt_recovers_linear_trend() {
        let mut h = Holt::new(0.5, 0.5);
        for t in 0..80 {
            h.update(2.0 * t as f32 + 5.0);
        }
        let f = h.forecast(1);
        let expect = 2.0 * 80.0 + 5.0;
        assert!(
            (f[0] - expect).abs() < 1.0,
            "线性趋势外推：预测 {} 期望≈{}",
            f[0],
            expect
        );
    }

    #[test]
    fn error_metrics_zero_on_equal_and_nonzero_on_deviation() {
        let a = [1.0, 2.0, 4.0];
        assert_eq!(mae(&a, &a), 0.0);
        assert_eq!(rmse(&a, &a), 0.0);
        assert_eq!(mape(&a, &a), 0.0);
        let p = [2.0, 2.0, 2.0];
        assert!((mae(&a, &p) - 1.0).abs() < 1e-6);
        assert!((rmse(&a, &p) - (5.0f32 / 3.0).sqrt()).abs() < 1e-6);
        assert!(mape(&a, &p) > 0.0);
        // 真值为 0 的点被 mape 跳过。
        assert_eq!(mape(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }
}
