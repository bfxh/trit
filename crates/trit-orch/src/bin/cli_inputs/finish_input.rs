use std::collections::BTreeMap;

use serde::Deserialize;
use trit_orch::{Receipt, RoundReport, Snapshot, TaskSpec, Ternary, TokenUsage};

/// 宿主亲自执行的一条验收命令。命令用 argv 表示，不经过 shell。
///
/// 这是默认完成闸唯一接受的 receipt 来源：`trit-orch finish` 自己 spawn 它、
/// 自己读退出状态、自己从标准输出解析断言计数。执行器（模型）无法直接填数字。
#[derive(Deserialize)]
pub(crate) struct Execution {
    pub(crate) task: String,
    /// `[program, arg, ...]`；program 必须在测试运行器白名单内。
    pub(crate) command: Vec<String>,
    /// 超时毫秒数；缺省 600000，上限 3600000。
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
    /// 宿主对本次执行所属 observation 的授权判定；缺失时 finish 拒绝。
    #[serde(default)]
    pub(crate) authorization: Option<Ternary>,
    /// 宿主对输入新鲜度的判定；缺失时 finish 拒绝。
    #[serde(default)]
    pub(crate) freshness: Option<Ternary>,
    /// 宿主记录的 token 用量；缺失时 finish 拒绝。
    #[serde(default)]
    pub(crate) usage: Option<TokenUsage>,
    /// 绑定到回执的测试工件（repo-relative）；宿主计算内容摘要写进 `test_digest`。
    #[serde(default)]
    pub(crate) test_files: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct FinishInput {
    pub(crate) tasks: Vec<TaskSpec>,
    pub(crate) snapshot: Snapshot,
    #[serde(default)]
    pub(crate) rounds: Vec<RoundReport>,
    /// 外部传入的回执。默认按 `SelfReported` 处理并拒绝；只有显式
    /// `--allow-self-reported` 才采信（用于外部已有独立信任锚的场景）。
    #[serde(default)]
    pub(crate) receipts: BTreeMap<String, Receipt>,
    /// 宿主亲自执行的验收命令；测量出的回执才是默认完成闸接受的证据。
    #[serde(default)]
    pub(crate) executions: Vec<Execution>,
    /// 宿主生成的挑战；给了就要求每条测量回执的 nonce 与它一致（防跨轮重放）。
    #[serde(default)]
    pub(crate) challenge: Option<String>,
    /// 全局验收下限：显式给出时只能抬高单个 `TaskSpec.min_assertions`，
    /// 不能压低；缺省时完全采用任务自己的阈值。
    #[serde(default)]
    pub(crate) min_assertions: Option<u32>,
    /// `claim_audit.py -Json` 的结论。提供时作为第二证据链：非绿报告拒绝完成。
    #[serde(default)]
    pub(crate) claim_audit: Option<ClaimAuditReport>,
}

/// `claim_audit.py -Json` 的顶层结构。只取完成闸需要的字段；未知字段忽略。
#[derive(Debug, Deserialize)]
pub(crate) struct ClaimAuditReport {
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) verdict: String,
    #[serde(default)]
    pub(crate) summary: ClaimAuditSummary,
    #[serde(default)]
    pub(crate) claims: Vec<ClaimAuditClaim>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ClaimAuditSummary {
    #[serde(default)]
    pub(crate) total: u32,
    #[serde(default)]
    pub(crate) verified: u32,
    #[serde(default)]
    pub(crate) unverified: u32,
    #[serde(default)]
    pub(crate) fabricated: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ClaimAuditClaim {
    pub(crate) claim: String,
    #[serde(default)]
    pub(crate) kind: String,
    pub(crate) verdict: String,
}
