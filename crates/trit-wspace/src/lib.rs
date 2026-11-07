//! # trit-wspace — 三值权重空间编程
//!
//! 传统流程是“写代码 → 训练 → 得到权重”。权重空间编程（weight-space programming）
//! 反过来把**权重本身当作源码**：读出行为、写出补丁、验证后再提交。2026-11-02
//! MIT CSAIL 报告的工作（GPT-6 Astra 直接读写 Qwen3.5-0.8B 的原始权重，改约 0.016%
//! 的权重把 MATH-500 从 7.9% 提到 48.1%，不走梯度）把这条路线推到台前；该结果来自
//! 外部报道，本仓**未复现**，这里只借用它的接口形态：读 / 写 / 验证。
//!
//! ## 三值为什么适合权重空间编程
//!
//! | 性质 | 三值权重 | f32 权重 |
//! |---|---|---|
//! | 一次编辑 | 离散、精确（`-1/0/+1` 三选一） | 连续值，编辑误差不可复现 |
//! | 判等 | 逐位相等 | 需要容差 |
//! | 回滚 | 存旧值即可逐位还原 | 需要快照 |
//! | 行为验证 | 有界域可**穷举**，给出完全等价 | 只能抽样/留出 |
//! | 规范形式 | `WSP1` 可 diff、可校验和 | 二进制 blob 不可读 |
//!
//! 代价同样明确：没有现成的 f32 预训练生态；表达力受三值限制；大规模网络的穷举
//! 不可行（本库的 M0 边界就是小网络 + 有界域）。
//!
//! ## 三层结构
//!
//! - **表示层**：[`Net`] / [`Layer`] 是不可变的权重工件；[`codec`] 把它写成 WSP1 文本，
//!   [`Trit`] 是唯一的权重单位。
//! - **编辑层**：[`Edit`] / [`Patch`] 是事务式补丁；[`Journal`] 支持逐位回滚；
//!   [`Program`] 把“前置行为 → 补丁 → 后置行为”打包成一次要么成功要么原样回滚的执行；
//!   [`Script`] 把多个 [`Program`] 组合成原子序列或按行为分支的脚本；
//!   [`AuditLog`] 把每次编辑记成带哈希链的 JSONL，可验链、可从初始工件重放；
//!   [`mod@shard`] 把长日志切成带 header 与分片内 oracle 表的多文件，跨片仍是同一条链；
//!   [`Checkpoint`] 可在分片边界保存工件与链头，从检查点续放；
//!   [`mod@rebase`] 把针对旧工件写的补丁三方重放到已经前进的工件上（冲突逐条报告）；
//!   [`mod@transplant`] 把一个隐藏单元（入边 + 偏置 + scale + 出边）整体搬到另一个网络。
//! - 程序语义：[`Program`] 把“前置行为 → 补丁 → 后置行为”打包成一次要么成功要么原样回滚的执行；
//! - 搜索层：[`mod@repair`] 不走梯度，在离散空间里找精确最小补丁；[`mod@analyze`] 对每个单点
//!   编辑**实测**失配变化并按收益排序（模型提议、库复核）；[`mod@explain`] 解释单个样本
//!   为什么错、哪些单点编辑能让它翻转/变对；[`mod@probe`] 做行级消融测行为漂移（大网读取）；
//!   [`mod@localize`] 在承重行内做 trit 级定位（行 → 权重两级读取）；
//!   [`merge`] 负责消掉隐藏单元置换对称性后再合并；[`mod@task`] 做三值任务向量投票合并；
//!   [`mod@dare`] 做 DARE 的三值对应物：按种子确定性丢弃差分 trit，重标度吸收进 scale。
//!
//! 规范自由度（不变量）：
//!
//! - 正 scale 不改行为，只改 f32 读数；
//! - 隐藏层行置换 + 下一层列置换不改行为；
//! - 因此行为判等必须用 [`Oracle`] 穷举，而不是比较权重。
//!
//! ## 快速开始
//!
//! 一个已经学会 XOR 的小网络被打坏一格；修补应只改一个三值：
//!
//! ```
//! use trit_wspace::{Edit, Layer, Net, Oracle, Patch, RepairOptions, Trit, repair};
//!
//! let net = Net::new(vec![
//!     Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).unwrap(),
//!     Layer::new(1, 2, &[-1, 1], &[-1]).unwrap(),
//! ]).unwrap();
//! let want = Oracle::boolean(2, |b| b[0] != b[1]).unwrap();
//! assert!(want.exact(&net).unwrap());
//!
//! let mut broken = net.clone();
//! Patch::new(vec![Edit::SetTrit { layer: 0, row: 0, col: 0, to: Trit::Zero }])
//!     .apply(&mut broken)
//!     .unwrap();
//! assert!(!want.exact(&broken).unwrap());
//!
//! let found = repair(&broken, &want, &RepairOptions::default()).unwrap().unwrap();
//! assert_eq!(found.patch.len(), 1);
//! let mut fixed = broken.clone();
//! found.patch.apply(&mut fixed).unwrap();
//! assert!(want.exact(&fixed).unwrap());
//! ```
//!
//! ## 边界（没做的部分）
//!
//! - 只支持符号激活的前馈网络；不带注意力、残差、卷积；
//! - 验证是**有界穷举**，不是形式化证明；超出域的输入没有保证；
//! - 修补搜索是启发式（半径 1/2 穷举 + 束搜索），不保证全局最小，只保证返回的解
//!   在验证集上完全正确；
//! - 没有“从自然语言生成补丁”的模型；本库提供的是可被外部程序/模型调用的
//!   编辑原语与验证闭环。
//!
//! 技术树与尚缺的层见 `crates/trit-wspace/README.md`。
//!
//! ## 模型/脚本接口
//!
//! 二进制 `trit-wspace` 提供两套入口，共用同一条判决路径（[`ops`]）：
//!
//! - CLI：`read / verify / patch / repair / diff / merge`，输入 `-` 读 stdin，报告一律 JSON；
//! - MCP：`trit-wspace mcp` 走 stdio JSON-RPC，暴露 `wsp_read`、`wsp_verify`、`wsp_patch`、
//!   `wsp_apply_candidate`、`wsp_repair`、`wsp_propose`、`wsp_explain`、`wsp_probe`、`wsp_localize`、
//!   `wsp_audit`、`wsp_diff`、`wsp_merge`、`wsp_shard`、`wsp_audit_shards`、`wsp_run_script`、
//!   `wsp_task_merge`、`wsp_rebase`、`wsp_transplant`、`wsp_dare_drop` 十九个工具，
//!   工具参数与返回格式见 [`wire`]。
//!
//! 补丁是稳定的 JSON 线格式（`version=1`），模型可以直接生成、人工可以逐条审。

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod analyze;
pub mod audit;
pub mod cli;
mod cli_audit;
pub mod codec;
pub mod dare;
pub mod edit;
pub mod error;
pub mod explain;
pub mod localize;
pub mod manifest;
pub mod mcp;
mod mcp_tools;
pub mod merge;
pub mod net;
pub mod ops;
pub mod oracle;
pub mod probe;
pub mod program;
pub mod rebase;
pub mod repair;
pub mod shard;
pub mod task;
pub mod transplant;
pub mod trit;
pub mod wire;

pub use analyze::{
    Analysis, EditImpact, PairAnalysis, PairImpact, TripleAnalysis, TripleImpact, analyze,
    analyze_pairs, analyze_triples,
};
pub use audit::{AuditLog, LogEntry, ReplayReport};
pub use codec::{checksum, from_wsp, to_wsp};
pub use dare::{DropOutcome, dare_drop, dare_drop_rows};
pub use edit::{Cost, Edit, Journal, Patch};
pub use error::{Error, Result};
pub use explain::{SampleEditImpact, SampleExplanation, explain_sample};
pub use localize::{
    LocalizeReport, LocalizeSelection, RowLocalization, TritImpact, localize, localize_row,
};
pub use manifest::{MANIFEST_VERSION, ManifestShard, ShardManifest, text_digest};
pub use merge::{align, merge_aligned, merge_vote};
pub use net::{Layer, Net};
pub use oracle::{Behavior, Oracle};
pub use probe::{
    ProbeReport, ProbeSelection, RowEffect, RowImpact, RowScore, probe_rows, probe_rows_adaptive,
};
pub use program::{Program, Report, Script, ScriptReport};
pub use repair::{Repair, RepairOptions, repair};
pub use shard::{Checkpoint, Shard, ShardCursor, ShardHeader, ShardReplay, ShardedLog};
pub use task::{TaskVector, merge_tasks, merge_tasks_dare, merge_tasks_per_task_dare};
pub use transplant::transplant_units;
pub use trit::Trit;
