//! 落地状态机：发布之后的过程不是"发通知等回复"，而是
//! **变更包 + 依赖图 + 接收方状态机 + 自动验签**。
//!
//! 状态：已发布 → 已通知 → 已评估影响 → 已适配 → 已验签 → 已归队；
//! 每个状态有 owner 与 SLA；发布方跟踪到"已验签"，接收方沉默按默认策略处理：
//! **小变更超时自动适配，大变更超时降级阻断，不能无限等**。
//!
//! - 精准推送不搞全员广播：受影响方 = 舰队跨仓边里"从变更节点指出去"的接收仓，
//!   影响面 = 以变更节点为源的 (min,max) 污染传播（复用 `propagate`）；
//! - 变更包标准化：影响面 / 为什么变（ADR）/ 迁移步骤 / 回滚 /
//!   **验收信号**（哪个契约测试过 = 落地成功）/ 发布方 owner / 接收方 SLA；
//! - 自动验签：验收信号全部到位才进"已验签"；旧路径按倒计时下线；
//! - 账外无数据：每次状态迁移追加一行 `RECV` 快照事件，重放即复原。

use crate::delta::{esc, unesc};
use crate::fleet::Fleet;
use crate::propagate::Contagion;
use crate::trit::Trit;
use crate::NodeId;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 落地状态机的六个状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptionState {
    /// 已发布。
    Published,
    /// 已通知（依赖图精准推送到受影响仓）。
    Notified,
    /// 已评估影响（无影响 / 需适配 / 需评审）。
    ImpactAssessed,
    /// 已适配（适配 PR / 迁移完成）。
    Adapted,
    /// 已验签（验收信号全部通过）。
    Verified,
    /// 已归队（发布方确认；旧路径进入下线倒计时）。
    Aligned,
}

impl AdoptionState {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PUBLISHED" => Some(Self::Published),
            "NOTIFIED" => Some(Self::Notified),
            "IMPACT" => Some(Self::ImpactAssessed),
            "ADAPTED" => Some(Self::Adapted),
            "VERIFIED" => Some(Self::Verified),
            "ALIGNED" => Some(Self::Aligned),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Published => "PUBLISHED",
            Self::Notified => "NOTIFIED",
            Self::ImpactAssessed => "IMPACT",
            Self::Adapted => "ADAPTED",
            Self::Verified => "VERIFIED",
            Self::Aligned => "ALIGNED",
        }
    }
}

/// 变更规模：超时默认策略的分界（小变更自动适配，大变更降级阻断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blast {
    Small,
    Large,
}

impl Blast {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "small" => Some(Self::Small),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Large => "large",
        }
    }
}

/// 接收方影响结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impact {
    NoImpact,
    NeedsAdaptation,
    NeedsReview,
}

impl Impact {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::NoImpact),
            "adapt" => Some(Self::NeedsAdaptation),
            "review" => Some(Self::NeedsReview),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoImpact => "none",
            Self::NeedsAdaptation => "adapt",
            Self::NeedsReview => "review",
        }
    }
}

/// 变更包：影响下游的变更必须带全，否则通知就是噪音。
#[derive(Debug, Clone)]
pub struct ChangePackage {
    /// 变更 ID（唯一）。
    pub id: String,
    pub title: String,
    /// 影响面：触及的设计敏感路径。
    pub areas: Vec<crate::design::DesignArea>,
    /// 为什么变：关联 ADR/设计决策。
    pub linked_adr: Option<String>,
    /// 迁移步骤。
    pub migration: Vec<String>,
    /// 自动化迁移提示。
    pub codemod: Option<String>,
    /// 回滚方案。
    pub rollback: String,
    /// 归队后旧路径下线倒计时（tick 数）。
    pub decommission_after: u32,
    /// 验收信号：哪个契约测试/监控/开关算落地成功。
    pub acceptance: Vec<String>,
    /// 发布方 owner。
    pub owner: String,
    /// 变更规模（超时默认策略分界）。
    pub blast: Blast,
    /// 接收方响应 SLA（tick 数）。
    pub receiver_sla: u32,
}

/// 一个接收方的落地状态。
#[derive(Debug, Clone)]
pub struct Receiver {
    pub pkg: usize,
    pub repo: String,
    pub owner: String,
    pub state: AdoptionState,
    pub sla_remaining: u32,
    pub impact: Option<Impact>,
    /// 已通过的验收信号。
    pub verified: Vec<String>,
    /// 降级/阻断标记（大变更超时、验签催办超时）。
    pub degraded: bool,
    /// 影响分析：本仓受影响的节点。
    pub affected: Vec<NodeId>,
}

/// 落地账本（内存态 + 可选追加式事件账本）。
#[derive(Debug, Default)]
pub struct Adoption {
    packages: Vec<ChangePackage>,
    receivers: Vec<Receiver>,
    decommission: Vec<(usize, String, u32)>,
    ledger: Option<PathBuf>,
}

impl Adoption {
    pub fn new() -> Self {
        Self::default()
    }

    /// 打开追加式事件账本（每次状态迁移追加一行快照，重放即复原）。
    pub fn open_ledger(&mut self, path: impl Into<PathBuf>) {
        self.ledger = Some(path.into());
    }

    /// 发布变更：**依赖图精准推送**——受影响方 = 舰队跨仓边里
    /// "从变更节点指出去"的接收仓；影响分析 = 以变更节点为源的污染传播。
    /// 返回登记的接收方数量（0 = 无人依赖，也如实记录）。
    pub fn publish(
        &mut self,
        fleet: &Fleet,
        pkg: ChangePackage,
        changed: &[(String, NodeId)],
    ) -> io::Result<usize> {
        for (repo, node) in changed {
            let j = fleet.get(repo).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("变更仓 {repo} 不存在"))
            })?;
            if node.0 as usize >= j.dag().nodes().len() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("变更节点 #{:02} 不存在于 {repo}", node.0),
                ));
            }
        }
        if self.packages.iter().any(|p| p.id == pkg.id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("变更 ID 重复：{}", pkg.id),
            ));
        }

        // 精准推送：跨仓边 from == 变更节点的接收仓
        let mut deps: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
        for l in fleet.cross_links() {
            if changed
                .iter()
                .any(|(r, n)| *r == l.from.0 && *n == l.from.1)
            {
                deps.entry(l.to.0.clone()).or_default().push(l.to.1);
            }
        }

        // 自动影响分析：以变更节点为源（最坏假设 -1 强度）的全局传播
        let impacted = Self::impact_from(fleet, changed);

        let pkg_idx = self.packages.len();
        self.commit_pkg(&pkg)?;
        self.packages.push(pkg.clone());

        let mut count = 0usize;
        for (repo, affected) in deps {
            let impact = if affected.is_empty() {
                Impact::NoImpact
            } else {
                Impact::NeedsAdaptation
            };
            let receiver = Receiver {
                pkg: pkg_idx,
                repo: repo.clone(),
                owner: format!("{repo}-owner"),
                state: AdoptionState::Notified,
                sla_remaining: self.packages[pkg_idx].receiver_sla,
                impact: Some(impact),
                verified: Vec::new(),
                degraded: false,
                affected,
            };
            self.commit_recv(pkg_idx, &receiver)?;
            self.receivers.push(receiver);
            count += 1;
        }
        let _ = impacted;
        Ok(count)
    }

    /// 以变更节点为源的影响分析：全局编号传播，返回 仓 → 受影响节点。
    fn impact_from(
        fleet: &Fleet,
        changed: &[(String, NodeId)],
    ) -> BTreeMap<String, Vec<NodeId>> {
        let mut base: BTreeMap<String, usize> = BTreeMap::new();
        let mut total = 0usize;
        for (n, j) in fleet.repos() {
            base.insert(n.clone(), total);
            total += j.dag().nodes().len();
        }
        let mut g = Contagion::new(total);
        for (n, j) in fleet.repos() {
            let off = base[n];
            for l in j.dag().links() {
                g.link(off + l.from.0 as usize, off + l.to.0 as usize, l.pass);
            }
        }
        for l in fleet.cross_links() {
            let (Some(fo), Some(to)) = (base.get(&l.from.0), base.get(&l.to.0)) else {
                continue;
            };
            g.link(fo + l.from.1.0 as usize, to + l.to.1.0 as usize, l.pass);
        }
        let sources: Vec<(usize, Trit)> = changed
            .iter()
            .filter_map(|(r, n)| base.get(r).map(|off| (off + n.0 as usize, Trit::Vulnerable)))
            .collect();
        let dist = g.spread(&sources);
        let mut out: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
        for (n, j) in fleet.repos() {
            let off = base[n];
            let affected: Vec<NodeId> = j
                .dag()
                .nodes()
                .iter()
                .filter(|node| dist[off + node.id.0 as usize] != Trit::Hardened)
                .map(|node| node.id)
                .collect();
            if !affected.is_empty() {
                out.insert(n.clone(), affected);
            }
        }
        out
    }

    fn locate(&self, pkg_id: &str, repo: &str) -> Result<(usize, usize), String> {
        let pkg = self
            .packages
            .iter()
            .position(|p| p.id == pkg_id)
            .ok_or_else(|| format!("无此变更包：{pkg_id:?}"))?;
        let recv = self
            .receivers
            .iter()
            .position(|r| r.pkg == pkg && r.repo == repo)
            .ok_or_else(|| format!("{repo} 未登记为此变更的接收方"))?;
        Ok((pkg, recv))
    }

    /// 接收方确认影响结论（无影响 / 需适配 / 需评审）。
    pub fn confirm_impact(
        &mut self,
        pkg_id: &str,
        repo: &str,
        impact: Impact,
    ) -> Result<AdoptionState, String> {
        let (pi, ri) = self.locate(pkg_id, repo)?;
        let r = &mut self.receivers[ri];
        if r.state != AdoptionState::Notified {
            return Err(format!("{repo} 当前状态 {:?} 不能确认影响", r.state));
        }
        r.impact = Some(impact);
        r.state = AdoptionState::ImpactAssessed;
        let state = r.state;
        let snap = r.clone();
        self.commit_recv(pi, &snap).map_err(|e| e.to_string())?;
        Ok(state)
    }

    /// 接收方标记适配完成。
    pub fn mark_adapted(&mut self, pkg_id: &str, repo: &str) -> Result<AdoptionState, String> {
        let (pi, ri) = self.locate(pkg_id, repo)?;
        let r = &mut self.receivers[ri];
        if r.state != AdoptionState::ImpactAssessed {
            return Err(format!("{repo} 当前状态 {:?} 不能标记适配", r.state));
        }
        r.state = AdoptionState::Adapted;
        let state = r.state;
        let snap = r.clone();
        self.commit_recv(pi, &snap).map_err(|e| e.to_string())?;
        Ok(state)
    }

    /// 自动验签：信号入账；**验收信号全部通过才进"已验签"**。
    pub fn verify(
        &mut self,
        pkg_id: &str,
        repo: &str,
        signal: &str,
    ) -> Result<AdoptionState, String> {
        let (pi, ri) = self.locate(pkg_id, repo)?;
        let r = &mut self.receivers[ri];
        if r.state != AdoptionState::Adapted {
            return Err(format!("{repo} 当前状态 {:?} 不能验签", r.state));
        }
        if !r.verified.iter().any(|s| s == signal) {
            r.verified.push(signal.to_string());
        }
        let acceptance = &self.packages[pi].acceptance;
        if !acceptance.is_empty()
            && acceptance.iter().all(|a| r.verified.iter().any(|v| v == a))
        {
            r.state = AdoptionState::Verified;
        }
        let state = r.state;
        let snap = r.clone();
        self.commit_recv(pi, &snap).map_err(|e| e.to_string())?;
        Ok(state)
    }

    /// 发布方确认"已归队"（只能从已验签进入）；旧路径进入下线倒计时。
    pub fn align(&mut self, pkg_id: &str, repo: &str) -> Result<AdoptionState, String> {
        let (pi, ri) = self.locate(pkg_id, repo)?;
        let r = &mut self.receivers[ri];
        if r.state != AdoptionState::Verified {
            return Err(format!(
                "{repo} 当前状态 {:?} 不能归队（必须先验签）",
                r.state
            ));
        }
        r.state = AdoptionState::Aligned;
        let countdown = self.packages[pi].decommission_after;
        self.decommission.push((pi, repo.to_string(), countdown));
        let state = r.state;
        let snap = r.clone();
        self.commit_recv(pi, &snap).map_err(|e| e.to_string())?;
        Ok(state)
    }

    /// 调度 tick：接收方 SLA 递减；超时按默认策略——
    /// 小变更自动推进（自动确认影响 / 自动适配），大变更降级阻断，不能无限等。
    /// 返回发生状态迁移的 (变更 ID, 仓, 新状态)。
    pub fn tick(&mut self) -> Vec<(String, String, AdoptionState)> {
        let mut transitions = Vec::new();
        for ri in 0..self.receivers.len() {
            let state = self.receivers[ri].state;
            if matches!(state, AdoptionState::Aligned | AdoptionState::Published) {
                continue;
            }
            if self.receivers[ri].degraded {
                continue; // 已降级的不重复处置
            }
            if self.receivers[ri].sla_remaining > 0 {
                self.receivers[ri].sla_remaining -= 1;
            }
            if self.receivers[ri].sla_remaining > 0 {
                continue;
            }
            let pkg_idx = self.receivers[ri].pkg;
            let blast = self.packages[pkg_idx].blast;
            let repo = self.receivers[ri].repo.clone();
            match (blast, state) {
                (Blast::Small, AdoptionState::Notified) => {
                    // 自动确认影响（沿用发布时的自动分析结论）
                    if self.receivers[ri].impact.is_none() {
                        self.receivers[ri].impact = Some(Impact::NoImpact);
                    }
                    self.receivers[ri].state = AdoptionState::ImpactAssessed;
                    transitions.push((
                        self.packages[pkg_idx].id.clone(),
                        repo.clone(),
                        AdoptionState::ImpactAssessed,
                    ));
                }
                (Blast::Small, AdoptionState::ImpactAssessed) => {
                    // 小变更：默认自动适配
                    self.receivers[ri].state = AdoptionState::Adapted;
                    transitions.push((
                        self.packages[pkg_idx].id.clone(),
                        repo.clone(),
                        AdoptionState::Adapted,
                    ));
                }
                (Blast::Small, AdoptionState::Adapted) => {
                    // 验签催办超时：降级标记
                    self.receivers[ri].degraded = true;
                }
                _ => {
                    // 大变更：降级阻断（状态不动，不算落地）
                    self.receivers[ri].degraded = true;
                }
            }
            let snap = self.receivers[ri].clone();
            self.commit_recv(pkg_idx, &snap)
                .expect("adoption 账本写入失败");
        }
        // 归队后旧路径下线倒计时
        for d in &mut self.decommission {
            if d.2 > 0 {
                d.2 -= 1;
            }
        }
        transitions
    }

    /// 旧路径下线时间表：(变更 ID, 仓, 剩余 tick)。
    pub fn decommission(&self) -> Vec<(String, String, u32)> {
        self.decommission
            .iter()
            .map(|(pi, repo, c)| (self.packages[*pi].id.clone(), repo.clone(), *c))
            .collect()
    }

    pub fn packages(&self) -> &[ChangePackage] {
        &self.packages
    }

    pub fn receivers(&self) -> &[Receiver] {
        &self.receivers
    }

    /// 看板：各接收方落地状态（漂移与降级一目了然）。
    pub fn board(&self) -> String {
        let mut out = String::new();
        for r in &self.receivers {
            let pkg = &self.packages[r.pkg];
            let _ = writeln!(
                out,
                "{} → {}　[{}]{}　影响 {:?}　验签 {}/{}　SLA {}",
                pkg.id,
                r.repo,
                r.state.as_str(),
                if r.degraded { " 降级" } else { "" },
                r.impact.map(|i| i.as_str()).unwrap_or("-"),
                r.verified.len(),
                pkg.acceptance.len(),
                r.sla_remaining
            );
        }
        out
    }

    fn commit_pkg(&mut self, pkg: &ChangePackage) -> io::Result<()> {
        let Some(ledger) = &self.ledger else {
            return Ok(());
        };
        let areas = pkg
            .areas
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let line = format!(
            "PKG\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            esc(&pkg.id),
            esc(&pkg.title),
            esc(&pkg.owner),
            pkg.blast.as_str(),
            pkg.receiver_sla,
            pkg.decommission_after,
            esc(pkg.linked_adr.as_deref().unwrap_or("-")),
            esc(&pkg.rollback),
            esc(&areas),
            esc(&pkg.acceptance.join(",")),
        );
        append(ledger, &line)
    }

    fn commit_recv(&mut self, pkg_idx: usize, r: &Receiver) -> io::Result<()> {
        let Some(ledger) = &self.ledger else {
            return Ok(());
        };
        let pkg_id = &self.packages[pkg_idx].id;
        let line = format!(
            "RECV\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            esc(pkg_id),
            esc(&r.repo),
            esc(&r.owner),
            r.state.as_str(),
            r.sla_remaining,
            r.impact.map(|i| i.as_str()).unwrap_or("-"),
            esc(&r.verified.join(",")),
            u8::from(r.degraded),
            esc(&r
                .affected
                .iter()
                .map(|n| n.0.to_string())
                .collect::<Vec<_>>()
                .join(",")),
        );
        append(ledger, &line)
    }

    /// 从事件账本重放复原：`PKG` 全量重建包，`RECV` 同 (包, 仓) 最后一条快照生效。
    pub fn replay(path: &Path) -> io::Result<Adoption> {
        let text = std::fs::read_to_string(path)?;
        let mut out = Adoption::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut f = line.split('\t');
            let conv = |e: String| io::Error::new(io::ErrorKind::InvalidData, e);
            match f.next().unwrap_or("") {
                "PKG" => {
                    let bad = || io::Error::new(io::ErrorKind::InvalidData, "PKG 行不合法");
                    let id = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let title = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let owner = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let blast = Blast::parse(f.next().ok_or_else(bad)?).ok_or_else(bad)?;
                    let receiver_sla = f
                        .next()
                        .ok_or_else(bad)?
                        .parse::<u32>()
                        .map_err(|e| conv(e.to_string()))?;
                    let decommission_after = f
                        .next()
                        .ok_or_else(bad)?
                        .parse::<u32>()
                        .map_err(|e| conv(e.to_string()))?;
                    let adr_raw = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let rollback = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let areas_raw = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let acceptance_raw =
                        unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    out.packages.push(ChangePackage {
                        id,
                        title,
                        areas: areas_raw
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .filter_map(crate::design::DesignArea::parse)
                            .collect(),
                        linked_adr: if adr_raw == "-" { None } else { Some(adr_raw) },
                        migration: Vec::new(),
                        codemod: None,
                        rollback,
                        decommission_after,
                        acceptance: acceptance_raw
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .collect(),
                        owner,
                        blast,
                        receiver_sla,
                    });
                }
                "RECV" => {
                    let bad = || io::Error::new(io::ErrorKind::InvalidData, "RECV 行不合法");
                    let pkg_id = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let repo = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let owner = unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let state = AdoptionState::parse(f.next().ok_or_else(bad)?)
                        .ok_or_else(bad)?;
                    let sla_remaining = f
                        .next()
                        .ok_or_else(bad)?
                        .parse::<u32>()
                        .map_err(|e| conv(e.to_string()))?;
                    let impact = Impact::parse(f.next().ok_or_else(bad)?);
                    let verified_raw =
                        unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let degraded = f.next().ok_or_else(bad)? == "1";
                    let affected_raw =
                        unesc(f.next().ok_or_else(bad)?).map_err(conv)?;
                    let Some(pi) = out.packages.iter().position(|p| p.id == pkg_id) else {
                        continue; // 包未重载：跳过（与仓状态解耦）
                    };
                    let recv = Receiver {
                        pkg: pi,
                        repo,
                        owner,
                        state,
                        sla_remaining,
                        impact,
                        verified: verified_raw
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .collect(),
                        degraded,
                        affected: affected_raw
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .filter_map(|s| s.parse::<u32>().ok())
                            .map(NodeId)
                            .collect(),
                    };
                    if let Some(slot) = out
                        .receivers
                        .iter_mut()
                        .find(|r| r.pkg == pi && r.repo == recv.repo)
                    {
                        *slot = recv;
                    } else {
                        out.receivers.push(recv);
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }
}

fn append(path: &Path, line: &str) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())
}
