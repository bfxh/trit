//! MCP 工具清单（从 `mcp` 拆出，避免单文件超长）。

use serde_json::{Value, json};

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": props, "required": required},
    })
}

/// 十七个工具。改数量必须同时改测试断言。
#[must_use]
pub fn tools() -> Vec<Value> {
    let artifact =
        || json!({"type": "string", "description": "WSP1 文本（以 wsp1 开头）或文件路径"});
    vec![
        tool(
            "wsp_read",
            "读权重工件：arch / 权重数 / 校验和 / 规范化 WSP1",
            json!({"artifact": artifact(), "path": {"type": "string"}}),
            &[],
        ),
        tool(
            "wsp_verify",
            "在验证集上逐样本比对，返回失配见证（输入/期望/实际）",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object", "description": "inputs/outputs 或 domain/vars/table"},
                "limit": {"type": "integer", "description": "最多返回几条见证，默认 32"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_patch",
            "事务式应用补丁；可选前置/后置行为检查，失败原样回滚",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "patch": {"type": "object", "description": "version=1 的 edits 列表"},
                "oracle": {"type": "object"},
                "require_before": {"type": "boolean"},
                "require_after": {"type": "boolean"},
            }),
            &["patch"],
        ),
        tool(
            "wsp_apply_candidate",
            "直接把 proposal 候选应用为补丁；可选后置检查与追加审计日志（日志以文本返回，不写盘）",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "candidate": {"type": "object", "description": "wsp_propose 的 candidates[i]"},
                "oracle": {"type": "object"},
                "require_after": {"type": "boolean"},
                "log": {"type": "string", "description": "已有 JSONL 日志文本或只读路径；空串表示新日志"},
            }),
            &["candidate"],
        ),
        tool(
            "wsp_repair",
            "无梯度求精确补丁（小网络完备 BFS，大网络束搜索），返回补丁与新工件",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "max_edits": {"type": "integer"},
                "beam_width": {"type": "integer"},
                "exhaustive_states": {"type": "integer"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_propose",
            "实测单点/两编辑/三编辑组合的失配变化并按收益排序；不改工件",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "limit": {"type": "integer", "description": "返回条数，默认 16；0=全部"},
                "depth": {"type": "integer", "description": "1=单点，2=配对，3=三组合；默认 1"},
                "pairs": {"type": "boolean", "description": "等价于 depth=2（兼容旧调用）"},
                "max_pairs": {"type": "integer", "description": "配对预算，默认 20000"},
                "max_triples": {"type": "integer", "description": "三组合预算，默认 20000"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_explain",
            "解释单个样本：哪些单点编辑让它翻转/变对；先单样本，再 top-K 全量复核",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "sample": {"type": "integer", "description": "样本下标"},
                "top": {"type": "integer", "description": "复核条数，默认 16；0=全部"},
            }),
            &["oracle", "sample"],
        ),
        tool(
            "wsp_probe",
            "逐行清零/取反测行为漂移（大网读取）；budget 超了按步长均匀抽样并标注",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "budget": {"type": "integer", "description": "最多探多少行，0=全部"},
                "top": {"type": "integer", "description": "返回条数，默认 16；0=全部"},
                "adaptive": {"type": "boolean", "description": "按激活活跃度自适应选行（默认均匀步长）"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_localize",
            "两级定位：自适应选承重行，再在行内做 trit 级扫描",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "rows": {"type": "integer", "description": "选多少行，默认 3"},
                "row_budget": {"type": "integer", "description": "行探针预算，0=全部"},
                "max_trits": {"type": "integer", "description": "单行最多探多少位置，默认 64；0=全部"},
                "top_trits": {"type": "integer", "description": "每行返回条数，默认 8；0=全部"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_audit",
            "校验 JSONL 编辑日志的哈希链；可选从初始工件重放并检查最终校验和",
            json!({
                "log": {"type": "string", "description": "JSONL 文本或路径"},
                "artifact": artifact(), "path": {"type": "string"},
            }),
            &["log"],
        ),
        tool(
            "wsp_diff",
            "两个同架构工件的逐点差集（补丁 JSON + 改动量）",
            json!({"a": artifact(), "b": artifact()}),
            &["a", "b"],
        ),
        tool(
            "wsp_merge",
            "合并两个同架构工件；aligned=true 时先消隐藏单元置换",
            json!({"a": artifact(), "b": artifact(), "aligned": {"type": "boolean"}}),
            &["a", "b"],
        ),
        tool(
            "wsp_task_merge",
            "多任务三值向量合并：每个任务只对改过的坐标投票，平票保留基准",
            json!({
                "base": artifact(),
                "tasks": {"type": "array", "items": {"type": "string"}},
                "aligned": {"type": "boolean"},
            }),
            &["base", "tasks"],
        ),
        tool(
            "wsp_rebase",
            "把针对 base 写的补丁三方重放到 target；冲突逐条报告，默认整体放弃，skip_conflicts 只落干净部分",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "base": artifact(),
                "patch": {"type": "object", "description": "version=1 的 edits 列表"},
                "skip_conflicts": {"type": "boolean"},
                "oracle": {"type": "object"},
                "require_before": {"type": "boolean"},
                "require_after": {"type": "boolean"},
            }),
            &["base", "patch"],
        ),
        tool(
            "wsp_run_script",
            "执行组合脚本（原子序列 / 按行为条件分支），失败整体回滚",
            json!({
                "artifact": artifact(),
                "path": {"type": "string"},
                "script": {"type": "object", "description": "step / sequence / when 树"},
                "oracle": {"type": "object"},
            }),
            &["script", "oracle"],
        ),
        tool(
            "wsp_shard",
            "把 JSONL 审计日志切成带 header 与分片内 oracle 表的分片；可按 start/limit 分页取片",
            json!({
                "log": {"type": "string", "description": "JSONL 文本或路径"},
                "size": {"type": "integer", "description": "每片最多多少条"},
                "start": {"type": "integer", "description": "从第几片开始（默认 0）"},
                "limit": {"type": "integer", "description": "本次返回几片（0=全部）"},
            }),
            &["log", "size"],
        ),
        tool(
            "wsp_audit_shards",
            "校验分片审计日志的跨片哈希链；可选从初始工件/检查点重放，或输出检查点",
            json!({
                "shards": {"type": "array", "items": {"type": "string"}},
                "manifest": {"type": "object", "description": "wsp_shard 返回的 manifest"},
                "checkpoint": {"type": "object", "description": "从此检查点续放"},
                "checkpoint_every": {"type": "integer", "description": "每 N 片输出一个检查点"},
                "artifact": artifact(),
                "path": {"type": "string"},
            }),
            &["shards"],
        ),
    ]
}
