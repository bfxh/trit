//! 执行 / 验证 / 对账三种机器预算，以及独立的人的注意力预算。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::flip_rates::{FlipRates, ScorePolicy, order_obligations_by_score};
use crate::obligation::{
    AdmissionObligation, ObligationKind, ObligationSkip, ObligationSkipReason, order_obligations,
};
use crate::{Attempt, Error, Ledger, Selection, TaskSpec, dispatch_ready};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetClass {
    Execution,
    Verification,
    Reconciliation,
}

impl BudgetClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Execution => "execution",
            Self::Verification => "verification",
            Self::Reconciliation => "reconciliation",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetSplit {
    pub execution: u64,
    pub verification: u64,
    pub reconciliation: u64,
    pub human: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetReport {
    pub execution_spent: u64,
    pub execution_reserved: u64,
    pub verification_spent: u64,
    pub reconciliation_spent: u64,
    pub human_spent: usize,
    pub execution_remaining: u64,
    pub verification_remaining: u64,
    pub reconciliation_remaining: u64,
    pub human_remaining: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub operation: String,
    pub task: String,
    pub epoch: u64,
    pub tokens: u64,
}

impl ExecutionRequest {
    /// 从选批结果生成派发请求；只取 selected 任务，token 来自 `granted_tokens`。
    pub fn from_selection(selection: &Selection, tasks: &[TaskSpec]) -> Vec<Self> {
        let selected: BTreeSet<&str> = selection.selected.iter().map(String::as_str).collect();
        selection
            .decisions
            .iter()
            .filter(|decision| selected.contains(decision.task.as_str()))
            .filter_map(|decision| {
                let spec = tasks.iter().find(|spec| spec.task.id == decision.task)?;
                Some(Self {
                    operation: decision.task.clone(),
                    task: decision.task.clone(),
                    epoch: spec.epoch,
                    tokens: decision.granted_tokens,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReservation {
    pub operation: String,
    pub task: String,
    pub epoch: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSettlement {
    pub operation: String,
    pub reserved: u64,
    pub used: u64,
    pub released: u64,
    pub overrun: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DispatchBudgetError {
    #[error(transparent)]
    Budget(#[from] BudgetError),
    #[error(transparent)]
    Dispatch(#[from] Error),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum BudgetError {
    #[error("budget {class:?} exhausted: required {required}, available {available}")]
    Tokens {
        class: BudgetClass,
        required: u64,
        available: u64,
    },
    #[error("human budget exhausted: required {required}, available {available}")]
    Human { required: usize, available: usize },
    #[error("duplicate execution reservation for {operation:?}")]
    DuplicateReservation { operation: String },
    #[error("unknown execution reservation {operation:?}")]
    UnknownReservation { operation: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetBook {
    limits: BudgetSplit,
    execution_spent: u64,
    verification_spent: u64,
    reconciliation_spent: u64,
    human_spent: usize,
    execution_reservations: BTreeMap<String, ExecutionReservation>,
}

impl BudgetBook {
    pub fn new(limits: BudgetSplit) -> Self {
        Self {
            limits,
            execution_spent: 0,
            verification_spent: 0,
            reconciliation_spent: 0,
            human_spent: 0,
            execution_reservations: BTreeMap::new(),
        }
    }

    pub fn remaining(&self, class: BudgetClass) -> u64 {
        match class {
            BudgetClass::Execution => self
                .limits
                .execution
                .saturating_sub(self.execution_spent)
                .saturating_sub(self.execution_reserved()),
            BudgetClass::Verification => self
                .limits
                .verification
                .saturating_sub(self.verification_spent),
            BudgetClass::Reconciliation => self
                .limits
                .reconciliation
                .saturating_sub(self.reconciliation_spent),
        }
    }

    pub fn spent(&self, class: BudgetClass) -> u64 {
        match class {
            BudgetClass::Execution => self.execution_spent,
            BudgetClass::Verification => self.verification_spent,
            BudgetClass::Reconciliation => self.reconciliation_spent,
        }
    }

    pub fn human_remaining(&self) -> usize {
        self.limits.human.saturating_sub(self.human_spent)
    }

    pub fn human_spent(&self) -> usize {
        self.human_spent
    }

    pub fn report(&self) -> BudgetReport {
        BudgetReport {
            execution_spent: self.execution_spent,
            execution_reserved: self.execution_reserved(),
            verification_spent: self.verification_spent,
            reconciliation_spent: self.reconciliation_spent,
            human_spent: self.human_spent,
            execution_remaining: self.remaining(BudgetClass::Execution),
            verification_remaining: self.remaining(BudgetClass::Verification),
            reconciliation_remaining: self.remaining(BudgetClass::Reconciliation),
            human_remaining: self.human_remaining(),
        }
    }

    pub fn charge(&mut self, class: BudgetClass, tokens: u64) -> Result<(), BudgetError> {
        self.try_charge(class, tokens, 0)
    }

    fn execution_reserved(&self) -> u64 {
        self.execution_reservations
            .values()
            .fold(0u64, |sum, reservation| {
                sum.saturating_add(reservation.tokens)
            })
    }

    /// 批量预留执行预算；任一操作重复或总额不足都不产生部分预留。
    pub fn reserve_batch(
        &mut self,
        requests: impl IntoIterator<Item = ExecutionRequest>,
    ) -> Result<Vec<ExecutionReservation>, BudgetError> {
        let requests: Vec<ExecutionRequest> = requests.into_iter().collect();
        let mut seen = BTreeSet::new();
        for request in &requests {
            if !seen.insert(request.operation.clone())
                || self.execution_reservations.contains_key(&request.operation)
            {
                return Err(BudgetError::DuplicateReservation {
                    operation: request.operation.clone(),
                });
            }
        }
        let mut total = 0u64;
        for request in &requests {
            total = total
                .checked_add(request.tokens)
                .ok_or(BudgetError::Tokens {
                    class: BudgetClass::Execution,
                    required: u64::MAX,
                    available: self.remaining(BudgetClass::Execution),
                })?;
        }
        let available = self.remaining(BudgetClass::Execution);
        if total > available {
            return Err(BudgetError::Tokens {
                class: BudgetClass::Execution,
                required: total,
                available,
            });
        }

        let mut reservations = Vec::with_capacity(requests.len());
        for request in requests {
            let reservation = ExecutionReservation {
                operation: request.operation,
                task: request.task,
                epoch: request.epoch,
                tokens: request.tokens,
            };
            self.execution_reservations
                .insert(reservation.operation.clone(), reservation.clone());
            reservations.push(reservation);
        }
        Ok(reservations)
    }

    /// 结算执行预留：实际用量进入 spent，未用部分释放；超支作为既成事实记录。
    pub fn settle_execution(
        &mut self,
        operation: &str,
        used: u64,
    ) -> Result<ExecutionSettlement, BudgetError> {
        let reservation = self
            .execution_reservations
            .remove(operation)
            .ok_or_else(|| BudgetError::UnknownReservation {
                operation: operation.to_owned(),
            })?;
        self.execution_spent = self.execution_spent.saturating_add(used);
        Ok(ExecutionSettlement {
            operation: reservation.operation,
            reserved: reservation.tokens,
            used,
            released: reservation.tokens.saturating_sub(used),
            overrun: used.saturating_sub(reservation.tokens),
        })
    }

    /// 取消/失败时释放预留，不产生 spent。
    pub fn release_execution(
        &mut self,
        operation: &str,
    ) -> Result<ExecutionReservation, BudgetError> {
        self.execution_reservations
            .remove(operation)
            .ok_or_else(|| BudgetError::UnknownReservation {
                operation: operation.to_owned(),
            })
    }

    pub fn charge_human(&mut self, count: usize) -> Result<(), BudgetError> {
        let available = self.human_remaining();
        if count > available {
            return Err(BudgetError::Human {
                required: count,
                available,
            });
        }
        self.human_spent += count;
        Ok(())
    }

    /// 原子扣费：token 或人的预算任一不足，都不修改任何计数。
    pub fn try_charge(
        &mut self,
        class: BudgetClass,
        tokens: u64,
        human: usize,
    ) -> Result<(), BudgetError> {
        let available = self.remaining(class);
        if tokens > available {
            return Err(BudgetError::Tokens {
                class,
                required: tokens,
                available,
            });
        }
        let human_available = self.human_remaining();
        if human > human_available {
            return Err(BudgetError::Human {
                required: human,
                available: human_available,
            });
        }
        self.add_spent(class, tokens);
        self.human_spent += human;
        Ok(())
    }

    fn add_spent(&mut self, class: BudgetClass, tokens: u64) {
        match class {
            BudgetClass::Execution => self.execution_spent += tokens,
            BudgetClass::Verification => self.verification_spent += tokens,
            BudgetClass::Reconciliation => self.reconciliation_spent += tokens,
        }
    }
}

/// 义务默认归入哪个机器预算：对账单独一池，其余都算验证。
pub const fn obligation_class(kind: ObligationKind) -> BudgetClass {
    match kind {
        ObligationKind::Reconcile => BudgetClass::Reconciliation,
        _ => BudgetClass::Verification,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetedObligationBatch {
    pub scheduled: Vec<AdmissionObligation>,
    pub skipped: Vec<ObligationSkip>,
    pub overdue: Vec<String>,
    pub budget: BudgetReport,
}

/// 用三池预算排义务：按类扣费，类内不足只跳过当前项，继续尝试后面的小项。
pub fn schedule_obligations_with_budget(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    book: &mut BudgetBook,
) -> BudgetedObligationBatch {
    schedule_obligations_with_budget_by(obligations, now, book, order_obligations)
}

/// 三池预算 + 期望信息增益/成本评分排序。
pub fn schedule_obligations_with_budget_by_score(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    book: &mut BudgetBook,
    rates: &FlipRates,
    policy: &ScorePolicy,
) -> BudgetedObligationBatch {
    schedule_obligations_with_budget_by(obligations, now, book, |left, right| {
        order_obligations_by_score(left, right, rates, policy)
    })
}

fn schedule_obligations_with_budget_by(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    book: &mut BudgetBook,
    order: impl Fn(&AdmissionObligation, &AdmissionObligation) -> Ordering,
) -> BudgetedObligationBatch {
    let mut obligations: Vec<AdmissionObligation> = obligations.into_iter().collect();
    for obligation in &mut obligations {
        obligation.ensure_id();
    }
    obligations.sort_by(order);

    let mut batch = BudgetedObligationBatch::default();
    let mut seen = BTreeSet::new();
    for obligation in obligations {
        if !seen.insert(obligation.id.clone()) {
            batch.skipped.push(ObligationSkip {
                id: obligation.id.clone(),
                kind: obligation.kind,
                reason: ObligationSkipReason::Duplicate,
            });
            continue;
        }
        let class = obligation_class(obligation.kind);
        let human = usize::from(obligation.cost.human);
        match book.try_charge(class, obligation.cost.tokens, human) {
            Ok(()) => {
                if obligation.deadline.is_some_and(|deadline| deadline <= now) {
                    batch.overdue.push(obligation.id.clone());
                }
                batch.scheduled.push(obligation);
            }
            Err(BudgetError::Human { .. }) => {
                batch.skipped.push(ObligationSkip {
                    id: obligation.id.clone(),
                    kind: obligation.kind,
                    reason: ObligationSkipReason::NoHuman,
                });
            }
            Err(BudgetError::Tokens {
                required,
                available,
                ..
            }) => {
                batch.skipped.push(ObligationSkip {
                    id: obligation.id.clone(),
                    kind: obligation.kind,
                    reason: ObligationSkipReason::OverBudget {
                        required,
                        available,
                    },
                });
            }
            Err(_) => {
                // try_charge 不会返回预留类错误；未知错误保守跳过，不静默放行。
                batch.skipped.push(ObligationSkip {
                    id: obligation.id.clone(),
                    kind: obligation.kind,
                    reason: ObligationSkipReason::OverBudget {
                        required: 0,
                        available: 0,
                    },
                });
            }
        }
    }
    batch.budget = book.report();
    batch
}

/// 派发前按选批 grants 预留执行预算。
pub fn reserve_for_dispatch(
    selection: &Selection,
    tasks: &[TaskSpec],
    book: &mut BudgetBook,
) -> Result<Vec<ExecutionReservation>, BudgetError> {
    book.reserve_batch(ExecutionRequest::from_selection(selection, tasks))
}

/// 预留 + 派发校验的组合入口；校验失败时释放本次预留，避免泄漏。
pub fn dispatch_with_execution_budget(
    selection: &Selection,
    tasks: &[TaskSpec],
    ledger: &Ledger,
    book: &mut BudgetBook,
) -> Result<(Vec<Attempt>, Vec<ExecutionReservation>), DispatchBudgetError> {
    let reservations = reserve_for_dispatch(selection, tasks, book)?;
    match dispatch_ready(selection, tasks, ledger) {
        Ok(attempts) => Ok((attempts, reservations)),
        Err(error) => {
            for reservation in &reservations {
                let _ = book.release_execution(&reservation.operation);
            }
            Err(DispatchBudgetError::Dispatch(error))
        }
    }
}
