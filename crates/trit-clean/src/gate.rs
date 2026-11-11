//! 三支决策门（C3）：把清洁度证据变成 `accept` / `reject` / `defer` 三个动作。
//!
//! 两种策略：
//! - [`Policy::Loss`]：决策理论口径——按代价矩阵最小化期望损失（平局默认 `defer`）；
//! - [`Policy::Thresholds`]：显式阈值——`p >= accept_at` 收、`p <= reject_at` 拒、其余转人工。
//!
//! 证据（posterior）口径：
//! - 有填充集的行：`completions_or_sample` 的一致比例（精确或抽样，原因码写明）；
//! - 无填充集且无 False/Unknown：`1.0`（Clean）；
//! - 只有 Unknown：`0.5`（**证据不足的显式约定**，默认代价下会落到 defer）；
//! - 有 False：`0.0`（Violating）；
//! - 表级 `UNIQUE` 冲突且该行没有自己的证据：`0.5`（不知道是哪一行的错 ⇒ 转人工）。
//!
//! 计数出错（域为空、类型不符等输入契约问题）不会静默降级：直接从 `gate` 返回错误。

use std::fmt;

use trit_core::value::Ternary;

use crate::dsl::Rule;
use crate::eval::evaluate;
use crate::repair::{Fill, RepairError};
use crate::sample::{CompletionOutcome, SampleBudget, completions_or_sample};
use crate::value::Table;

/// 三种动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// 接受（干净，或可自动修复）。
    Accept,
    /// 拒绝（确定脏且修不动）。
    Reject,
    /// 转人工（证据不足）。
    Defer,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::Defer => "defer",
        })
    }
}

/// 代价矩阵：`cost(action | state)`，状态只有 Clean / Dirty 两种。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Costs {
    /// 接受干净行。
    pub accept_clean: f64,
    /// 接受脏行（漏放）。
    pub accept_dirty: f64,
    /// 拒绝干净行（误杀）。
    pub reject_clean: f64,
    /// 拒绝脏行。
    pub reject_dirty: f64,
    /// 干净行转人工。
    pub defer_clean: f64,
    /// 脏行转人工。
    pub defer_dirty: f64,
}

impl Default for Costs {
    /// 默认代价：漏放（accept_dirty=10）与误杀（reject_clean=5）都远高于转人工（1）。
    fn default() -> Self {
        Self {
            accept_clean: 0.0,
            accept_dirty: 10.0,
            reject_clean: 5.0,
            reject_dirty: 0.0,
            defer_clean: 1.0,
            defer_dirty: 1.0,
        }
    }
}

impl Costs {
    fn expected(&self, action: Action, clean: f64) -> f64 {
        let dirty = 1.0 - clean;
        match action {
            Action::Accept => clean * self.accept_clean + dirty * self.accept_dirty,
            Action::Reject => clean * self.reject_clean + dirty * self.reject_dirty,
            Action::Defer => clean * self.defer_clean + dirty * self.defer_dirty,
        }
    }

    fn validate(&self) -> Result<(), GateError> {
        let values = [
            self.accept_clean,
            self.accept_dirty,
            self.reject_clean,
            self.reject_dirty,
            self.defer_clean,
            self.defer_dirty,
        ];
        if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(GateError::InvalidPolicy {
                detail: "代价必须是有限非负数".into(),
            });
        }
        if self.accept_clean >= self.accept_dirty {
            return Err(GateError::InvalidPolicy {
                detail: "accept_clean 必须小于 accept_dirty（接错比接对更贵）".into(),
            });
        }
        if self.reject_dirty >= self.reject_clean {
            return Err(GateError::InvalidPolicy {
                detail: "reject_dirty 必须小于 reject_clean（拒对比拒错更贵）".into(),
            });
        }
        Ok(())
    }
}

/// 决策策略。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Policy {
    /// 按代价矩阵最小化期望损失。
    Loss(Costs),
    /// 显式阈值：`p >= accept_at` 收、`p <= reject_at` 拒、其余转人工。
    Thresholds {
        /// 接受阈值。
        accept_at: f64,
        /// 拒绝阈值。
        reject_at: f64,
    },
}

impl Policy {
    fn validate(&self) -> Result<(), GateError> {
        match self {
            Self::Loss(costs) => costs.validate(),
            Self::Thresholds {
                accept_at,
                reject_at,
            } => {
                let ok = accept_at.is_finite()
                    && reject_at.is_finite()
                    && (0.0..=1.0).contains(accept_at)
                    && (0.0..=1.0).contains(reject_at)
                    && *reject_at < *accept_at;
                if ok {
                    Ok(())
                } else {
                    Err(GateError::InvalidPolicy {
                        detail: "阈值必须满足 0 ≤ reject_at < accept_at ≤ 1".into(),
                    })
                }
            }
        }
    }

    fn decide(&self, posterior: f64) -> (Action, Option<[f64; 3]>) {
        match self {
            Self::Loss(costs) => {
                let losses = [
                    costs.expected(Action::Accept, posterior),
                    costs.expected(Action::Reject, posterior),
                    costs.expected(Action::Defer, posterior),
                ];
                // 数值容差：浮点下的“理论平局”仍然默认 defer（1e-12 以内视作相等）。
                const TIE_EPS: f64 = 1e-12;
                let mut best = Action::Defer;
                let mut best_loss = losses[2];
                if losses[0] + TIE_EPS < best_loss {
                    best = Action::Accept;
                    best_loss = losses[0];
                }
                if losses[1] + TIE_EPS < best_loss {
                    best = Action::Reject;
                }
                (best, Some(losses))
            }
            Self::Thresholds {
                accept_at,
                reject_at,
            } => {
                let action = if posterior >= *accept_at {
                    Action::Accept
                } else if posterior <= *reject_at {
                    Action::Reject
                } else {
                    Action::Defer
                };
                (action, None)
            }
        }
    }
}

/// 证据说明：哪几条规则在该行是 False / Unknown。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceNote {
    /// 判为 False 的规则下标。
    pub violated: Vec<usize>,
    /// 判为 Unknown 的规则下标。
    pub uncertain: Vec<usize>,
}

/// 结构化原因码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// 全部规则为 True。
    Clean,
    /// 只有 Unknown，没有计数证据。
    UnknownOnly {
        /// Unknown 的规则下标。
        rules: Vec<usize>,
    },
    /// 有 False，没有计数证据。
    Violating {
        /// False 的规则下标。
        rules: Vec<usize>,
    },
    /// 表级 UNIQUE 冲突，但没有行级证据指出是哪一行的错。
    TableViolation {
        /// 表级 False 的规则下标。
        rules: Vec<usize>,
    },
    /// 用精确计数得到的一致比例。
    Completion(EvidenceNote),
    /// 用抽样得到的一致比例（附样本数）。
    SampledCompletion {
        /// 抽样次数。
        samples: u64,
        /// 证据说明。
        note: EvidenceNote,
    },
}

/// 一行证据：这一行的哪些格子有候选域。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowEvidence {
    /// 行下标。
    pub row: usize,
    /// 该行的填充集（空 = 没有计数证据）。
    pub fills: Vec<Fill>,
}

impl RowEvidence {
    /// 新建一行证据。
    #[must_use]
    pub fn new(row: usize, fills: Vec<Fill>) -> Self {
        Self { row, fills }
    }
}

/// 单行判决。
#[derive(Clone, Debug, PartialEq)]
pub struct ItemDecision {
    /// 行下标。
    pub row: usize,
    /// 后验清洁度证据。
    pub posterior: f64,
    /// 动作。
    pub action: Action,
    /// 期望损失（仅 `Policy::Loss` 下有值）。
    pub losses: Option<[f64; 3]>,
    /// 结构化原因码。
    pub reason: Reason,
}

/// 汇总计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GateCounts {
    /// accept 行数。
    pub accept: usize,
    /// reject 行数。
    pub reject: usize,
    /// defer 行数。
    pub defer: usize,
}

/// 判决报告。
#[derive(Clone, Debug, PartialEq)]
pub struct GateReport {
    /// 每行一个判决，按行下标升序。
    pub items: Vec<ItemDecision>,
    /// 三动作计数。
    pub counts: GateCounts,
}

/// 对整表做三支决策；`evidence` 逐行给出填充集（缺省行按规则状态推导）。
pub fn gate(
    table: &Table,
    rules: &[Rule],
    evidence: &[RowEvidence],
    policy: &Policy,
    budget: SampleBudget,
) -> Result<GateReport, GateError> {
    policy.validate()?;
    let report = evaluate(table, rules);
    let mut states: Vec<Vec<(usize, Ternary)>> = vec![Vec::new(); table.rows.len()];
    for node in &report.rows {
        states[node.row].push((node.rule, node.state));
    }
    let table_violations: Vec<usize> = report
        .tables
        .iter()
        .filter(|t| t.state == Ternary::False)
        .map(|t| t.rule)
        .collect();
    let mut evidence_by_row: Vec<Option<&RowEvidence>> = vec![None; table.rows.len()];
    for entry in evidence {
        let slot = evidence_by_row
            .get_mut(entry.row)
            .ok_or(GateError::RowOutOfRange { row: entry.row })?;
        if slot.replace(entry).is_some() {
            return Err(GateError::DuplicateEvidence { row: entry.row });
        }
    }
    let mut items = Vec::with_capacity(table.rows.len());
    for row in 0..table.rows.len() {
        let violated: Vec<usize> = states[row]
            .iter()
            .filter(|(_, state)| *state == Ternary::False)
            .map(|(rule, _)| *rule)
            .collect();
        let uncertain: Vec<usize> = states[row]
            .iter()
            .filter(|(_, state)| *state == Ternary::Unknown)
            .map(|(rule, _)| *rule)
            .collect();
        let note = EvidenceNote {
            violated: violated.clone(),
            uncertain: uncertain.clone(),
        };
        let (posterior, reason) = match evidence_by_row[row] {
            Some(entry) if !entry.fills.is_empty() => {
                match completions_or_sample(table, rules, &entry.fills, budget)? {
                    CompletionOutcome::Exact(completion) => {
                        (completion.ratio(), Reason::Completion(note))
                    }
                    CompletionOutcome::Sampled(sampled) => (
                        sampled.ratio,
                        Reason::SampledCompletion {
                            samples: sampled.samples,
                            note,
                        },
                    ),
                }
            }
            _ => default_evidence(&violated, &uncertain, &table_violations),
        };
        let (action, losses) = policy.decide(posterior);
        items.push(ItemDecision {
            row,
            posterior,
            action,
            losses,
            reason,
        });
    }
    let counts = GateCounts {
        accept: items.iter().filter(|i| i.action == Action::Accept).count(),
        reject: items.iter().filter(|i| i.action == Action::Reject).count(),
        defer: items.iter().filter(|i| i.action == Action::Defer).count(),
    };
    Ok(GateReport { items, counts })
}

fn default_evidence(
    violated: &[usize],
    uncertain: &[usize],
    table_violations: &[usize],
) -> (f64, Reason) {
    if !violated.is_empty() {
        (
            0.0,
            Reason::Violating {
                rules: violated.to_vec(),
            },
        )
    } else if !uncertain.is_empty() {
        (
            0.5,
            Reason::UnknownOnly {
                rules: uncertain.to_vec(),
            },
        )
    } else if !table_violations.is_empty() {
        (
            0.5,
            Reason::TableViolation {
                rules: table_violations.to_vec(),
            },
        )
    } else {
        (1.0, Reason::Clean)
    }
}

/// 一个扫描点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanPoint {
    /// 对称置信阈值：`p >= c` 收、`p <= 1-c` 拒、其余转人工。
    pub confidence: f64,
    /// accept 数。
    pub accept: usize,
    /// reject 数。
    pub reject: usize,
    /// defer 数。
    pub defer: usize,
    /// 按代价矩阵算的期望总代价。
    pub expected_cost: f64,
}

/// 阈值扫描：对一组后验按对称置信阈值扫一遍，给出三动作计数与期望总代价。
pub fn scan_confidence(
    posteriors: &[f64],
    costs: &Costs,
    steps: usize,
) -> Result<Vec<ScanPoint>, GateError> {
    costs.validate()?;
    if steps == 0 {
        return Err(GateError::InvalidPolicy {
            detail: "扫描步数必须 > 0".into(),
        });
    }
    if posteriors
        .iter()
        .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
    {
        return Err(GateError::InvalidPolicy {
            detail: "后验必须落在 [0, 1]".into(),
        });
    }
    let mut points = Vec::with_capacity(steps + 1);
    for step in 0..=steps {
        let confidence = 0.5 + 0.5 * (step as f64) / (steps as f64);
        let mut accept = 0;
        let mut reject = 0;
        let mut defer = 0;
        let mut expected_cost = 0.0;
        for &posterior in posteriors {
            let action = if posterior >= confidence {
                Action::Accept
            } else if posterior <= 1.0 - confidence {
                Action::Reject
            } else {
                Action::Defer
            };
            match action {
                Action::Accept => accept += 1,
                Action::Reject => reject += 1,
                Action::Defer => defer += 1,
            }
            expected_cost += costs.expected(action, posterior);
        }
        points.push(ScanPoint {
            confidence,
            accept,
            reject,
            defer,
            expected_cost,
        });
    }
    Ok(points)
}

/// 三支决策错误。
#[derive(Debug, thiserror::Error)]
pub enum GateError {
    /// 策略非法。
    #[error("策略非法：{detail}")]
    InvalidPolicy {
        /// 具体原因。
        detail: String,
    },
    /// 同一行给了两份证据。
    #[error("证据行重复：第 {row} 行")]
    DuplicateEvidence {
        /// 行下标。
        row: usize,
    },
    /// 证据行越界。
    #[error("证据行越界：第 {row} 行")]
    RowOutOfRange {
        /// 行下标。
        row: usize,
    },
    /// 计数错误（输入契约问题）。
    #[error(transparent)]
    Repair(#[from] RepairError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::CmpOp;
    use crate::value::{Column, Ty, Value};

    fn table(rows: Vec<Vec<Value>>) -> Table {
        let mut t = Table::new(vec![
            Column::new("age", Ty::Int),
            Column::new("tag", Ty::Text),
        ]);
        for r in rows {
            t.push_row(r).unwrap();
        }
        t
    }

    fn check_ge(limit: i64) -> Rule {
        Rule::Check {
            col: 0,
            op: CmpOp::Ge,
            lit: Value::Int(limit),
        }
    }

    #[test]
    fn loss_policy_three_regions() {
        let policy = Policy::Loss(Costs::default());
        assert_eq!(policy.decide(1.0).0, Action::Accept);
        assert_eq!(policy.decide(0.95).0, Action::Accept);
        assert_eq!(policy.decide(0.5).0, Action::Defer);
        assert_eq!(policy.decide(0.1).0, Action::Reject);
        assert_eq!(policy.decide(0.0).0, Action::Reject);
        // 平局（p=0.9：accept 期望损失 1.0 = defer 1.0）默认 defer。
        assert_eq!(policy.decide(0.9).0, Action::Defer);
    }

    #[test]
    fn threshold_policy_boundaries() {
        let policy = Policy::Thresholds {
            accept_at: 0.9,
            reject_at: 0.1,
        };
        assert_eq!(policy.decide(0.9).0, Action::Accept);
        assert_eq!(policy.decide(0.899).0, Action::Defer);
        assert_eq!(policy.decide(0.1).0, Action::Reject);
        assert_eq!(policy.decide(0.101).0, Action::Defer);
        assert!(policy.decide(0.9).1.is_none(), "阈值模式不给期望损失");
    }

    #[test]
    fn invalid_policies_are_rejected() {
        let bad = Policy::Thresholds {
            accept_at: 0.5,
            reject_at: 0.5,
        };
        assert!(matches!(
            bad.validate(),
            Err(GateError::InvalidPolicy { .. })
        ));
        let costs = Costs {
            accept_clean: 11.0,
            ..Costs::default()
        };
        assert!(matches!(
            Policy::Loss(costs).validate(),
            Err(GateError::InvalidPolicy { .. })
        ));
    }

    #[test]
    fn gate_derives_reasons_per_row() {
        let t = table(vec![
            vec![Value::Int(1), Value::Text("a".into())],
            vec![Value::Int(-1), Value::Text("b".into())],
            vec![Value::Null, Value::Text("c".into())],
        ]);
        let rules = vec![check_ge(0)];
        let policy = Policy::Loss(Costs::default());
        let report = gate(&t, &rules, &[], &policy, SampleBudget::default()).unwrap();
        assert_eq!(report.items[0].action, Action::Accept);
        assert_eq!(report.items[0].reason, Reason::Clean);
        assert_eq!(report.items[1].action, Action::Reject);
        assert_eq!(report.items[1].reason, Reason::Violating { rules: vec![0] });
        assert_eq!(report.items[2].action, Action::Defer);
        assert_eq!(
            report.items[2].reason,
            Reason::UnknownOnly { rules: vec![0] }
        );
        assert_eq!(
            report.counts,
            GateCounts {
                accept: 1,
                reject: 1,
                defer: 1
            }
        );
    }

    #[test]
    fn gate_uses_counts_as_evidence() {
        let t = table(vec![vec![Value::Null, Value::Text("a".into())]]);
        let rules = vec![check_ge(2)];
        let evidence = vec![RowEvidence::new(
            0,
            vec![Fill::new(
                0,
                0,
                vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            )],
        )];
        let policy = Policy::Loss(Costs::default());
        let report = gate(&t, &rules, &evidence, &policy, SampleBudget::default()).unwrap();
        // 一致完成 2/3 ⇒ 后验 0.6667，默认代价下落在 defer 区。
        assert!((report.items[0].posterior - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(report.items[0].action, Action::Defer);
        assert!(matches!(
            report.items[0].reason,
            Reason::Completion(EvidenceNote { .. })
        ));
    }

    #[test]
    fn gate_sampled_evidence_is_marked() {
        let cols: Vec<Column> = (0..13)
            .map(|i| Column::new(&format!("c{i}"), Ty::Int))
            .collect();
        let mut t = Table::new(cols);
        t.push_row(vec![Value::Null; 13]).unwrap();
        let rules = vec![Rule::Check {
            col: 0,
            op: CmpOp::Lt,
            lit: Value::Int(2),
        }];
        let domain: Vec<Value> = (0..10).map(Value::Int).collect();
        let fills: Vec<Fill> = (0..13)
            .map(|col| Fill::new(0, col, domain.clone()))
            .collect();
        let evidence = vec![RowEvidence::new(0, fills)];
        let budget = SampleBudget {
            samples: 20_000,
            seed: 9,
        };
        let policy = Policy::Thresholds {
            accept_at: 0.9,
            reject_at: 0.1,
        };
        let report = gate(&t, &rules, &evidence, &policy, budget).unwrap();
        match &report.items[0].reason {
            Reason::SampledCompletion { samples, .. } => assert_eq!(*samples, 20_000),
            other => panic!("期望抽样原因码，实际 {other:?}"),
        }
        // 真值 0.2，落在 (0.1, 0.9) ⇒ defer。
        assert_eq!(report.items[0].action, Action::Defer);
    }

    #[test]
    fn gate_marks_table_violation_as_defer() {
        let t = table(vec![
            vec![Value::Int(1), Value::Text("a".into())],
            vec![Value::Int(1), Value::Text("b".into())],
        ]);
        let rules = vec![Rule::Unique { col: 0 }];
        let policy = Policy::Loss(Costs::default());
        let report = gate(&t, &rules, &[], &policy, SampleBudget::default()).unwrap();
        for item in &report.items {
            assert_eq!(item.action, Action::Defer, "{item:?}");
            assert_eq!(item.reason, Reason::TableViolation { rules: vec![0] });
        }
    }

    #[test]
    fn duplicate_or_out_of_range_evidence_is_an_error() {
        let t = table(vec![vec![Value::Int(1), Value::Text("a".into())]]);
        let rules = vec![check_ge(0)];
        let policy = Policy::Thresholds {
            accept_at: 0.9,
            reject_at: 0.1,
        };
        let err = gate(
            &t,
            &rules,
            &[RowEvidence::new(0, vec![]), RowEvidence::new(0, vec![])],
            &policy,
            SampleBudget::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, GateError::DuplicateEvidence { row: 0 }),
            "{err}"
        );
        let err = gate(
            &t,
            &rules,
            &[RowEvidence::new(9, vec![])],
            &policy,
            SampleBudget::default(),
        )
        .unwrap_err();
        assert!(matches!(err, GateError::RowOutOfRange { row: 9 }), "{err}");
    }

    #[test]
    fn scan_confidence_reports_three_way_counts() {
        let posteriors = [1.0, 0.5, 0.0];
        let points = scan_confidence(&posteriors, &Costs::default(), 2).unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].confidence, 0.5);
        assert_eq!(
            (points[0].accept, points[0].reject, points[0].defer),
            (2, 1, 0)
        );
        assert_eq!(
            (points[1].accept, points[1].reject, points[1].defer),
            (1, 1, 1)
        );
        assert_eq!(
            (points[2].accept, points[2].reject, points[2].defer),
            (1, 1, 1)
        );
        assert!(
            points.windows(2).all(|w| w[0].accept >= w[1].accept),
            "accept 数随置信阈值单调不增"
        );
    }

    #[test]
    fn scan_rejects_bad_inputs() {
        assert!(matches!(
            scan_confidence(&[0.5], &Costs::default(), 0),
            Err(GateError::InvalidPolicy { .. })
        ));
        assert!(matches!(
            scan_confidence(&[f64::NAN], &Costs::default(), 2),
            Err(GateError::InvalidPolicy { .. })
        ));
    }
}
