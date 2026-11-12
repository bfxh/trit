//! sync 的单元测试（从 src/sync.rs 拆出，控制单文件行数预算）。

use super::*;
use crate::trit::Trit;

fn sd(writer: u32, phys: u64, counter: u32, delta: Delta) -> Stamped {
    Stamped {
        ts: Hlc::new(phys, counter, writer),
        delta,
    }
}

#[test]
fn independent_roots_are_deduped_and_namespaces_do_not_collide() {
    // 两个各自独立建库的副本：各自的 ROOT 戳不同
    let s0 = vec![
        sd(
            0,
            100,
            0,
            Delta::Root {
                title: "main".into(),
            },
        ),
        sd(
            0,
            101,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "A 模块".into(),
            },
        ),
        sd(
            0,
            102,
            0,
            Delta::Record {
                id: NodeId(1),
                verdict: Trit::Vulnerable,
                evidence: None,
                note: "A 打穿自己的模块".into(),
            },
        ),
    ];
    let s1 = vec![
        sd(
            1,
            200,
            0,
            Delta::Root {
                title: "main".into(),
            },
        ),
        sd(
            1,
            201,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "B 模块".into(),
            },
        ),
        sd(
            1,
            202,
            0,
            Delta::Record {
                id: NodeId(1),
                verdict: Trit::Hardened,
                evidence: None,
                note: "B 放行自己的模块".into(),
            },
        ),
    ];
    let m = merge_replicas(vec![(0, s0), (1, s1)]);
    assert_eq!(m.skipped_roots, 1, "第二个独立根被跳过");
    assert!(m.deferred.is_empty(), "{:?}", m.deferred);
    assert_eq!(m.dag.nodes().len(), 3, "根 + 两个各自的 spawn");
    // 两个写者的本地 #1 各自映射到自己的全局节点
    let a = m.dag.nodes().iter().find(|n| n.title == "A 模块").unwrap();
    let b = m.dag.nodes().iter().find(|n| n.title == "B 模块").unwrap();
    assert_eq!(a.verdict, Trit::Vulnerable);
    assert_eq!(b.verdict, Trit::Hardened);
}

#[test]
fn unknown_local_reference_is_deferred_not_applied() {
    let s0 = vec![sd(
        0,
        50,
        0,
        Delta::Root {
            title: "main".into(),
        },
    )];
    let s1 = vec![
        sd(
            1,
            100,
            0,
            Delta::Root {
                title: "main".into(),
            },
        ),
        sd(
            1,
            101,
            0,
            Delta::Record {
                id: NodeId(9),
                verdict: Trit::Hardened,
                evidence: None,
                note: "悬空引用".into(),
            },
        ),
    ];
    let m = merge_replicas(vec![(0, s0), (1, s1)]);
    assert_eq!(m.deferred.len(), 1, "悬空引用进隔离区，不静默丢弃");
    assert!(
        m.deferred[0].reason.contains("未知本地节点") || m.deferred[0].reason.contains("不存在")
    );
    assert_eq!(m.dag.nodes().len(), 1);
    assert!(m
        .canonical
        .iter()
        .all(|s| !matches!(s.delta, Delta::Record { .. })));
}

#[test]
fn frame_roundtrip() {
    let stamps = vec![
        sd(
            0,
            100,
            0,
            Delta::Root {
                title: "main\t带制表".into(),
            },
        ),
        sd(
            1,
            200,
            3,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Message,
                title: "标题\n含换行".into(),
            },
        ),
    ];
    let text = encode_frame(7, &stamps);
    let f = parse_frame(&text).unwrap();
    assert_eq!(f.writer, 7);
    assert_eq!(f.stamped, stamps);
    assert!(parse_frame("nope").is_err());
    assert!(parse_frame("trit-immune-log v1\n").is_err());
}

#[test]
fn frame_sign_verify_and_tamper() {
    let stamps = vec![sd(
        0,
        100,
        0,
        Delta::Root {
            title: "main".into(),
        },
    )];
    let unsigned = encode_frame(3, &stamps);
    let signed = sign_frame(&unsigned, b"k1").expect("sign");
    let f = verify_frame(&signed, b"k1").expect("verify");
    assert_eq!(f.writer, 3);
    assert_eq!(f.stamped, stamps);

    // 篡改帧体任意一行 → HMAC 拒绝
    let tampered = signed.replace("main", "mane");
    assert!(verify_frame(&tampered, b"k1").is_err(), "篡改必须被拒绝");
    // 错误密钥 → 拒绝
    assert!(verify_frame(&signed, b"k2").is_err());
    // 提供密钥即强制认证：缺 AUTH 行的无签帧直接拒绝
    assert!(verify_frame(&unsigned, b"k1").is_err());
    // 已签名的帧不能再签一次
    assert!(sign_frame(&signed, b"k1").is_err());
}

#[test]
fn record_referencing_a_not_yet_replayed_node_is_deferred_not_panicking() {
    // H2：伪造「引用先于 spawn」的乱序戳。RECORD@phys=1 引用本地 #1，但其 SPAWN@phys=5
    // 在全序里更晚重放 ⇒ translate 时 own_global 还没填到该下标。旧实现 `globals[i]`
    // 越界 panic（服务端在 journal 写锁内 ⇒ 锁中毒、整个服务永久不可用）；现在按隔离处理。
    let stream = vec![
        sd(
            7,
            0,
            0,
            Delta::Root {
                title: "main".into(),
            },
        ),
        sd(
            7,
            5,
            0,
            Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Delta,
                title: "x".into(),
            },
        ),
        sd(
            7,
            1,
            0,
            Delta::Record {
                id: NodeId(1),
                verdict: Trit::Vulnerable,
                evidence: None,
                note: "y".into(),
            },
        ),
    ];
    // 核心断言：不 panic（旧代码在此 `globals[0]` 越界）。
    let m = merge_replicas(vec![(7, stream)]);
    assert!(
        m.deferred.iter().any(|d| d.reason.contains("尚未重放")),
        "引用尚未重放的本地节点应进隔离区而非越界：{:?}",
        m.deferred
    );
}
