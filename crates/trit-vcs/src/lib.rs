//! `trit-vcs`：内容寻址的协作版本库（M1：存储 / 审计 / 认领门）。
//!
//! 这不是"再写一个 Git"，而是把 Git 做不到或做弱的三件事做成硬约束，同时把 Git 做得好、
//! 但没必要重写的东西留给 Git（详见 `docs/10-全面替代Git-每个维度都要更强.md`）：
//!
//! | 维度 | 这里的做法 | 与 Git 比 |
//! |---|---|---|
//! | 内容完整性 | BLAKE3（256 位），**读时重算校验** | SHA-1 → BLAKE3：抗碰撞更强、更快 |
//! | 存体积 | **内容定义分块 + 块级去重** | 整文件 blob：中间插一段就要整块新对象，这里只多几个块 |
//! | 审计 | **哈希链事件日志**（提交/暂存/门禁结论全进链） | reflog 是本地可改写、gc 会丢 |
//! | 并发写 | **未认领的提交默认拒绝**（复用 `trit-orch` 的认领语义） | Git 不拦，冲突在 push/merge 才炸 |
//! | 提交元数据 | 会话身份 + 认领 id + 证据 + 标签进对象内容 | 只有 author/committer 字符串 |
//! | 时间 | 混合逻辑时钟（墙上倒退也全序） | 墙上时钟，可乱 |
//! | 证据 | **测试见证绑定树；合并准入离线复核，改一个字节就拒** | 无内建证据，CI 结论在外部、可被重跑改写 |
//! | 传输 | **块级闭包 + 目录级部分拉取 + 断点续传**（本地包；网络协议未做） | pack 协议成熟；我们只做离线包，不宣称网络面 |
//!
//! 合并、Git 兼容桥在 M4–M6；没做的维度不在这里宣称。

pub mod bundle;
pub mod central;
pub mod chunk;
pub mod delta;
pub mod gate;
pub mod git_bridge;
pub mod hash;
pub mod journal;
pub mod mcp;
pub mod merge;
pub mod model;
pub mod preflight;
pub mod remediation;
pub mod repo;
pub mod status_cache;
pub mod store;
pub mod techmap;
pub mod tokens;
pub mod witness;
pub mod worktree;

pub use central::{PublishStats, PullStats, publish as central_publish, pull as central_pull};
pub use chunk::Chunking;
pub use gate::{ClaimSpec, Verdict, check as gate_check};
pub use hash::Hash;
pub use journal::{Event, Journal};
pub use model::{Commit, Hlc, Intent};
pub use preflight::{Facts as PreflightFacts, SizeFacts};
pub use remediation::{Context as RemediationContext, commands as verdict_commands};
pub use repo::{CommitFilter, CommitOutcome, CommitRequest, Repo, RepoError, VerifyReport};
pub use store::{FileMap, PutStats, Stats, Store, StoreError};
pub use witness::{Witness, WitnessError};
pub use worktree::{Change, Kind as ChangeKind, StatusStats, WorktreeError};
