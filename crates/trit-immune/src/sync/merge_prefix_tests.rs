//! #620 补充：克隆分叉（共有前缀 + 副本自有追加）下的恒等映射回归。

use super::*;
use crate::trit::Trit;

fn sd(writer: u32, phys: u64, counter: u32, delta: Delta) -> Stamped {
    Stamped {
        ts: Hlc::new(phys, counter, writer),
        delta,
    }
}

#[test]
fn cloned_prefix_spawn_stays_identity_mapped_for_remote_own_record() {
    // 克隆分叉场景：前缀根 + spawn 两份共有；副本 1 只追加一条自有 Record。
    // 前缀 spawn 归原作者副本，副本 1 的自有 Record 引用前缀 #1 必须恒等映射，
    // 不得进自有表后错映到尚未重放的槽位。
    let prefix_local = vec![
        sd(
            0,
            1,
            0,
            Delta::Root {
                title: "main".into(),
            },
        ),
        sd(
            0,
            2,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "前缀模块".into(),
            },
        ),
    ];
    let mut cloned = prefix_local.clone();
    cloned.push(sd(
        0,
        3,
        0,
        Delta::Record {
            id: NodeId(1),
            verdict: Trit::Hardened,
            evidence: None,
            note: "克隆侧放行".into(),
        },
    ));
    let m = merge_replicas(vec![(0, prefix_local), (1, cloned)]);
    assert!(m.deferred.is_empty(), "{:?}", m.deferred);
    let node = m
        .dag
        .nodes()
        .iter()
        .find(|n| n.title == "前缀模块")
        .unwrap();
    assert!(node.history.iter().any(|e| e.note == "克隆侧放行"));
}
