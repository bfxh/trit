//! 游戏环境扫描、受控故障注入与恢复核心（Phase 0）。
//!
//! 本 crate 只依赖 [`trit_core`]，不依赖 Bevy/Unity/Unreal/Godot 或 MCP SDK；
//! 引擎与协议依赖应由 adapter crate 承担。
//!
//! 提供：
//! - [`Ternary`] 三值判定（复用 `trit-core` 的 Kleene 语义：`True` 通过、
//!   `False` 失败、`Unknown` 证据不足）；
//! - 规范化的 [`WorldSnapshot`] 与结构/物理规则；
//! - [`PenetrationOracle`]：sphere/capsule/box 的 collider 穿模检测；
//! - [`MutationPlan`] / [`Mutator`]：受控状态污染；
//! - [`RecoveryCoordinator`]：checkpoint → 注入 → 扫描 → 回滚 → 验证；
//! - [`ReplayWorld`]：Phase 0 的 replay adapter。
//!
//! 设计文档见仓库 `docs/GAME-SCAN.md`。

#![forbid(unsafe_code)]

mod chaos;
mod findings;
mod hash;
mod penetration;
mod recovery;
mod replay;
mod rule;
mod rules;
mod snapshot;

pub use chaos::{Mutation, MutationError, MutationPlan, MutationReceipt, Mutator};
pub use findings::{Finding, RuleOutcome, ScanReport, Severity};
pub use penetration::{PenetrationConfig, PenetrationOracle};
pub use recovery::{
    CheckpointId, CheckpointStore, InMemoryCheckpointStore, RecoveryCheck, RecoveryCoordinator,
    RecoveryError, RecoveryStatus, RestoreLevel, TransactionOutcome, VerifyReport,
};
pub use replay::ReplayWorld;
pub use rule::{StructuralRule, run_rules};
pub use rules::default_structural_rules;
pub use snapshot::{
    ColliderShape, ColliderSnapshot, EntityId, EntitySnapshot, ResourceSnapshot, Transform,
    WorldSnapshot,
};
pub use trit_core::Ternary;
