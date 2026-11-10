//! 设计决策生产线：**不审代码，审决策；不逐 PR 决策，把决策变成规则。**
//!
//! 三分类（先区分真设计问题与伪设计问题）：
//! ①重复性设计争论 → 不是设计问题，是缺原则/契约/架构测试——解决一次，
//!   固化为规则（**案例法 → 成文法**），下次自动判；
//! ②执行既有决策 → 设计早就定了，走快速通道，只检查是否偏离契约；
//! ③真正的新决策 → 唯一值得人类设计评审的一类，按 **可逆性 × 影响范围** 分级。
//!
//! 关键机制：**决策所有者 + SLA + 限时默认通过 + 异议升级**——否则设计评审
//! 变成新瓶颈。AI 的定位：分类、去重、补上下文；**所有权与最终拍板永远是人**。
//!
//! 效果：设计评审的量随规则积累**下降**，而不是随 PR 数量线性爆炸。

use crate::dag::{ImmuneDag, NodeId};
use std::fmt;

/// 设计敏感路径（触碰即视为可能的新决策）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignArea {
    /// 公共接口。
    PublicApi,
    /// 数据模型。
    DataModel,
    /// 安全边界。
    SecurityBoundary,
    /// 跨团队契约。
    CrossTeamContract,
    /// 不可逆迁移。
    IrreversibleMigration,
}

impl DesignArea {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "api" | "public-api" => Some(Self::PublicApi),
            "model" | "data-model" => Some(Self::DataModel),
            "security" | "security-boundary" => Some(Self::SecurityBoundary),
            "contract" | "cross-team" => Some(Self::CrossTeamContract),
            "migration" | "irreversible" => Some(Self::IrreversibleMigration),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PublicApi => "api",
            Self::DataModel => "model",
            Self::SecurityBoundary => "security",
            Self::CrossTeamContract => "contract",
            Self::IrreversibleMigration => "migration",
        }
    }
}

impl fmt::Display for DesignArea {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// PR 模板强制字段（未填齐且触及敏感路径 → 不进代码审查，退回设计队列）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrIntake {
    pub title: String,
    /// 触及的设计敏感路径（空 = 非设计敏感，走快速通道）。
    pub touches: Vec<DesignArea>,
    /// 模板栏：这是"新决策"还是"执行既有决策"？
    pub is_new_decision: bool,
    /// 关联的 ADR/RFC/契约编号。
    pub linked_adr: Option<String>,
    /// 影响面摘要。
    pub impact_summary: Option<String>,
    /// 回滚方案。
    pub rollback: Option<String>,
}

/// 评审分级：**可逆性 × 影响范围**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// 局部可逆：默认原则 + 事后记录，不前置评审（错了快速回滚）。
    LocalReversible,
    /// 局部不可逆 / 跨模块：轻量设计说明，一个 owner 看，限时默认通过。
    LightReview,
    /// 跨团队 / 安全边界 / 不可逆迁移：正式 RFC，利益相关者评审。
    FormalRfc,
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalReversible => f.write_str("LocalReversible"),
            Self::LightReview => f.write_str("LightReview"),
            Self::FormalRfc => f.write_str("FormalRfc"),
        }
    }
}

/// 按 可逆性 × 影响范围 分级。
pub fn grade(touches: &[DesignArea]) -> Tier {
    if touches.iter().any(|a| {
        matches!(
            a,
            DesignArea::IrreversibleMigration
                | DesignArea::SecurityBoundary
                | DesignArea::CrossTeamContract
        )
    }) {
        Tier::FormalRfc
    } else if touches
        .iter()
        .any(|a| matches!(a, DesignArea::PublicApi | DesignArea::DataModel))
    {
        Tier::LightReview
    } else {
        Tier::LocalReversible
    }
}

/// 分类结果：进哪条道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routing {
    /// 快速通道：非设计敏感，普通代码审查。
    CodeReview,
    /// 执行既有决策：快速通道 + 契约检查（不进设计评审）。
    ExecuteAdr { adr: String },
    /// 退回设计队列（按 可逆性×影响范围 分级）。
    DesignQueue { tier: Tier },
}

/// 分类：先区分真设计问题与伪设计问题，再定路由。
pub fn classify(intake: &PrIntake) -> Routing {
    if intake.touches.is_empty() {
        return Routing::CodeReview;
    }
    match (intake.is_new_decision, &intake.linked_adr) {
        // 执行既有决策：PR 只承载已决策的实现
        (false, Some(adr)) => Routing::ExecuteAdr { adr: adr.clone() },
        // 触及设计边界却声称非新决策又无关联 → 退回设计队列（补决策，不是补代码）
        (false, None) => Routing::DesignQueue {
            tier: grade(&intake.touches),
        },
        // 新决策：进设计队列，按分级走对应评审
        (true, _) => Routing::DesignQueue {
            tier: grade(&intake.touches),
        },
    }
}

/// 队列条目状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueStatus {
    Pending,
    /// owner 拍板。
    Approved,
    /// SLA 到 0：限时默认通过。
    DefaultApproved,
    /// SLA 到 0 且该档不默认通过 → 升级（异议升级通道）。
    Escalated,
    Rejected,
}

/// 队列条目。
#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub intake: PrIntake,
    pub tier: Tier,
    /// 决策所有者（登记时指定；拍板权不可代理）。
    pub owner: String,
    /// SLA 剩余 tick。
    pub sla_remaining: u32,
    pub status: QueueStatus,
}

/// 档位配置：owner 组 + SLA + 限时默认通过。
#[derive(Debug, Clone)]
pub struct TierPolicy {
    pub tier: Tier,
    /// 有权拍板该档位的所有者组。
    pub owners: Vec<String>,
    /// SLA 剩余 0 时：true = 默认通过；false = 升级（Escalated，走异议升级）。
    pub default_approve: bool,
    pub sla: u32,
}

/// 设计队列：决策等待区（有限——与 GOAL.md 的有界劳动同构）。
#[derive(Debug, Clone, Default)]
pub struct DesignQueue {
    entries: Vec<QueueEntry>,
    policies: Vec<TierPolicy>,
}

impl DesignQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 配置某档位（owner 组 + SLA + 默认通过）。
    pub fn set_policy(&mut self, policy: TierPolicy) {
        self.policies.retain(|p| p.tier != policy.tier);
        self.policies.push(policy);
    }

    /// PR 入口：分类；设计敏感的进队列并登记 owner 与 SLA。
    pub fn route(&mut self, intake: PrIntake, owner: &str) -> Result<Routing, String> {
        let routing = classify(&intake);
        if let Routing::DesignQueue { tier } = &routing {
            let policy = self
                .policies
                .iter()
                .find(|p| p.tier == *tier)
                .ok_or_else(|| format!("档位 {tier} 未配置 owner/SLA"))?;
            self.entries.push(QueueEntry {
                intake,
                tier: *tier,
                owner: owner.to_string(),
                sla_remaining: policy.sla,
                status: QueueStatus::Pending,
            });
        }
        Ok(routing)
    }

    /// owner 拍板。**拍板权不可代理**：非 owner 组成员拒绝。
    pub fn approve(&mut self, idx: usize, by: &str) -> Result<QueueStatus, String> {
        let entry = self
            .entries
            .get_mut(idx)
            .ok_or_else(|| format!("队列条目 #{idx} 不存在"))?;
        if entry.status != QueueStatus::Pending {
            return Err(format!("条目 #{idx} 已处置（{:?}）", entry.status));
        }
        let allowed = self
            .policies
            .iter()
            .find(|p| p.tier == entry.tier)
            .map(|p| p.owners.iter().any(|o| o == by))
            .unwrap_or(false);
        if !allowed {
            return Err(format!("{by} 无权拍板 {:?} 档位", entry.tier));
        }
        entry.status = QueueStatus::Approved;
        Ok(entry.status)
    }

    /// 调度 tick：SLA 递减；到 0 → 限时默认通过或升级（返回状态变化的条目）。
    pub fn tick(&mut self) -> Vec<(usize, QueueStatus)> {
        let mut changed = Vec::new();
        for (idx, entry) in self.entries.iter_mut().enumerate() {
            if entry.status != QueueStatus::Pending {
                continue;
            }
            if entry.sla_remaining > 0 {
                entry.sla_remaining -= 1;
            }
            if entry.sla_remaining == 0 {
                let default_approve = self
                    .policies
                    .iter()
                    .find(|p| p.tier == entry.tier)
                    .map(|p| p.default_approve)
                    .unwrap_or(false);
                entry.status = if default_approve {
                    QueueStatus::DefaultApproved
                } else {
                    QueueStatus::Escalated
                };
                changed.push((idx, entry.status));
            }
        }
        changed
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    /// 待决条目下标。
    pub fn pending(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.status == QueueStatus::Pending)
            .map(|(i, _)| i)
            .collect()
    }
}

/// 固化的规则：**每解决一次重复争论，产出一条**（案例法 → 成文法）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statute {
    /// 架构原则（文字，供评审引用）。
    Principle(String),
    /// 接口/数据契约（名字 + 约定）。
    Contract { name: String, terms: String },
    /// 架构测试（机器可查，进 CI）。
    ArchTest(ArchTest),
    /// 模式库正例反例。
    Pattern { good: String, bad: String },
}

/// 机器可查的架构测试。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchTest {
    /// 耦合阈值：扇入超阈 = 违规（复用 `dag::design_smells`）。
    MaxFanIn(usize),
    /// 设计敏感变更必须带 ADR——在 `classify` 的路由层强制（此处仅为记录）。
    RequireAdrForSensitive,
}

/// 成文法注册表：原则 / 契约 / 架构测试 / 模式正反例。
#[derive(Debug, Clone, Default)]
pub struct Statutes {
    items: Vec<Statute>,
}

/// 架构测试违规。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub statute: String,
    pub node: NodeId,
    pub detail: String,
}

impl Statutes {
    pub fn codify(&mut self, statute: Statute) -> usize {
        self.items.push(statute);
        self.items.len() - 1
    }

    pub fn items(&self) -> &[Statute] {
        &self.items
    }

    /// 架构测试：对 DAG 跑全部机器可查规则，返回违规清单。
    /// 审计口径：**"未检查"不当"通过"**——没有机器规则就返回空违规清单，
    /// 但文字原则的遵守情况仍归人类评审（如实分账）。
    pub fn arch_test(&self, dag: &ImmuneDag) -> Vec<Violation> {
        let mut out = Vec::new();
        for s in &self.items {
            match s {
                Statute::ArchTest(ArchTest::MaxFanIn(n)) => {
                    for id in dag.design_smells(*n) {
                        let actual = dag.coupling(id).fan_in;
                        out.push(Violation {
                            statute: format!("MaxFanIn({n})"),
                            node: id,
                            detail: format!("扇入 {actual} 超阈 {n}"),
                        });
                    }
                }
                Statute::ArchTest(ArchTest::RequireAdrForSensitive) => {
                    // 在 classify 路由层强制（设计敏感无 ADR → 退回设计队列）
                }
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intake(touches: &[DesignArea], is_new: bool, adr: Option<&str>) -> PrIntake {
        PrIntake {
            title: "t".into(),
            touches: touches.to_vec(),
            is_new_decision: is_new,
            linked_adr: adr.map(|s| s.to_string()),
            impact_summary: None,
            rollback: None,
        }
    }

    #[test]
    fn classify_routes_three_kinds() {
        // ① 执行既有决策：快速通道 + 契约检查
        assert_eq!(
            classify(&intake(&[DesignArea::PublicApi], false, Some("ADR-7"))),
            Routing::ExecuteAdr {
                adr: "ADR-7".into()
            }
        );
        // ② 触及敏感边界却无关联 → 退回设计队列（且按影响范围给 FormalRfc）
        assert_eq!(
            classify(&intake(&[DesignArea::SecurityBoundary], false, None)),
            Routing::DesignQueue {
                tier: Tier::FormalRfc
            }
        );
        // ③ 真新决策 → 设计队列
        assert_eq!(
            classify(&intake(&[DesignArea::PublicApi], true, None)),
            Routing::DesignQueue {
                tier: Tier::LightReview
            }
        );
        // 非设计敏感 → 普通代码审查
        assert_eq!(classify(&intake(&[], false, None)), Routing::CodeReview);
    }

    #[test]
    fn grade_by_reversibility_times_blast_radius() {
        assert_eq!(grade(&[]), Tier::LocalReversible);
        assert_eq!(grade(&[DesignArea::PublicApi]), Tier::LightReview);
        assert_eq!(grade(&[DesignArea::DataModel]), Tier::LightReview);
        assert_eq!(grade(&[DesignArea::IrreversibleMigration]), Tier::FormalRfc);
        // 混合触及取最高档
        assert_eq!(
            grade(&[DesignArea::PublicApi, DesignArea::CrossTeamContract]),
            Tier::FormalRfc
        );
    }

    #[test]
    fn sla_default_approve_and_escalation() {
        let mut q = DesignQueue::new();
        q.set_policy(TierPolicy {
            tier: Tier::LightReview,
            owners: vec!["lead".into()],
            default_approve: true,
            sla: 2,
        });
        q.set_policy(TierPolicy {
            tier: Tier::FormalRfc,
            owners: vec!["board".into()],
            default_approve: false,
            sla: 3,
        });
        q.route(intake(&[DesignArea::PublicApi], true, None), "author")
            .unwrap();
        q.route(
            intake(&[DesignArea::SecurityBoundary], true, None),
            "author",
        )
        .unwrap();
        // 第二条 Light：不拍板，留给 SLA 默认通过
        q.route(intake(&[DesignArea::DataModel], true, None), "author")
            .unwrap();
        assert_eq!(q.pending().len(), 3);

        // owner 拍板：有权
        assert_eq!(q.approve(0, "lead").unwrap(), QueueStatus::Approved);
        // 非 owner 拍板：拒绝（所有权不可代理）
        assert!(
            q.approve(1, "lead").is_err(),
            "LightReview 的 owner 不能拍 FormalRfc"
        );

        // SLA 走完：未拍板的 Light 默认通过；Formal 升级
        q.tick();
        q.tick();
        q.tick();
        assert_eq!(
            q.entries()[0].status,
            QueueStatus::Approved,
            "手拍板的不受 SLA 影响"
        );
        assert_eq!(
            q.entries()[1].status,
            QueueStatus::Escalated,
            "Formal 不默认通过"
        );
        assert_eq!(
            q.entries()[2].status,
            QueueStatus::DefaultApproved,
            "Light 限时默认通过"
        );
        assert!(q.pending().is_empty());
    }

    #[test]
    fn codified_architecture_test_runs_on_dag() {
        let mut dag = ImmuneDag::root("main");
        let hub = dag.spawn(NodeId(0), crate::NodeKind::Delta, "枢纽");
        for i in 0..3 {
            let x = dag.spawn(NodeId(0), crate::NodeKind::Delta, &format!("依赖{i}"));
            dag.cross_link(x, hub, crate::Trit::Hardened, "依赖");
        }

        let mut statutes = Statutes::default();
        statutes.codify(Statute::ArchTest(ArchTest::MaxFanIn(2)));
        let v = statutes.arch_test(&dag);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].node, hub);
        assert!(v[0].detail.contains("超阈"));

        // 收紧后无违规（审计口径：未检查不当通过——但没有机器规则时返回空，如实分账）
        statutes.codify(Statute::Principle("扇入即债务".into()));
        assert_eq!(statutes.arch_test(&dag).len(), 1, "文字原则不参与机器检查");
    }
}
