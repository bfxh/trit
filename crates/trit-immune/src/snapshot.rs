//! 快照：全量状态的一次性序列化，作为日志重放的**缓存**。
//!
//! - 只增日志永远不会被改写；快照是可有可无的 sidecar（`<journal>.snapshot`），
//!   损坏或不匹配时 [`Journal::open`](crate::Journal::open) 自动回退全量重放；
//! - 前缀校验用 FNV-1a（非加密哈希，只防意外损坏，不防恶意篡改——日志本身的
//!   完整性由追加式写入与 poison 语义保证）；
//! - 格式与 Delta 编码同风格：一行一记录、制表符分字段、字符串转义；
//!   `NODE` 行后跟其 `hist_len` 条 `EV` 行（完整审计历史逐条保留）。

use crate::dag::{CrossLink, ImmuneDag, Node, NodeId, NodeKind, VerdictEvent};
use crate::delta;
use crate::trit::Trit;
use crate::verdict::ExploitEvidence;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 快照文件头（第一行）。
pub const SNAPSHOT_HEADER: &str = "trit-immune-snapshot v1";

/// 快照覆盖范围：多少条 Delta、日志前缀多少字节、前缀 FNV 指纹。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotMeta {
    pub deltas_covered: usize,
    pub prefix_len: u64,
    pub prefix_fnv: u64,
}

pub(crate) fn sidecar_path(journal: &Path) -> PathBuf {
    journal.with_extension("snapshot")
}

pub(crate) fn fnv1a_bytes(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 序列化全量状态 + 覆盖范围元数据。
pub fn encode(dag: &ImmuneDag, meta: &SnapshotMeta) -> String {
    let mut out = String::new();
    out.push_str(SNAPSHOT_HEADER);
    out.push('\n');
    let _ = writeln!(
        out,
        "COVERED\t{}\t{}\t{:016x}",
        meta.deltas_covered, meta.prefix_len, meta.prefix_fnv
    );
    for node in dag.nodes() {
        let _ = writeln!(
            out,
            "NODE\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            node.id.0,
            node.parent
                .map(|p| p.0.to_string())
                .unwrap_or_else(|| "-".into()),
            node.kind.as_str(),
            delta::esc(&node.title),
            node.verdict,
            u8::from(node.quarantined),
            node.history.len(),
        );
        for ev in &node.history {
            let (t, v, r, p) = match &ev.evidence {
                Some(e) => (
                    e.target.0.to_string(),
                    delta::encode_vector(&e.vector),
                    e.round.to_string(),
                    delta::esc(&e.payload),
                ),
                None => (
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                ),
            };
            let _ = writeln!(
                out,
                "EV\t{}\t{}\t{}\t{}\t{}\t{}",
                ev.verdict,
                t,
                v,
                r,
                p,
                delta::esc(&ev.note)
            );
        }
    }
    for l in dag.links() {
        let _ = writeln!(
            out,
            "LINK\t{}\t{}\t{}\t{}",
            l.from.0,
            l.to.0,
            l.pass,
            delta::esc(&l.note)
        );
    }
    out
}

/// 解析快照；任何结构问题都返回 `Err`（调用方把它当缓存，回退全量重放）。
pub fn parse(text: &str) -> Result<(ImmuneDag, SnapshotMeta), String> {
    let mut lines = text.lines();
    match lines.next() {
        Some(h) if h.trim_end_matches('\r') == SNAPSHOT_HEADER => {}
        Some(other) => return Err(format!("头部不合法：{other:?}")),
        None => return Err("空文件".into()),
    }

    let covered_line = lines.next().ok_or("缺少 COVERED 行")?;
    let mut cf = covered_line.split('\t');
    let bad_covered = || "COVERED 行不合法".to_string();
    if cf.next() != Some("COVERED") {
        return Err(bad_covered());
    }
    let meta = SnapshotMeta {
        deltas_covered: cf
            .next()
            .ok_or_else(bad_covered)?
            .parse::<usize>()
            .map_err(|_| bad_covered())?,
        prefix_len: cf
            .next()
            .ok_or_else(bad_covered)?
            .parse::<u64>()
            .map_err(|_| bad_covered())?,
        prefix_fnv: u64::from_str_radix(cf.next().ok_or_else(bad_covered)?, 16)
            .map_err(|_| bad_covered())?,
    };
    if cf.next().is_some() {
        return Err(bad_covered());
    }

    let rest: Vec<&str> = lines.collect();
    let mut nodes: Vec<Node> = Vec::new();
    let mut links: Vec<CrossLink> = Vec::new();
    let mut i = 0usize;
    while i < rest.len() {
        let line_no = i + 3; // 头部 + COVERED 之后，1 起报错行号
        let line = rest[i];
        i += 1;
        if line.trim().is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        match f.next().unwrap_or("") {
            "NODE" => {
                let bad = || format!("第 {line_no} 行 NODE 不合法");
                let id = NodeId(
                    f.next()
                        .ok_or_else(bad)?
                        .parse::<u32>()
                        .map_err(|_| bad())?,
                );
                if id.0 as usize != nodes.len() {
                    return Err(format!(
                        "节点 id 不连续：期望 #{:02}，遇到 #{:02}",
                        nodes.len(),
                        id.0
                    ));
                }
                let parent = match f.next().ok_or_else(bad)? {
                    "-" => None,
                    s => Some(NodeId(s.parse::<u32>().map_err(|_| bad())?)),
                };
                let kind = f.next().ok_or_else(bad)?.parse::<NodeKind>()?;
                let title = delta::unesc(f.next().ok_or_else(bad)?)?;
                let verdict = f.next().ok_or_else(bad)?.parse::<Trit>()?;
                let quarantined = match f.next().ok_or_else(bad)? {
                    "0" => false,
                    "1" => true,
                    _ => return Err(bad()),
                };
                let hist_len = f
                    .next()
                    .ok_or_else(bad)?
                    .parse::<usize>()
                    .map_err(|_| bad())?;
                if f.next().is_some() {
                    return Err(bad());
                }
                let mut history = Vec::with_capacity(hist_len);
                for _ in 0..hist_len {
                    let ev_line = rest.get(i).ok_or("NODE 的历史条目不足")?;
                    i += 1;
                    history.push(parse_ev(i + 3, ev_line)?);
                }
                nodes.push(Node {
                    id,
                    parent,
                    kind,
                    title,
                    verdict,
                    quarantined,
                    history,
                });
            }
            "LINK" => {
                let bad = || format!("第 {line_no} 行 LINK 不合法");
                let from = NodeId(
                    f.next()
                        .ok_or_else(bad)?
                        .parse::<u32>()
                        .map_err(|_| bad())?,
                );
                let to = NodeId(
                    f.next()
                        .ok_or_else(bad)?
                        .parse::<u32>()
                        .map_err(|_| bad())?,
                );
                let pass = f.next().ok_or_else(bad)?.parse::<Trit>()?;
                let note = delta::unesc(f.next().ok_or_else(bad)?)?;
                if f.next().is_some() {
                    return Err(bad());
                }
                links.push(CrossLink {
                    from,
                    to,
                    pass,
                    note,
                });
            }
            other => return Err(format!("未知快照记录 {other:?}")),
        }
    }

    let dag = ImmuneDag::from_parts(nodes, links)?;
    Ok((dag, meta))
}

fn parse_ev(line_no: usize, line: &str) -> Result<VerdictEvent, String> {
    let mut f = line.split('\t');
    let bad = || format!("第 {line_no} 行 EV 不合法");
    if f.next() != Some("EV") {
        return Err(bad());
    }
    let verdict = f.next().ok_or_else(bad)?.parse::<Trit>()?;
    let t = f.next().ok_or_else(bad)?;
    let v = f.next().ok_or_else(bad)?;
    let r = f.next().ok_or_else(bad)?;
    let p = f.next().ok_or_else(bad)?;
    let note = delta::unesc(f.next().ok_or_else(bad)?)?;
    if f.next().is_some() {
        return Err(bad());
    }
    let evidence = if t == "-" && v == "-" && r == "-" && p == "-" {
        None
    } else if t == "-" || v == "-" || r == "-" || p == "-" {
        return Err("证据字段部分缺失".into());
    } else {
        Some(ExploitEvidence {
            target: NodeId(t.parse::<u32>().map_err(|_| bad())?),
            vector: delta::parse_vector(v)?,
            round: r.parse::<u32>().map_err(|_| bad())?,
            payload: delta::unesc(p)?,
        })
    };
    Ok(VerdictEvent {
        verdict,
        evidence,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AttackVector;

    #[test]
    fn encode_parse_roundtrip_with_nasty_strings() {
        let mut dag = ImmuneDag::root("main\t带制表");
        let n = dag.spawn(NodeId(0), NodeKind::Message, "标题\n含换行\\反斜");
        dag.record(
            n,
            Trit::Vulnerable,
            Some(ExploitEvidence {
                vector: AttackVector::Other("OTHER:像标签".into()),
                payload: "pay\nload".into(),
                target: NodeId(0),
                round: 7,
            }),
            "注\n意",
        );
        dag.quarantine(n, "隔\r离");
        let meta = SnapshotMeta {
            deltas_covered: 9,
            prefix_len: 123,
            prefix_fnv: 0xdead_beef,
        };
        let text = encode(&dag, &meta);
        // 单行承诺：EV 里的多行 payload/note 不能把记录劈开
        assert!(text.lines().all(|l| l.starts_with("trit-immune")
            || l.starts_with("COVERED")
            || l.starts_with("NODE")
            || l.starts_with("EV")
            || l.starts_with("LINK")));
        let (dag2, meta2) = parse(&text).unwrap();
        assert_eq!(dag2, dag);
        assert_eq!(meta2, meta);
    }
}
