//! 准入受阻时的最小补证动作，以及它们的成本、归属、截止期与排期。
//!
//! 只有会让准入变成 `Unknown` 的原因才产生义务；`False` 原因
//! （容量上限、明确拒绝）是改计划，不是补证。
//!
//! 三块职责分开：`kind` 是类型与默认表，`schedule` 是排序与预算，`route` 是原因到义务的映射。

mod kind;
mod route;
mod schedule;

pub(crate) use kind::canonical_id;

pub use kind::{AdmissionObligation, ObligationCost, ObligationKind, ObligationPolicy};
pub use route::obligations_for;
pub use schedule::{
    ObligationBatch, ObligationBudget, ObligationSkip, ObligationSkipReason, schedule_obligations,
};
