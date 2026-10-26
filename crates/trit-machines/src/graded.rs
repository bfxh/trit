//! 分级神经元回路 M0（Issue #166）：自主搜寻 → 等待 → 双向/散布交流 → 收敛结论。
//!
//! 这一层是 [`crate::snn`] 之上的**调度器**，不改放电规则，只决定「谁在什么时候被驱动」：
//!
//! | 档 | 做什么 | 产物 |
//! |---|---|---|
//! | [`Grade::Find`] | 按 `seeds` 把神经元切块，逐块单独驱动 `probe_ticks`，用**块外被带动的脉冲数**打分 | 得分最高的 `groups` 块 |
//! | [`Grade::Wait`] | 命中后静默 `wait_ticks`，让残余膜电位自己漏掉 | 冷却后的末态 |
//! | [`Grade::Exchange`] | 每轮各组轮流发言 `exchange_ticks`，其余组只靠突触接收；可选三值 Hebbian 强化 | 每轮每组一票 |
//! | [`Grade::Conclude`] | 各组票合成三态判决，连续 `quorum` 轮不变即收敛 | [`Verdict`] |
//!
//! 「自主」的含义：搜寻**不需要指定目标**——它找的是「最能带动别人的一块」。找到后按
//! 得分降序取 `groups` 块：`groups = 2` 是双向来回，`groups > 2` 是散布式轮转。
//!
//! 边界（M0 明确不做）：
//! - 不是学习算法：强化只把 `0` 连成 `±1`，不翻已有符号、不改阈值/泄漏/不应期；
//! - 搜寻只看块外脉冲数，不做模式匹配；没有命中（得分全 0）就停在 [`Grade::Find`]，
//!   结论 [`Ternary::Unknown`]；
//! - 轮数耗尽仍不稳定时判 `Unknown`（不猜），档位记 [`Grade::Exchange`]。

use trit_core::value::Ternary;

use crate::snn::{self, Network, NetworkError, State};

/// 参与交流的组数上限。
pub const MAX_GROUPS: usize = 8;
/// 交流轮数上限。
pub const MAX_ROUNDS: usize = 4096;

/// 回路当前停在哪一档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    /// 自主搜寻（还没找到能带动别人的一块）。
    Find,
    /// 命中后的静默等待。
    Wait,
    /// 双向/散布交流中。
    Exchange,
    /// 已收敛。
    Conclude,
}

/// 回路参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GradeConfig {
    /// 搜寻时把神经元切成几块（≥ 2，且 ≤ 神经元数）。
    pub seeds: usize,
    /// 每块探针跑多少 tick。
    pub probe_ticks: usize,
    /// 命中后静默多少 tick。
    pub wait_ticks: usize,
    /// 参与交流的组数：2 = 双向，> 2 = 散布。
    pub groups: usize,
    /// 每组每轮发言多少 tick。
    pub exchange_ticks: usize,
    /// 最多多少轮。
    pub rounds: usize,
    /// 连续多少轮判决不变算收敛。
    pub quorum: usize,
    /// 发言组每个 tick 的外部驱动（必须为正）。
    pub drive: i32,
    /// 是否做三值 Hebbian 强化。
    pub reinforce: bool,
}

impl Default for GradeConfig {
    fn default() -> Self {
        Self {
            seeds: 4,
            probe_ticks: 8,
            wait_ticks: 4,
            groups: 2,
            exchange_ticks: 4,
            rounds: 16,
            quorum: 3,
            drive: 2,
            reinforce: true,
        }
    }
}

impl GradeConfig {
    fn validate(&self, neurons: usize) -> Result<(), NetworkError> {
        if self.seeds < 2 {
            return Err(NetworkError::Config(format!(
                "seeds 至少为 2，实际 {}",
                self.seeds
            )));
        }
        if self.seeds > neurons {
            return Err(NetworkError::Config(format!(
                "seeds {} 超过神经元数 {neurons}",
                self.seeds
            )));
        }
        if !(2..=MAX_GROUPS).contains(&self.groups) {
            return Err(NetworkError::Config(format!(
                "groups 必须在 2..={MAX_GROUPS}，实际 {}",
                self.groups
            )));
        }
        if self.groups > self.seeds {
            return Err(NetworkError::Config(format!(
                "groups {} 不能超过 seeds {}",
                self.groups, self.seeds
            )));
        }
        if self.probe_ticks == 0 || self.exchange_ticks == 0 {
            return Err(NetworkError::Config(
                "probe_ticks 与 exchange_ticks 必须为正".to_string(),
            ));
        }
        if self.rounds == 0 || self.rounds > MAX_ROUNDS {
            return Err(NetworkError::Config(format!(
                "rounds 必须在 1..={MAX_ROUNDS}，实际 {}",
                self.rounds
            )));
        }
        if self.quorum == 0 || self.quorum > self.rounds {
            return Err(NetworkError::Config(format!(
                "quorum 必须在 1..=rounds({})，实际 {}",
                self.rounds, self.quorum
            )));
        }
        if self.drive <= 0 {
            return Err(NetworkError::Config(format!(
                "drive 必须为正，实际 {}",
                self.drive
            )));
        }
        Ok(())
    }
}

/// 搜寻命中的一块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    /// 这一块的神经元（升序）。
    pub neurons: Vec<usize>,
    /// 块外被带动的脉冲数。
    pub score: u64,
}

/// 一轮交流的读数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundVote {
    /// 轮次（0 起）。
    pub round: usize,
    /// 每组的票，下标即组号。
    pub votes: Vec<Ternary>,
    /// 本轮合成判决。
    pub outcome: Ternary,
}

/// 一次完整回路的结论。
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// 最终判决；没找到 / 没收敛时为 `Unknown`。
    pub outcome: Ternary,
    /// 停在的档位。
    pub grade: Grade,
    /// 实际跑了几轮交流。
    pub rounds: usize,
    /// 收敛时连续稳定的轮数。
    pub stable_rounds: usize,
    /// 搜寻命中的块（得分降序）。
    pub assemblies: Vec<Assembly>,
    /// 每轮读数。
    pub votes: Vec<RoundVote>,
    /// 强化后的网络（输入网络的克隆；`reinforce = false` 时与输入相同）。
    pub network: Network,
}

/// 跑一次完整回路。
pub fn run(network: &Network, config: &GradeConfig) -> Result<Verdict, NetworkError> {
    config.validate(network.size())?;
    network.validate()?;
    let mut trained = network.clone();
    let (assemblies, mut state) = search(&trained, config)?;
    if assemblies
        .first()
        .is_none_or(|assembly| assembly.score == 0)
    {
        return Ok(Verdict {
            outcome: Ternary::Unknown,
            grade: Grade::Find,
            rounds: 0,
            stable_rounds: 0,
            assemblies,
            votes: Vec::new(),
            network: trained,
        });
    }
    state = snn::simulate_with_input_from(&trained, &state, config.wait_ticks, |_, _| 0)?.state;
    let (votes, outcome, stable, grade) = exchange(&mut trained, &assemblies, state, config)?;
    Ok(Verdict {
        outcome,
        grade,
        rounds: votes.len(),
        stable_rounds: stable,
        assemblies,
        votes,
        network: trained,
    })
}

/// 自主搜寻：逐块探针，按块外脉冲数打分，返回得分最高的 `groups` 块与冠军末态。
fn search(network: &Network, config: &GradeConfig) -> Result<(Vec<Assembly>, State), NetworkError> {
    let size = network.size();
    let mut scored = Vec::with_capacity(config.seeds);
    let mut winner = State::default();
    let mut best = 0u64;
    for seed in 0..config.seeds {
        let segment = snn::simulate_with_input(network, config.probe_ticks, |_, neuron| {
            if block_of(neuron, size, config.seeds) == seed {
                config.drive
            } else {
                0
            }
        })?;
        let inside: u64 = (0..size)
            .filter(|neuron| block_of(*neuron, size, config.seeds) == seed)
            .map(|neuron| u64::from(segment.fired[neuron]))
            .sum();
        let total: u64 = segment.fired.iter().map(|count| u64::from(*count)).sum();
        let score = total - inside;
        if score > best {
            best = score;
            winner = segment.state.clone();
        }
        scored.push((score, seed));
    }
    // 分数降序；同分按块序号升序，保证确定性。
    scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    let assemblies = scored
        .iter()
        .take(config.groups)
        .map(|(score, seed)| Assembly {
            neurons: (0..size)
                .filter(|neuron| block_of(*neuron, size, config.seeds) == *seed)
                .collect(),
            score: *score,
        })
        .collect();
    Ok((assemblies, winner))
}

/// 交流：轮转发言 → 每组投票 → 连续稳定判收敛。
#[allow(clippy::type_complexity)]
fn exchange(
    network: &mut Network,
    assemblies: &[Assembly],
    mut state: State,
    config: &GradeConfig,
) -> Result<(Vec<RoundVote>, Ternary, usize, Grade), NetworkError> {
    let size = network.size();
    let mut group_of = vec![usize::MAX; size];
    for (group, assembly) in assemblies.iter().enumerate() {
        for &neuron in &assembly.neurons {
            group_of[neuron] = group;
        }
    }
    let mut votes = Vec::with_capacity(config.rounds);
    let mut last = Ternary::Unknown;
    let mut stable = 0usize;
    for round in 0..config.rounds {
        let mut plus = vec![0i64; config.groups];
        let mut minus = vec![0i64; config.groups];
        for turn in 0..config.groups {
            let speaker = (round + turn) % config.groups;
            let segment = snn::simulate_with_input_from(
                network,
                &state,
                config.exchange_ticks,
                |_, neuron| {
                    if group_of[neuron] == speaker {
                        config.drive
                    } else {
                        0
                    }
                },
            )?;
            state = segment.state;
            for neuron in 0..size {
                let value = segment.polarity[neuron];
                if value > 0 {
                    plus[speaker] += i64::from(value);
                } else if value < 0 {
                    minus[speaker] -= i64::from(value);
                }
            }
            if config.reinforce {
                reinforce(
                    network,
                    &group_of,
                    speaker,
                    &segment.fired,
                    &segment.polarity,
                );
            }
        }
        let group_votes: Vec<Ternary> = (0..config.groups)
            .map(|group| vote(plus[group], minus[group]))
            .collect();
        let outcome = majority(&group_votes);
        stable = if outcome == Ternary::Unknown {
            0
        } else if outcome == last {
            stable + 1
        } else {
            1
        };
        last = outcome;
        votes.push(RoundVote {
            round,
            votes: group_votes,
            outcome,
        });
        if stable >= config.quorum {
            return Ok((votes, outcome, stable, Grade::Conclude));
        }
    }
    Ok((votes, Ternary::Unknown, stable, Grade::Exchange))
}

/// 三值 Hebbian 强化：发言组里发过火的神经元 → 本段发过火的组外神经元，
/// 只把空连接 `0` 补成 `±1`（极性同号兴奋、异号抑制）。
fn reinforce(
    network: &mut Network,
    group_of: &[usize],
    speaker: usize,
    fired: &[u32],
    polarity: &[i32],
) {
    let speakers: Vec<usize> = (0..network.size())
        .filter(|neuron| group_of[*neuron] == speaker && fired[*neuron] > 0)
        .collect();
    if speakers.is_empty() {
        return;
    }
    for listener in 0..network.size() {
        if fired[listener] == 0 || group_of[listener] == speaker {
            continue;
        }
        for &source in &speakers {
            if network.weights[listener][source] == 0 {
                let sign = polarity[source].signum() * polarity[listener].signum();
                network.weights[listener][source] = sign as i8;
            }
        }
    }
}

/// 一组一轮的票：脉冲正极性多于负极性投 `+`，反之投 `-`，平票弃权。
fn vote(plus: i64, minus: i64) -> Ternary {
    if plus > minus {
        Ternary::True
    } else if minus > plus {
        Ternary::False
    } else {
        Ternary::Unknown
    }
}

/// 组间合成：`+` 多判 `+`、`-` 多判 `-`、平票/全弃权判未知。
fn majority(votes: &[Ternary]) -> Ternary {
    let plus = votes
        .iter()
        .filter(|value| **value == Ternary::True)
        .count();
    let minus = votes
        .iter()
        .filter(|value| **value == Ternary::False)
        .count();
    if plus > minus {
        Ternary::True
    } else if minus > plus {
        Ternary::False
    } else {
        Ternary::Unknown
    }
}

/// 第 `neuron` 个神经元属于第几块（`seeds` 块尽量等宽）。
fn block_of(neuron: usize, size: usize, seeds: usize) -> usize {
    neuron * seeds / size
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GradeConfig {
        GradeConfig {
            seeds: 2,
            probe_ticks: 4,
            wait_ticks: 2,
            groups: 2,
            exchange_ticks: 3,
            rounds: 8,
            quorum: 2,
            drive: 2,
            reinforce: false,
        }
    }

    fn network(weights: Vec<Vec<i8>>) -> Network {
        Network {
            weights,
            threshold: 1,
            decay: (1, 1),
            refractory: 0,
        }
    }

    #[test]
    fn quiet_network_finds_nothing() {
        let verdict = run(&network(vec![vec![0; 4]; 4]), &config()).expect("合法回路");
        assert_eq!(verdict.grade, Grade::Find);
        assert_eq!(verdict.outcome, Ternary::Unknown);
        assert!(verdict.votes.is_empty());
        assert!(
            verdict
                .assemblies
                .iter()
                .all(|assembly| assembly.score == 0)
        );
    }

    #[test]
    fn search_picks_the_block_that_drives_the_rest() {
        // 0/1 自激并驱动 2..8；其余块没有出边。
        let mut weights = vec![vec![0i8; 8]; 8];
        weights[0][0] = 1;
        weights[0][1] = 1;
        weights[1][0] = 1;
        weights[1][1] = 1;
        for row in weights.iter_mut().skip(2) {
            row[0] = 1;
        }
        let cfg = GradeConfig {
            seeds: 4,
            ..config()
        };
        let verdict = run(&network(weights), &cfg).expect("合法回路");
        assert_eq!(verdict.assemblies[0].neurons, vec![0, 1]);
        assert!(verdict.assemblies[0].score > 0);
    }

    #[test]
    fn two_groups_converge_to_positive() {
        let verdict = run(&network(vec![vec![1; 8]; 8]), &config()).expect("合法回路");
        assert_eq!(verdict.grade, Grade::Conclude);
        assert_eq!(verdict.outcome, Ternary::True);
        assert!(verdict.stable_rounds >= 2);
        assert_eq!(verdict.rounds, verdict.votes.len());
    }

    #[test]
    fn distributed_round_robin_uses_every_group() {
        let cfg = GradeConfig {
            seeds: 4,
            groups: 4,
            ..config()
        };
        let verdict = run(&network(vec![vec![1; 8]; 8]), &cfg).expect("合法回路");
        assert_eq!(verdict.assemblies.len(), 4);
        assert_eq!(verdict.votes[0].votes.len(), 4);
        assert_eq!(verdict.grade, Grade::Conclude);
    }

    #[test]
    fn reinforcement_only_fills_empty_links() {
        // 两块各 2 个神经元；只有 2 收到 0 的边，1 这条边一开始是空的。
        let mut weights = vec![vec![0i8; 4]; 4];
        for row in weights.iter_mut().take(2) {
            row[0] = 1;
            row[1] = 1;
        }
        for row in weights.iter_mut().skip(2) {
            row[2] = 1;
            row[3] = 1;
        }
        weights[2][0] = 1;
        let cfg = GradeConfig {
            reinforce: true,
            ..config()
        };
        let verdict = run(&network(weights.clone()), &cfg).expect("合法回路");
        assert!(
            verdict
                .network
                .weights
                .iter()
                .flatten()
                .all(|weight| (-1..=1).contains(weight))
        );
        assert_eq!(
            verdict.network.weights[2][1], 1,
            "0 发言时 2 跟着发火，1→2 应被补成兴奋边"
        );
        assert_eq!(verdict.network.weights[0][1], weights[0][1], "已有连接不改");
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let net = network(vec![vec![1; 4]; 4]);
        for cfg in [
            GradeConfig {
                seeds: 1,
                ..config()
            },
            GradeConfig {
                seeds: 8,
                ..config()
            },
            GradeConfig {
                groups: 1,
                ..config()
            },
            GradeConfig {
                groups: 3,
                ..config()
            },
            GradeConfig {
                quorum: 0,
                ..config()
            },
            GradeConfig {
                rounds: 0,
                ..config()
            },
            GradeConfig {
                drive: 0,
                ..config()
            },
            GradeConfig {
                probe_ticks: 0,
                ..config()
            },
        ] {
            assert!(
                matches!(run(&net, &cfg), Err(NetworkError::Config(_))),
                "应拒绝 {cfg:?}"
            );
        }
    }
}
