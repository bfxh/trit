//! 三值脉冲神经元网络（离散时间、整数膜电位、`{-1,0,+1}` 突触）。
//!
//! 每个 tick：膜电位先按 `decay` 泄漏，再叠加当前输入；越过 `±threshold` 就发一个
//! `+1`/`-1` 脉冲并把膜电位清零，随后进入 `refractory` 个 tick 的不应期。
//! 权重三值意味着连接只有「兴奋 / 无 / 抑制」三档，没有可调强度——这正是仿生侧
//! 关心的稀疏性来源。

/// 神经元数上限（`weights` 是方阵，平方增长）。
pub const MAX_NEURONS: usize = 4096;
/// 单次模拟的 tick 上限（`per_tick` 会按 tick 数分配）。
pub const MAX_TICKS: usize = 1_000_000;

/// 网络/模拟参数错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    /// `weights` 不是方阵。
    Shape {
        /// 行下标。
        neuron: usize,
        /// 期望列数（= 神经元数）。
        expected: usize,
        /// 实际列数。
        actual: usize,
    },
    /// 权重不是 `-1/0/1`。
    Weight {
        /// 行下标。
        neuron: usize,
        /// 列下标。
        synapse: usize,
        /// 实际权重。
        weight: i8,
    },
    /// 阈值非正。
    Threshold(i32),
    /// 泄漏系数非法。
    Decay(i32, i32),
    /// 规模超限。
    Limit(String),
    /// 上层驱动的参数配置非法。
    Config(String),
    /// 膜电位超出 `i32`。
    Overflow {
        /// tick 下标。
        tick: usize,
        /// 神经元下标。
        neuron: usize,
    },
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape {
                neuron,
                expected,
                actual,
            } => write!(
                formatter,
                "第 {neuron} 行有 {actual} 个突触，应为 {expected}（weights 必须是方阵）"
            ),
            Self::Weight {
                neuron,
                synapse,
                weight,
            } => write!(
                formatter,
                "第 {neuron} 行第 {synapse} 个权重 {weight} 不在 {{-1,0,1}}"
            ),
            Self::Threshold(value) => write!(formatter, "threshold 必须为正，实际 {value}"),
            Self::Decay(numerator, denominator) => write!(
                formatter,
                "decay 必须是非负分子与正分母，实际 {numerator}/{denominator}"
            ),
            Self::Limit(message) => write!(formatter, "规模超限：{message}"),
            Self::Config(message) => write!(formatter, "参数非法：{message}"),
            Self::Overflow { tick, neuron } => {
                write!(
                    formatter,
                    "第 {tick} tick 第 {neuron} 个神经元膜电位溢出 i32"
                )
            }
        }
    }
}

impl std::error::Error for NetworkError {}

/// 一个脉冲网络。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    /// `weights[i][j]`：`j` 到 `i` 的突触，取值 `-1/0/1`。
    pub weights: Vec<Vec<i8>>,
    /// 触发脉冲的绝对值阈值。
    pub threshold: i32,
    /// 泄漏：每 tick 膜电位先乘 `decay_num / decay_den`。
    pub decay: (i32, i32),
    /// 不应期 tick 数。
    pub refractory: u32,
}

impl Network {
    /// 随机稀疏网络（确定性种子）。
    pub fn random(size: usize, seed: u64) -> Self {
        let mut state = seed | 1;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let weights = (0..size)
            .map(|_| {
                (0..size)
                    .map(|_| match next() % 5 {
                        0 => 1,
                        1 => -1,
                        _ => 0,
                    })
                    .collect()
            })
            .collect();
        Self {
            weights,
            threshold: 3,
            decay: (3, 4),
            refractory: 1,
        }
    }

    /// 神经元数。
    pub fn size(&self) -> usize {
        self.weights.len()
    }

    /// 校验结构：方阵、权重 `{-1,0,1}`、阈值 > 0、decay 非负分子/正分母、规模预算。
    ///
    /// `weights` 是公开字段，`simulate` 不能再假定调用方构造的是合法网络（复审 M10）。
    pub fn validate(&self) -> Result<(), NetworkError> {
        let size = self.size();
        if size > MAX_NEURONS {
            return Err(NetworkError::Limit(format!(
                "神经元数 {size} 超过上限 {MAX_NEURONS}"
            )));
        }
        for (neuron, row) in self.weights.iter().enumerate() {
            if row.len() != size {
                return Err(NetworkError::Shape {
                    neuron,
                    expected: size,
                    actual: row.len(),
                });
            }
            if let Some((synapse, weight)) = row
                .iter()
                .enumerate()
                .find(|(_, weight)| !(-1..=1).contains(*weight))
            {
                return Err(NetworkError::Weight {
                    neuron,
                    synapse,
                    weight: *weight,
                });
            }
        }
        if self.threshold <= 0 {
            return Err(NetworkError::Threshold(self.threshold));
        }
        if self.decay.0 < 0 || self.decay.1 <= 0 {
            return Err(NetworkError::Decay(self.decay.0, self.decay.1));
        }
        Ok(())
    }
}

/// 一个 tick 之后的状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// 膜电位。
    pub membrane: Vec<i32>,
    /// 剩余不应期。
    pub refractory: Vec<u32>,
    /// 本 tick 的脉冲（`-1/0/1`）。
    pub spikes: Vec<i32>,
}

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

/// 跑 `ticks` 步；初始膜电位全 0，无外部输入。
pub fn simulate(network: &Network, ticks: usize) -> Result<Stats, NetworkError> {
    simulate_with_drive(network, ticks, &[])
}

/// 同 [`simulate`]，但每个 tick 额外给所有神经元加一个外部驱动。
///
/// 复审 M10：先 `validate`，再用 `i64` 中间量算膜电位——非法网络/极端参数返回
/// 结构化错误，而不是在 `weights[target]` 处 panic 或让 `i32` 静默溢出。
pub fn simulate_with_drive(
    network: &Network,
    ticks: usize,
    external: &[i32],
) -> Result<Stats, NetworkError> {
    Ok(simulate_with_input(network, ticks, |tick, _| {
        external.get(tick).copied().unwrap_or(0)
    })?
    .stats)
}

/// 一次模拟的完整读数。
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// 统计。
    pub stats: Stats,
    /// 末态（可回灌给 [`simulate_with_input_from`] 续跑）。
    pub state: State,
    /// 每个神经元本次发出的脉冲数。
    pub fired: Vec<u32>,
    /// 每个神经元本次的脉冲极性合计（`+1` 与 `-1` 相加）。
    pub polarity: Vec<i32>,
}

/// 同 [`simulate`]，但每个 tick、每个神经元可以拿到不同的外部驱动。
///
/// `input(tick, neuron)` 返回该 tick 加给该神经元的电位；从全零膜电位起步。
pub fn simulate_with_input<F>(
    network: &Network,
    ticks: usize,
    input: F,
) -> Result<Run, NetworkError>
where
    F: FnMut(usize, usize) -> i32,
{
    simulate_with_input_from(network, &State::default(), ticks, input)
}

/// 从给定末态续跑：膜电位/不应期/上一步脉冲都接着用，其余语义同 [`simulate_with_input`]。
///
/// `start` 的向量长度不足时按 0 补齐，超出时截断——跨阶段续跑不必自己对齐长度。
pub fn simulate_with_input_from<F>(
    network: &Network,
    start: &State,
    ticks: usize,
    input: F,
) -> Result<Run, NetworkError>
where
    F: FnMut(usize, usize) -> i32,
{
    run(network, start, ticks, input)
}

fn run<F>(network: &Network, start: &State, ticks: usize, mut input: F) -> Result<Run, NetworkError>
where
    F: FnMut(usize, usize) -> i32,
{
    network.validate()?;
    if ticks > MAX_TICKS {
        return Err(NetworkError::Limit(format!(
            "ticks {ticks} 超过上限 {MAX_TICKS}"
        )));
    }
    let size = network.size();
    let mut state = State {
        membrane: sized(&start.membrane, size),
        refractory: sized(&start.refractory, size),
        spikes: sized(&start.spikes, size),
    };
    let mut fired_counts = vec![0u32; size];
    let mut polarity = vec![0i32; size];
    let mut stats = Stats {
        ticks,
        spikes: 0,
        positive: 0,
        negative: 0,
        active_fraction: 0.0,
        per_tick: Vec::with_capacity(ticks),
    };
    let mut ever_spiked = vec![false; size];
    for tick in 0..ticks {
        let drive: Vec<i64> = (0..size)
            .map(|target| {
                network.weights[target]
                    .iter()
                    .zip(state.spikes.iter())
                    .map(|(weight, spike)| i64::from(*weight) * i64::from(*spike))
                    .sum()
            })
            .collect();
        let mut fired_now = 0usize;
        for neuron in 0..size {
            if state.refractory[neuron] > 0 {
                state.refractory[neuron] -= 1;
                state.spikes[neuron] = 0;
                let leaked = i64::from(state.membrane[neuron]) * i64::from(network.decay.0)
                    / i64::from(network.decay.1);
                state.membrane[neuron] =
                    i32::try_from(leaked).map_err(|_| NetworkError::Overflow { tick, neuron })?;
                continue;
            }
            let leaked = i64::from(state.membrane[neuron]) * i64::from(network.decay.0)
                / i64::from(network.decay.1);
            let kick = i64::from(input(tick, neuron));
            let potential = leaked + drive[neuron] + kick;
            let spike = if potential >= i64::from(network.threshold) {
                1
            } else if potential <= -i64::from(network.threshold) {
                -1
            } else {
                0
            };
            if spike > 0 {
                stats.positive += 1;
            } else if spike < 0 {
                stats.negative += 1;
            }
            if spike != 0 {
                fired_now += 1;
                fired_counts[neuron] += 1;
                polarity[neuron] += spike;
                ever_spiked[neuron] = true;
                state.membrane[neuron] = 0;
                state.refractory[neuron] = network.refractory;
            } else {
                state.membrane[neuron] = i32::try_from(potential)
                    .map_err(|_| NetworkError::Overflow { tick, neuron })?;
            }
            state.spikes[neuron] = spike;
        }
        stats.spikes += fired_now;
        stats.per_tick.push(fired_now);
    }
    stats.active_fraction = if size == 0 {
        0.0
    } else {
        ever_spiked.iter().filter(|fired| **fired).count() as f64 / size as f64
    };
    Ok(Run {
        stats,
        state,
        fired: fired_counts,
        polarity,
    })
}

/// 把续跑状态对齐到当前网络规模：不足补 0，超出截断。
fn sized<T: Clone + Default>(source: &[T], size: usize) -> Vec<T> {
    let mut values = source.to_vec();
    values.resize(size, T::default());
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single(weight: i8) -> Network {
        Network {
            weights: vec![vec![weight]],
            threshold: 1,
            decay: (1, 1),
            refractory: 0,
        }
    }

    #[test]
    fn invalid_networks_are_rejected_instead_of_panicking() {
        // 复审 M10：ragged 矩阵曾在 weights[target] 处 panic；权重 2、decay 分母 0 都被静默接受。
        let ragged = Network {
            weights: vec![vec![0, 0], vec![0]],
            threshold: 1,
            decay: (1, 1),
            refractory: 0,
        };
        assert!(matches!(ragged.validate(), Err(NetworkError::Shape { .. })));
        assert!(matches!(
            simulate(&ragged, 3),
            Err(NetworkError::Shape { .. })
        ));

        let bad_weight = Network {
            weights: vec![vec![2]],
            threshold: 1,
            decay: (1, 1),
            refractory: 0,
        };
        assert!(matches!(
            bad_weight.validate(),
            Err(NetworkError::Weight { .. })
        ));

        let zero_denominator = Network {
            weights: vec![vec![0]],
            threshold: 1,
            decay: (1, 0),
            refractory: 0,
        };
        assert!(matches!(
            zero_denominator.validate(),
            Err(NetworkError::Decay(1, 0))
        ));

        let zero_threshold = Network {
            weights: vec![vec![0]],
            threshold: 0,
            decay: (1, 1),
            refractory: 0,
        };
        assert!(matches!(
            zero_threshold.validate(),
            Err(NetworkError::Threshold(0))
        ));

        // tick 预算：per_tick 按 tick 数分配，必须先拒。
        assert!(matches!(
            simulate(&single(0), MAX_TICKS + 1),
            Err(NetworkError::Limit(_))
        ));
        assert_eq!(simulate(&single(0), 0).expect("0 tick 合法").ticks, 0);
    }

    #[test]
    fn quiet_network_stays_quiet() {
        let network = Network {
            weights: vec![vec![0, 0], vec![0, 0]],
            threshold: 3,
            decay: (1, 2),
            refractory: 1,
        };
        let stats = simulate(&network, 10).expect("合法网络");
        assert_eq!(stats.spikes, 0);
        assert_eq!(stats.active_fraction, 0.0);
    }

    #[test]
    fn external_kick_starts_self_excitation() {
        let stats = simulate_with_drive(&single(1), 3, &[1]).expect("合法网络");
        assert_eq!(stats.per_tick, vec![1, 1, 1]);
        assert_eq!(stats.positive, 3);
        assert_eq!(stats.spikes_per_neuron_tick(1), 1.0);
    }

    #[test]
    fn negative_drive_produces_inhibitory_spike() {
        let stats = simulate_with_drive(&single(0), 3, &[-1, 0, 0]).expect("合法网络");
        assert_eq!(stats.per_tick, vec![1, 0, 0]);
        assert_eq!(stats.negative, 1);
        assert_eq!(stats.positive, 0);
    }

    #[test]
    fn refractory_blocks_consecutive_spikes() {
        let mut network = single(0);
        network.refractory = 1;
        let stats = simulate_with_drive(&network, 4, &[1, 1, 1, 1]).expect("合法网络");
        assert_eq!(stats.per_tick, vec![1, 0, 1, 0]);
    }
}
