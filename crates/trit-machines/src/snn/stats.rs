//! 一次脉冲模拟的统计读数。

/// 一次模拟的统计。
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// tick 数。
    pub ticks: usize,
    /// 脉冲总数。
    pub spikes: usize,
    /// 其中兴奋 / 抑制脉冲数。
    pub positive: usize,
    /// 抑制脉冲数。
    pub negative: usize,
    /// 任意时刻发过脉冲的神经元比例。
    pub active_fraction: f64,
    /// 每个 tick 的脉冲数。
    pub per_tick: Vec<usize>,
}

impl Stats {
    /// 平均每个神经元每 tick 的脉冲数（稀疏度指标）。
    pub fn spikes_per_neuron_tick(&self, neurons: usize) -> f64 {
        if neurons == 0 || self.ticks == 0 {
            return 0.0;
        }
        self.spikes as f64 / (neurons * self.ticks) as f64
    }
}
