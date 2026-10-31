//! # trit-immune —— 三进制免疫系统核心
//!
//! 把"渗透测试"做成 DeltaDB 分支图的原生伴生机制：
//!
//! - 每个节点/分支用三进制裁决 [`Trit`] `{-1, 0, +1}` 标注（Kleene K3 传播）；
//!   `-1` 攻击成功、`0` 未知/沙盒挂起、`+1` 防御成功；
//! - **分支即攻击向量**：红队 Agent 在任意锚点零成本拉出攻击分支（[`NodeKind::AttackVector`]）；
//! - **负数消息回链**：攻击成功的 [`ExploitEvidence`] 直接指回被打穿的代码节点；
//! - **合并门禁**：[`ImmuneDag::merge_gate`] 对因果链做 K3 AND 折叠，
//!   得出三态动作 [`GateDecision`]（放行 / 冻结 / 阻断）；
//! - **人类主控 0 态**：[`ImmuneDag::human_review`] 是唯一能把隔离解除、把 0 推向 ±1 的人为通道；
//! - **Prompt 注入溯源**：`Message` 节点被污染时 [`ImmuneDag::quarantine`] 沿因果链隔离；
//! - **持久化**：[`Delta`] 是每次变更的一等公民编码，[`Journal`] 追加式落盘 +
//!   快照缓存（`<journal>.snapshot`，open 只重放尾部增量，损坏自动回退全量），
//!   实时写入与日志重放共用 `ImmuneDag::apply` 同一条代码路径——日志即数据库；
//! - **三值传播**：[`CrossLink`] 跨树污染边 + (min, max) 半环传播（[`Contagion`]，
//!   `TernaryTopology` 可达性的极性对偶），`merge_gate` 按有效裁决折叠；
//! - **静态扫描**：[`scan`] 把 SAST/规则引擎的原始告警校准成三值裁决
//!   （Dispositive → `-1`，Heuristic → `0`，**永不产生 `+1`**），走同一条 Delta 协议；
//! - **跨副本同步**：[`sync`] 以 HLC 戳为 Delta 身份去重，全序归并 + 本地 id 改写，
//!   收敛出与输入顺序无关的同一份 DAG；重放失败进隔离区，不静默丢弃。
//!
//! # 示例
//!
//! ```
//! use trit_immune::{GateDecision, ImmuneDag, NodeId, NodeKind, Trit};
//!
//! let mut dag = ImmuneDag::root("main");
//! let feat = dag.spawn(NodeId(0), NodeKind::Delta, "登录模块");
//! // 未测即 0：新代码默认冻结，而不是默认安全
//! assert_eq!(dag.merge_gate(feat), GateDecision::Freeze);
//!
//! dag.record(feat, Trit::Hardened, None, "回归测试通过");
//! assert_eq!(dag.merge_gate(feat), GateDecision::Allow);
//! ```
//!
//! 与 [bfxh/trit](https://github.com/bfxh/trit) 的关系：本 crate 复用同一套 K3 语义
//! （`trit-core` 的 `Ternary`），聚焦安全裁决域；可作为该工作区的新成员接入。

pub mod adoption;
pub mod attest;
pub mod auth;
pub mod bitparallel;
pub mod compat;
pub mod cull;
pub mod dag;
pub mod daemon;
pub mod delta;
pub mod design;
pub mod diag;
pub mod fleet;
pub mod harvest;
pub mod hlc;
pub mod import;
pub mod intake;
pub mod json;
#[cfg(kani)]
pub mod kani_proofs;
pub mod journal;
pub mod lifecycle;
pub mod lsp;
pub mod migrate;
pub mod net;
pub mod policy;
pub mod propagate;
pub mod recovery;
pub mod sandbox;
pub mod scan;
pub mod scheduler;
pub mod server;
pub mod snapshot;
pub mod supply;
pub mod sync;
pub mod taskmd;
pub mod template;
#[cfg(feature = "tls")]
pub mod tls;
pub mod transport;
pub mod trit;
pub mod verdict;
pub mod vex;
pub mod vocab;

pub use adoption::{Adoption, AdoptionState, Blast, ChangePackage, Impact, Receiver};
pub use auth::{ct_eq, hmac_sha256, sha256};
pub use attest::{attest, verify, PREDICATE_TYPE, STATEMENT_TYPE};
pub use bitparallel::{tri_pack, tri_unpack, PackError, TriWord};
pub use cull::{cull, CullAction, CullRules};
pub use daemon::{encode_frame, json_escape, read_frame, LspSession};
pub use dag::{BreachKind, Convergence, Coupling, CrossLink, ImmuneDag, Node, NodeId, NodeKind, VerdictEvent};
pub use delta::{merge_order, ApplyError, Delta, Stamped};
pub use design::{
    classify, grade, ArchTest, DesignArea, DesignQueue, PrIntake, QueueEntry, QueueStatus,
    Routing, Statute, Statutes, Tier, TierPolicy, Violation,
};
pub use diag::{layer_verdict, parse_rustc_stream, Diagnostic, Severity};
pub use fleet::{AttackChain, AttackChains, ChainStep, ChainVia, CrossRepoDelta, Fleet, FleetRow};
pub use harvest::{harvest, HarvestReport, Registry};
pub use hlc::{Clock, Hlc};
pub use import::{collect_git_log, export_dag_tsv, import_commits, parse_git_log, CommitRecord};
pub use intake::{find_dead_deps, intake_pr, parse_cargo_deps, DeadDep, IntakeReport};
pub use json::Json;
pub use journal::{parse_stamped_line, BackupMode, Journal, JournalError, OpenError};
pub use lifecycle::{Lifecycle, LifecycleNode};
pub use lsp::{parse_lsp_stream, LspDiagnostic};
pub use migrate::{detect_version, migrate_to_current, verify_after_migration, MigrationReport};
pub use net::check_url;
pub use policy::{decide, next_layer, Action, Channel, Decision, MergePolicy, PrSnapshot};
pub use propagate::Contagion;
pub use recovery::{BackupVault, Inventory, Retention, SnapshotInfo};
pub use sandbox::{FuzzReport, Sandbox};
pub use scan::{run_scan, Evidence, Finding, NodeFold, Rule, ScanReport};
pub use scheduler::{drain_plan, Handler, JournalHandler, Scheduler, Task, WorkReport};
pub use server::{FleetServer, Server};
pub use snapshot::SnapshotMeta;
pub use supply::{audit, encode_baseline, fingerprint, parse_baseline, parse_lock, Pin, SupplyFinding, SupplyKind};
pub use sync::{anti_entropy, merge_replicas, DeferredDelta, LogFrame, MergeOutcome};
pub use taskmd::{ingest as ingest_md, parse as parse_md_tasks, taskbar, IngestReport, MdTask};
pub use template::{audit_discipline, scaffold, Discipline, DisciplineViolation, ViolationKind};
pub use transport::{pull_merge_via, MemoryTransport, TcpTransport, Transport};
pub use trit::Trit;
pub use verdict::{merge_gate, AttackVector, ExploitEvidence, GateDecision};
pub use vex::{export_vex, OPENVEX_CONTEXT, VexStatus};
pub use vocab::{Absorption, Concept, Registration, Vocabulary};
