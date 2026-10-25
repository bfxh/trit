//! `topology` 子命令：三值拓扑传播与「通过 / 未定 / 否定」验证。
//!
//! 输入 JSON：`{"nodes": 3, "origin": 0, "edges": [{"from": 0, "to": 1, "value": "X"}]}`，
//! 边的 `value` 是单字符 `0` / `1` / `X`（`False` 边直接丢弃）。

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use trit_core::topology::{ReachEvidence, TernaryTopology, Verdict};
use trit_core::value::Ternary;

use crate::output::Format;

#[derive(Debug, Deserialize)]
struct TopologyInput {
    nodes: usize,
    #[serde(default)]
    origin: Option<usize>,
    #[serde(default)]
    edges: Vec<EdgeInput>,
}

#[derive(Debug, Deserialize)]
struct EdgeInput {
    from: usize,
    to: usize,
    value: String,
}

pub(crate) fn cmd_topology(
    path: &Path,
    format: Format,
    origin: Option<usize>,
    to: Option<usize>,
) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取拓扑文件失败：{}", path.display()))?;
    let input: TopologyInput = serde_json::from_str(&text)
        .with_context(|| format!("解析拓扑 JSON 失败：{}", path.display()))?;
    // 复审 M3：节点数直接来自外部 JSON，`vec![Vec::new(); nodes]` 会按声明值分配；
    // 没有上限时一个 {"nodes": 1000000000} 就能打爆内存。
    const MAX_NODES: usize = 100_000;
    if input.nodes > MAX_NODES {
        bail!("拓扑节点数 {} 超过上限 {MAX_NODES}", input.nodes);
    }
    let mut graph = TernaryTopology::new(input.nodes);
    for edge in &input.edges {
        let value = parse_value(&edge.value)?;
        graph
            .add_edge(edge.from, edge.to, value)
            .with_context(|| format!("边 {} -> {} 非法", edge.from, edge.to))?;
    }
    let origin = origin.or(input.origin).unwrap_or(0);
    if origin >= input.nodes {
        bail!("起点 {origin} 越界（共 {} 个节点）", input.nodes);
    }

    if let Some(target) = to {
        if target >= input.nodes {
            bail!("终点 {target} 越界（共 {} 个节点）", input.nodes);
        }
        let evidence = graph
            .explain(origin, target)
            .with_context(|| format!("判定 {origin} -> {target} 失败"))?;
        print_evidence(path, origin, target, &evidence, format);
        return Ok(());
    }

    let values = graph
        .reachability(origin)
        .with_context(|| format!("从 {origin} 传播失败"))?;
    let counts = |value: Ternary| values.iter().filter(|v| **v == value).count();
    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "file": path.display().to_string(),
                "nodes": graph.nodes(),
                "edges": graph.edges().len(),
                "origin": origin,
                "reachability": values.iter().map(|v| v.as_char().to_string()).collect::<Vec<_>>(),
                "summary": {
                    "proved": counts(Ternary::True),
                    "undetermined": counts(Ternary::Unknown),
                    "refuted": counts(Ternary::False),
                },
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
        _ => {
            println!(
                "三值拓扑：{}  [{} 节点 / {} 条边，起点 {origin}]",
                path.display(),
                graph.nodes(),
                graph.edges().len()
            );
            for (node, value) in values.iter().enumerate() {
                let label = match value {
                    Ternary::True => "通过（纯 True 路径可达）",
                    Ternary::Unknown => "未定（路径上有 Unknown 边）",
                    Ternary::False => "否定（不可达）",
                };
                println!("  {node} = {}  {label}", value.as_char());
            }
            println!(
                "\n汇总：通过 {} / 未定 {} / 否定 {}",
                counts(Ternary::True),
                counts(Ternary::Unknown),
                counts(Ternary::False)
            );
        }
    }
    Ok(())
}

fn print_evidence(
    path: &Path,
    origin: usize,
    target: usize,
    evidence: &ReachEvidence,
    format: Format,
) {
    let verdict = match evidence.verdict {
        Verdict::Proved => "proved",
        Verdict::Undetermined => "undetermined",
        Verdict::Refuted => "refuted",
    };
    match format {
        Format::Json => {
            let obj = serde_json::json!({
                "file": path.display().to_string(),
                "origin": origin,
                "target": target,
                "verdict": verdict,
                "value": evidence.value.as_char().to_string(),
                "path": evidence.path,
                "unknown_edges": evidence.unknown_edges
                    .iter()
                    .map(|(from, to)| serde_json::json!([from, to]))
                    .collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
        }
        _ => {
            let label = match evidence.verdict {
                Verdict::Proved => "通过：所有补全下都可达",
                Verdict::Undetermined => "未定：只有把路径上的 Unknown 边补成存在才可达",
                Verdict::Refuted => "否定：任何补全都不可达",
            };
            println!(
                "{origin} -> {target}：{label}（值 {}）",
                evidence.value.as_char()
            );
            if !evidence.path.is_empty() {
                let nodes: Vec<String> = evidence.path.iter().map(|n| n.to_string()).collect();
                println!("  证书路径：{}", nodes.join(" -> "));
            }
            if !evidence.unknown_edges.is_empty() {
                let edges: Vec<String> = evidence
                    .unknown_edges
                    .iter()
                    .map(|(from, to)| format!("{from}->{to}"))
                    .collect();
                println!("  需要补全的边：{}", edges.join(", "));
            }
        }
    }
}

fn parse_value(raw: &str) -> Result<Ternary> {
    let trimmed = raw.trim();
    let mut chars = trimmed.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ternary::parse_char(c)
            .ok_or_else(|| anyhow::anyhow!("未知边值 {raw:?}，只接受 0 / 1 / X")),
        _ => bail!("未知边值 {raw:?}，只接受 0 / 1 / X"),
    }
}
