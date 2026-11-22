//! 双轨证据累积（P2 的代数核心）。
//!
//! P2 的要点不是"多一个四值枚举"，而是**同一命题可以同时有支持和反对两轨证据，且两轨都不能被消掉**：
//!
//! - 禁止多数投票：3 份支持 + 1 份反对仍然是冲突，不是"通过"。
//! - 禁止最后写入胜：后到的报告只累加到自己的轨，不覆盖另一轨。
//! - 冲突单调：一旦两轨都非空，再合并任何证据都不会让它退回 `True`/`False`。消掉冲突的唯一途径
//!   是 [`RailsLedger::rebase`]——换 epoch 重取证，那时旧证据整体作废（A5）。
//!
//! 这一层刻意**不**依赖 `Observation` 的结构：现有 `Observation.evidence` 只能装一份
//! [`CheckReport`]，表达不了"两个来源一正一反"。`RailsLedger` 是宿主侧的累积器，
//! 把多份报告按命题累积成双轨；是否把它塞进 `Observation` 是另一次结构决定（见 README 的边界）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CheckReport, EvidenceVerdict, Ternary};

/// 支持轨与反对轨的计数。
///
/// 计数而不是布尔：`2/1` 与 `1/0` 都是"未定"吗？不是——前者是冲突（要裁决），后者是没证据（要补证）。
/// 保留计数才能让出口分开，否则 `Conflicted` 和 `Neither` 会被压成同一个 `Unknown`，裁决与补证就分不开了。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rails {
    /// 支持该命题的证据份数。
    pub support: u32,
    /// 反对该命题的证据份数。
    pub oppose: u32,
}

impl Rails {
    pub const fn empty() -> Self {
        Self {
            support: 0,
            oppose: 0,
        }
    }

    pub const fn new(support: u32, oppose: u32) -> Self {
        Self { support, oppose }
    }

    /// 单条证据上轨。`Neither` 不动——"没有证据"不投票，否则它会被当成一张反对票。
    pub fn observe(&mut self, verdict: EvidenceVerdict) {
        match verdict {
            EvidenceVerdict::Proved => self.support = self.support.saturating_add(1),
            EvidenceVerdict::Refuted => self.oppose = self.oppose.saturating_add(1),
            // 自称冲突的报告本身就证明了"两边都有材料"：两轨各记一次。
            EvidenceVerdict::Conflicted => {
                self.support = self.support.saturating_add(1);
                self.oppose = self.oppose.saturating_add(1);
            }
            EvidenceVerdict::Neither => {}
        }
    }

    /// 上一条报告。`epoch` 不符或 `checks == 0` 的报告按"没证据"处理，不动任何轨（A5：过期证据不算数）。
    pub fn observe_report(&mut self, report: &CheckReport, epoch: u64) {
        self.observe(report.judgment(epoch));
    }

    /// 四值状态。这是 P2 要求"B 单独存在"的落点：`Conflicted` 与 `Neither` 必须可区分。
    pub const fn state(&self) -> EvidenceVerdict {
        match (self.support > 0, self.oppose > 0) {
            (true, true) => EvidenceVerdict::Conflicted,
            (true, false) => EvidenceVerdict::Proved,
            (false, true) => EvidenceVerdict::Refuted,
            (false, false) => EvidenceVerdict::Neither,
        }
    }

    /// 三值口径：冲突与没证据都返回 `Unknown`，**但调用方要能靠 [`Self::state`] 区分**。
    pub const fn judge(&self) -> Ternary {
        self.state().value()
    }

    pub const fn is_conflicted(&self) -> bool {
        self.support > 0 && self.oppose > 0
    }

    pub const fn has_evidence(&self) -> bool {
        self.support > 0 || self.oppose > 0
    }

    /// 合并两份累积：两轨各自饱和累加。
    ///
    /// 这是"不做多数投票、不做最后写入胜"的机器表达——任何一轨的计数只会增加，所以
    /// 一旦冲突，合并后仍冲突。
    pub const fn merge(self, other: Rails) -> Rails {
        Rails {
            support: self.support.saturating_add(other.support),
            oppose: self.oppose.saturating_add(other.oppose),
        }
    }
}

/// 按命题累积双轨证据的台账。
///
/// 键是命题（任务 / 判定）ID。同一命题的多份报告都在这里累积，而不是互相覆盖。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailsLedger {
    rails: BTreeMap<String, Rails>,
}

impl RailsLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.rails.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rails.len()
    }

    /// 记一份报告。过期或空的报告不产生任何轨。
    pub fn observe(&mut self, subject: &str, report: &CheckReport, epoch: u64) {
        let rails = self.rails.entry(subject.to_owned()).or_default();
        rails.observe_report(report, epoch);
    }

    /// 直接记一条四值判定（宿主自己已经折算过时用）。
    pub fn observe_verdict(&mut self, subject: &str, verdict: EvidenceVerdict) {
        self.rails
            .entry(subject.to_owned())
            .or_default()
            .observe(verdict);
    }

    pub fn rails(&self, subject: &str) -> Rails {
        self.rails.get(subject).copied().unwrap_or(Rails::empty())
    }

    pub fn judgment(&self, subject: &str) -> EvidenceVerdict {
        self.rails(subject).state()
    }

    pub fn judge(&self, subject: &str) -> Ternary {
        self.rails(subject).judge()
    }

    /// 是否冲突。冲突要走 `Adjudicate`，没证据要走 `Measure`——出口不同，不能合并成一个"未定"。
    pub fn is_conflicted(&self, subject: &str) -> bool {
        self.rails(subject).is_conflicted()
    }

    /// 所有冲突中的命题，按 ID 升序。
    pub fn conflicted(&self) -> Vec<&str> {
        self.rails
            .iter()
            .filter(|(_, rails)| rails.is_conflicted())
            .map(|(subject, _)| subject.as_str())
            .collect()
    }

    /// 合并另一份台账（跨主机 / 跨 patch segment / 断点续跑）。
    ///
    /// 合并是保守的：冲突只会增多，不会因为某台机器上证据更全就消失。
    pub fn merge(&mut self, other: RailsLedger) {
        for (subject, rails) in other.rails {
            let slot = self.rails.entry(subject).or_default();
            *slot = slot.merge(rails);
        }
    }

    /// 换基：某个命题的 epoch 变了，旧证据整体作废。
    ///
    /// 这是**唯一**允许消掉冲突的操作，因为它的语义是"旧的都过期了"（A5），不是"我挑一个我喜欢的"。
    pub fn rebase(&mut self, subject: &str) {
        self.rails.remove(subject);
    }

    /// 换基全部：计划打补丁后整批重取证。
    pub fn rebase_all(&mut self) {
        self.rails.clear();
    }

    /// 需要补证的命题（有争议之外的"没证据"也算需要补证，但这里只列"完全没有证据"的）。
    pub fn unevidenced(&self) -> Vec<&str> {
        self.rails
            .iter()
            .filter(|(_, rails)| !rails.has_evidence())
            .map(|(subject, _)| subject.as_str())
            .collect()
    }
}
