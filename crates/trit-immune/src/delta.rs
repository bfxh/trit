//! Delta 编码：DAG 的每次变更都是一条一等公民操作，含文本编解码与应用校验。
//!
//! 实时写入与日志重放共用 `ImmuneDag::apply` 同一条代码路径；
//! 文本格式为**一行一条 Delta、制表符分字段、字符串转义**（`\\` `\t` `\n` `\r`），
//! 因此标题/Payload 可以安全携带空格、换行、引号与中文。

use crate::dag::{NodeId, NodeKind};
use crate::hlc::Hlc;
use crate::trit::Trit;
use crate::verdict::{AttackVector, ExploitEvidence};
use std::fmt;

/// DAG 的一次原子变更（追加式日志的记录单位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    /// 建立根节点（每个日志有且仅有一条，且必须是第一条）。
    Root { title: String },
    /// 零成本拉分支（开发增量 / 攻击向量 / 补丁 / 消息）。
    Spawn {
        parent: NodeId,
        kind: NodeKind,
        title: String,
    },
    /// 写入裁决（可携带负数消息回链证据）。
    Record {
        id: NodeId,
        verdict: Trit,
        evidence: Option<ExploitEvidence>,
        note: String,
    },
    /// 因果链隔离（Prompt 注入污染，沿子树传播）。
    Quarantine { id: NodeId, note: String },
    /// 人工裁决（解除隔离 + 落锤）。
    HumanReview {
        id: NodeId,
        verdict: Trit,
        note: String,
    },
    /// 跨树因果边（依赖 / 污染通道），参与三值传播。
    CrossLink {
        from: NodeId,
        to: NodeId,
        pass: Trit,
        note: String,
    },
}

/// 应用一条 Delta 时的结构校验错误（日志损坏 / 手工构造非法操作）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    RootAlreadyExists,
    UnknownParent {
        parent: NodeId,
    },
    UnknownNode {
        id: NodeId,
    },
    /// `ExploitEvidence.target` 指向不存在的节点。
    UnknownTarget {
        id: NodeId,
    },
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootAlreadyExists => write!(f, "根节点已存在（ROOT 只能有一条）"),
            Self::UnknownParent { parent } => write!(f, "父节点 #{:02} 不存在", parent.0),
            Self::UnknownNode { id } => write!(f, "节点 #{:02} 不存在", id.0),
            Self::UnknownTarget { id } => write!(f, "证据目标节点 #{:02} 不存在", id.0),
        }
    }
}

impl std::error::Error for ApplyError {}

/// 盖了时间戳的 Delta：journal v2 日志行的最小单位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped {
    pub ts: Hlc,
    pub delta: Delta,
}

/// 跨副本确定性归并：按 HLC 全序（物理→计数→写者）归并多份戳时间的 Delta 流。
/// 同写者戳严格递增 + 稳定排序 ⇒ 流内相对顺序保留，结果与归并输入顺序无关。
pub fn merge_order(streams: Vec<Vec<Stamped>>) -> Vec<Stamped> {
    let mut all: Vec<Stamped> = streams.into_iter().flatten().collect();
    all.sort_by_key(|a| a.ts);
    all
}

/// 编码为单行文本（不含换行符）。
pub fn encode(delta: &Delta) -> String {
    match delta {
        Delta::Root { title } => format!("ROOT\t{}", esc(title)),
        Delta::Spawn {
            parent,
            kind,
            title,
        } => {
            format!("SPAWN\t{}\t{}\t{}", parent.0, kind.as_str(), esc(title))
        }
        Delta::Record {
            id,
            verdict,
            evidence,
            note,
        } => {
            let (t, v, r, p) = match evidence {
                Some(e) => (
                    e.target.0.to_string(),
                    encode_vector(&e.vector),
                    e.round.to_string(),
                    esc(&e.payload),
                ),
                None => (
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                ),
            };
            format!(
                "RECORD\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                id.0,
                verdict,
                t,
                v,
                r,
                p,
                esc(note)
            )
        }
        Delta::Quarantine { id, note } => format!("QUARANTINE\t{}\t{}", id.0, esc(note)),
        Delta::HumanReview { id, verdict, note } => {
            format!("REVIEW\t{}\t{}\t{}", id.0, verdict, esc(note))
        }
        Delta::CrossLink {
            from,
            to,
            pass,
            note,
        } => format!("LINK\t{}\t{}\t{}\t{}", from.0, to.0, pass, esc(note)),
    }
}

/// 解析单行文本（journal 的一行）。
pub fn parse_line(line: &str) -> Result<Delta, String> {
    let mut f = line.split('\t');
    let d = match f.next().unwrap_or("") {
        "ROOT" => {
            let title = unesc(field(&mut f)?)?;
            Delta::Root { title }
        }
        "SPAWN" => {
            let parent = NodeId(num(field(&mut f)?)?);
            let kind = field(&mut f)?.parse::<NodeKind>()?;
            let title = unesc(field(&mut f)?)?;
            Delta::Spawn {
                parent,
                kind,
                title,
            }
        }
        "RECORD" => {
            let id = NodeId(num(field(&mut f)?)?);
            let verdict = field(&mut f)?.parse::<Trit>()?;
            let t = field(&mut f)?;
            let v = field(&mut f)?;
            let r = field(&mut f)?;
            let p = field(&mut f)?;
            let note = unesc(field(&mut f)?)?;
            let evidence = if t == "-" && v == "-" && r == "-" && p == "-" {
                None
            } else if t == "-" || v == "-" || r == "-" || p == "-" {
                return Err("证据字段部分缺失".into());
            } else {
                Some(ExploitEvidence {
                    target: NodeId(num(t)?),
                    vector: parse_vector(v)?,
                    round: num(r)?,
                    payload: unesc(p)?,
                })
            };
            Delta::Record {
                id,
                verdict,
                evidence,
                note,
            }
        }
        "QUARANTINE" => {
            let id = NodeId(num(field(&mut f)?)?);
            let note = unesc(field(&mut f)?)?;
            Delta::Quarantine { id, note }
        }
        "REVIEW" => {
            let id = NodeId(num(field(&mut f)?)?);
            let verdict = field(&mut f)?.parse::<Trit>()?;
            let note = unesc(field(&mut f)?)?;
            Delta::HumanReview { id, verdict, note }
        }
        "LINK" => {
            let from = NodeId(num(field(&mut f)?)?);
            let to = NodeId(num(field(&mut f)?)?);
            let pass = field(&mut f)?.parse::<Trit>()?;
            let note = unesc(field(&mut f)?)?;
            Delta::CrossLink {
                from,
                to,
                pass,
                note,
            }
        }
        other => return Err(format!("未知操作 {other:?}")),
    };
    if f.next().is_some() {
        return Err("多余字段".into());
    }
    Ok(d)
}

fn field<'a, I: Iterator<Item = &'a str>>(f: &mut I) -> Result<&'a str, String> {
    f.next().ok_or_else(|| "字段缺失".to_string())
}

fn num(s: &str) -> Result<u32, String> {
    s.parse::<u32>().map_err(|_| format!("非法编号 {s:?}"))
}

pub(crate) fn encode_vector(v: &AttackVector) -> String {
    match v {
        AttackVector::SqlInjection => "SQLI".into(),
        AttackVector::Xss => "XSS".into(),
        AttackVector::AuthBypass => "AUTH".into(),
        AttackVector::PromptInjection => "PROMPT".into(),
        AttackVector::FuzzCrash => "FUZZ".into(),
        AttackVector::Other(name) => format!("OTHER:{}", esc(name)),
    }
}

pub(crate) fn parse_vector(tok: &str) -> Result<AttackVector, String> {
    match tok {
        "SQLI" => Ok(AttackVector::SqlInjection),
        "XSS" => Ok(AttackVector::Xss),
        "AUTH" => Ok(AttackVector::AuthBypass),
        "PROMPT" => Ok(AttackVector::PromptInjection),
        "FUZZ" => Ok(AttackVector::FuzzCrash),
        t if t.starts_with("OTHER:") => unesc(&t["OTHER:".len()..]).map(AttackVector::Other),
        other => Err(format!("未知攻击向量 {other:?}")),
    }
}

/// 转义：`\` → `\\`，制表/换行/回车 → `\t` `\n` `\r`（Delta 与快照格式共用）。
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn unesc(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => return Err(format!("非法转义 \\{other}")),
            None => return Err("孤立的反斜杠".into()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_unesc_roundtrip() {
        for s in [
            "a b",
            "tab\there",
            "nl\nhere",
            "cr\rhere",
            "bs\\here",
            "引\"号",
            "中文✓",
            "",
            "混合\\t\n\r",
        ] {
            assert_eq!(unesc(&esc(s)).unwrap(), s);
        }
        assert!(unesc("孤\\x").is_err());
        assert!(unesc("尾部\\").is_err());
    }

    #[test]
    fn merge_order_sorts_by_hlc_and_keeps_stream_order() {
        use crate::hlc::Clock;
        let mk = |ts: Hlc, n: usize| Stamped {
            ts,
            delta: Delta::Root {
                title: format!("d{n}"),
            },
        };
        let fixed = |ms: u64| {
            let v = ms;
            Box::new(move || v) as Box<dyn Fn() -> u64 + Send + Sync>
        };
        let mut a = Clock::with_now(1, fixed(1000));
        let mut b = Clock::with_now(2, fixed(1000));
        let stream_a = vec![mk(a.tick(), 1), mk(a.tick(), 3)];
        let stream_b = vec![mk(b.tick(), 2), mk(b.tick(), 4)];

        let merged = merge_order(vec![stream_b, stream_a]);
        let writers: Vec<u32> = merged.iter().map(|s| s.ts.writer).collect();
        assert_eq!(writers, vec![1, 2, 1, 2], "按 (物理,计数,写者) 全序交错");
        let titles: Vec<String> = merged
            .iter()
            .map(|s| match &s.delta {
                Delta::Root { title } => title.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(titles, ["d1", "d2", "d3", "d4"], "流内相对顺序保留");
    }
}
