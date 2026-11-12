//! probe 报告 → 攻击图的登记桥（从 probe.rs 拆出，纯搬家不改语义）。

use serde::{Deserialize, Serialize};

use super::{ProbeError, ProbeFinding};
use crate::netsec::attack::{AttackGraph, Evidence};

/// 登记桥的计数：这次登记保证了多少节点与边存在（幂等重跑数字不变）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AttachSummary {
    /// 主机节点数。
    pub hosts: usize,
    /// 服务节点数（每条 finding 一个）。
    pub services: usize,
    /// 登记的边数（station→host 每主机一条 + host→service 每探测一条）。
    pub edges: usize,
}

/// 从 findings JSONL（每行一个 [`ProbeFinding`]）复原清单；坏行显式报错。
pub fn findings_from_jsonl(text: &str) -> Result<Vec<ProbeFinding>, ProbeError> {
    let mut findings = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let finding: ProbeFinding = serde_json::from_str(line)
            .map_err(|error| ProbeError::Json(format!("第 {} 行：{error}", number + 1)))?;
        findings.push(finding);
    }
    Ok(findings)
}

/// probe 报告 → 攻击图的**半自动登记桥**。
///
/// 分工：桥机械地登记**探测证据**（出处直接用 finding 的 `source`，即
/// `tcp-connect`）；攻击语义（哪个利用、哪条路径）由人随后用 LWW 重新登记
/// 改写——同端点/同 label 后写覆盖是攻击图的既定纪律，桥不会锁死语义。
///
/// 结构（就这三层，不猜更多）：
///
/// - `station_label` 节点：`T`（probe 从这里发出，确定存在）；
/// - 每主机一个节点：label = 字面 IP，证据 = **主机应答**——任一探测拿到
///   `T` 或 `F` 就算主机在网（`F`/refused 也是主机回的 RST），全 `X` 才是
///   `X`（分不清 filtered 还是没这主机）；
/// - 每服务一个节点：label = `ip:port`，证据 = finding 的证据，备注 =
///   finding 的 note（banner 线索跟过去）；
/// - 边 station→host（应答证据）与 host→service（finding 证据），出处都是
///   `tcp-connect`。
///
/// 幂等：同 label 节点 LWW 覆盖、同端点边 LWW 覆盖 ⇒ 同一份报告重复登记图不变；
/// 换一份新报告重登记（端口状态变了）⇒ 节点与边都被刷新成最新证据。
pub fn attach_probe(
    graph: &mut AttackGraph,
    findings: &[ProbeFinding],
    station_label: &str,
) -> Result<AttachSummary, ProbeError> {
    let station = graph.add_node(station_label, Evidence::Confirmed, "probe 起点");
    let mut summary = AttachSummary::default();
    // 按主机分组，保持报告顺序。
    let mut groups: Vec<(String, Vec<&ProbeFinding>)> = Vec::new();
    for finding in findings {
        match groups.iter_mut().find(|(ip, _)| *ip == finding.ip) {
            Some((_, group)) => group.push(finding),
            None => groups.push((finding.ip.clone(), vec![finding])),
        }
    }
    for (ip, group) in &groups {
        let answered = group
            .iter()
            .any(|finding| finding.evidence != Evidence::Unverified);
        let host_evidence = if answered {
            Evidence::Confirmed
        } else {
            Evidence::Unverified
        };
        let note = format!("probe 探得 {} 项", group.len());
        let host = graph.add_node(ip, host_evidence, &note);
        graph.add_edge(station, host, host_evidence, "tcp-connect")?;
        summary.hosts += 1;
        summary.edges += 1;
        for finding in group {
            let service_label = format!("{}:{}", finding.ip, finding.port);
            let service = graph.add_node(&service_label, finding.evidence, &finding.note);
            graph.add_edge(host, service, finding.evidence, &finding.source)?;
            summary.services += 1;
            summary.edges += 1;
        }
    }
    Ok(summary)
}
