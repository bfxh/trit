//! 本地 id → 全局 id 改写（从 src/sync.rs 拆出，控制单文件行数预算）。

use super::*;
use crate::verdict::ExploitEvidence;

/// 把写者 `w` 本地命名空间里的节点引用改写成全局 id；共同前缀恒等映射。
pub(super) fn translate(
    own_local: &HashMap<u32, Vec<u32>>,
    own_global: &HashMap<u32, Vec<u32>>,
    w: u32,
    x: u32,
) -> Result<u32, String> {
    let (locals, globals) = match (own_local.get(&w), own_global.get(&w)) {
        (Some(l), Some(g)) => (l, g),
        _ => return Ok(x), // 未见其日志的写者：按共同前缀处理
    };
    match locals.binary_search(&x) {
        // 本地 id 已登记、但还没在全序里重放到（伪造的「引用先于 spawn」乱序戳）：
        // 按隔离处理，**绝不** `globals[i]` 越界——那会在 journal 写锁内 panic、毒死整个
        // 服务（H2：单请求永久 DoS）。
        Ok(i) => globals
            .get(i)
            .copied()
            .ok_or_else(|| format!("写者 #{w} 引用了尚未重放的本地节点 #{x:02}")),
        Err(0) => Ok(x), // 共同前缀
        Err(_) => Err(format!("写者 #{w} 引用了未知本地节点 #{x:02}")),
    }
}

pub(super) fn translate_delta(
    own_local: &HashMap<u32, Vec<u32>>,
    own_global: &HashMap<u32, Vec<u32>>,
    w: u32,
    delta: &Delta,
) -> Result<Delta, String> {
    Ok(match delta {
        Delta::Root { .. } | Delta::Spawn { .. } => delta.clone(),
        Delta::Record {
            id,
            verdict,
            evidence,
            note,
        } => Delta::Record {
            id: NodeId(translate(own_local, own_global, w, id.0)?),
            verdict: *verdict,
            evidence: match evidence {
                Some(e) => Some(ExploitEvidence {
                    vector: e.vector.clone(),
                    payload: e.payload.clone(),
                    target: NodeId(translate(own_local, own_global, w, e.target.0)?),
                    round: e.round,
                }),
                None => None,
            },
            note: note.clone(),
        },
        Delta::Quarantine { id, note } => Delta::Quarantine {
            id: NodeId(translate(own_local, own_global, w, id.0)?),
            note: note.clone(),
        },
        Delta::HumanReview { id, verdict, note } => Delta::HumanReview {
            id: NodeId(translate(own_local, own_global, w, id.0)?),
            verdict: *verdict,
            note: note.clone(),
        },
        Delta::CrossLink {
            from,
            to,
            pass,
            note,
        } => Delta::CrossLink {
            from: NodeId(translate(own_local, own_global, w, from.0)?),
            to: NodeId(translate(own_local, own_global, w, to.0)?),
            pass: *pass,
            note: note.clone(),
        },
    })
}
