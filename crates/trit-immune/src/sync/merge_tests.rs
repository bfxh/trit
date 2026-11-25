//! #620 / 审计 F39-2 回归：跨副本合并按副本标号归属命名空间，
//! 远端自有裁决不得按本地 id 错映到本地同号节点。

use super::*;
use crate::trit::Trit;

fn sd(writer: u32, phys: u64, counter: u32, delta: Delta) -> Stamped {
    Stamped {
        ts: Hlc::new(phys, counter, writer),
        delta,
    }
}

fn local_stream() -> Vec<Stamped> {
    vec![
        sd(
            0,
            1,
            0,
            Delta::Root {
                title: "Lroot".into(),
            },
        ),
        sd(
            0,
            2,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "la".into(),
            },
        ),
        sd(
            0,
            3,
            0,
            Delta::Record {
                id: NodeId(1),
                verdict: Trit::Vulnerable,
                evidence: None,
                note: "la-bad".into(),
            },
        ),
    ]
}

fn remote_stream() -> Vec<Stamped> {
    vec![
        sd(
            0,
            100,
            0,
            Delta::Root {
                title: "Rroot".into(),
            },
        ),
        sd(
            0,
            101,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "lb".into(),
            },
        ),
        sd(
            0,
            102,
            0,
            Delta::Record {
                id: NodeId(1),
                verdict: Trit::Hardened,
                evidence: None,
                note: "lb-good".into(),
            },
        ),
    ]
}

#[test]
fn remote_own_record_lands_on_remote_node_not_local_same_id() {
    let m = merge_replicas(vec![(0, local_stream()), (1, remote_stream())]);
    assert!(m.deferred.is_empty(), "{:?}", m.deferred);
    assert_eq!(m.dag.nodes().len(), 3, "根 + 两个各自的 spawn");

    let la = m.dag.nodes().iter().find(|n| n.title == "la").unwrap();
    let lb = m.dag.nodes().iter().find(|n| n.title == "lb").unwrap();
    // 本地 la 的 -1 不得被远端 +1 顶掉；远端 +1 必须落在 lb 自己身上。
    assert_eq!(la.verdict, Trit::Vulnerable);
    assert!(la.history.iter().any(|e| e.note == "la-bad"));
    assert!(!la.history.iter().any(|e| e.note == "lb-good"));
    assert_eq!(lb.verdict, Trit::Hardened);
    assert!(lb.history.iter().any(|e| e.note == "lb-good"));
}

#[test]
fn reverse_direction_remote_negative_does_not_flip_local_node() {
    let local = vec![
        sd(
            0,
            1,
            0,
            Delta::Root {
                title: "Lroot".into(),
            },
        ),
        sd(
            0,
            2,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "la".into(),
            },
        ),
        sd(
            0,
            3,
            0,
            Delta::Record {
                id: NodeId(1),
                verdict: Trit::Hardened,
                evidence: None,
                note: "la-good".into(),
            },
        ),
    ];
    let remote = vec![
        sd(
            0,
            200,
            0,
            Delta::Root {
                title: "Rroot".into(),
            },
        ),
        sd(
            0,
            201,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "lb".into(),
            },
        ),
        sd(
            0,
            202,
            0,
            Delta::Record {
                id: NodeId(1),
                verdict: Trit::Vulnerable,
                evidence: None,
                note: "lb-bad".into(),
            },
        ),
    ];
    let m = merge_replicas(vec![(0, local), (1, remote)]);
    assert!(m.deferred.is_empty(), "{:?}", m.deferred);
    let la = m.dag.nodes().iter().find(|n| n.title == "la").unwrap();
    let lb = m.dag.nodes().iter().find(|n| n.title == "lb").unwrap();
    assert_eq!(la.verdict, Trit::Hardened);
    assert_eq!(lb.verdict, Trit::Vulnerable);
}

#[test]
fn merge_is_order_independent_under_label_namespaces() {
    let forward = merge_replicas(vec![(0, local_stream()), (1, remote_stream())]);
    let swapped = merge_replicas(vec![(1, remote_stream()), (0, local_stream())]);
    for m in [forward, swapped] {
        let la = m.dag.nodes().iter().find(|n| n.title == "la").unwrap();
        let lb = m.dag.nodes().iter().find(|n| n.title == "lb").unwrap();
        assert_eq!(la.verdict, Trit::Vulnerable);
        assert_eq!(lb.verdict, Trit::Hardened);
    }
}
