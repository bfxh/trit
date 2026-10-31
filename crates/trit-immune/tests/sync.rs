//! 跨副本同步：真实 journal 克隆 + 追加，合并收敛、顺序无关、LWW、隔离区。

use trit_immune::*;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("trit-immune-{name}-{}.journal", std::process::id()))
}

fn read_log(path: &std::path::Path) -> Vec<Stamped> {
    let raw = std::fs::read_to_string(path).unwrap();
    let mut out = Vec::new();
    for line in raw.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let (ts, d) = journal::parse_stamped_line(line).unwrap();
        out.push(Stamped { ts, delta: d });
    }
    out
}

#[test]
fn two_replicas_converge_regardless_of_merge_order() {
    let pa = temp("sync-a");
    let pb = temp("sync-b");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    // 副本 A（写者 0）：主干 + 登录模块 + 攻击分支
    let mut a = Journal::create_as(&pa, "main", 0).expect("create A");
    let login = a.spawn(NodeId(0), NodeKind::Delta, "登录").expect("spawn");
    let atk = a
        .spawn(login, NodeKind::AttackVector, "SQLi")
        .expect("spawn");
    a.record(atk, Trit::Vulnerable, None, "打穿").expect("record");
    a.into_dag().expect("flush");

    // 副本 B = 克隆 A 的日志文件，写者 1，继续追加自己的工作
    std::fs::copy(&pa, &pb).expect("克隆日志");
    let mut b = Journal::open_as(&pb, 1).expect("open B");
    let pay = b.spawn(NodeId(0), NodeKind::Delta, "支付").expect("spawn");
    b.record(pay, Trit::Hardened, None, "自身通过").expect("record");
    b.record(login, Trit::Hardened, None, "B 复验共享节点").expect("record");
    b.into_dag().expect("flush");

    // A 继续追加
    let mut a = Journal::open_as(&pa, 0).expect("reopen A");
    let docs = a.spawn(NodeId(0), NodeKind::Delta, "文档").expect("spawn");
    a.record(docs, Trit::Hardened, None, "A 的文档模块").expect("record");
    a.into_dag().expect("flush");

    let sa = read_log(&pa);
    let sb = read_log(&pb);
    let m1 = merge_replicas(vec![(0, sa.clone()), (1, sb.clone())]);
    let m2 = merge_replicas(vec![(1, sb), (0, sa)]);

    assert_eq!(m1.dag, m2.dag, "合并结果与输入顺序无关（确定性收敛）");
    assert!(m1.deferred.is_empty(), "克隆+追加模型下不应有隔离项：{:?}", m1.deferred);
    assert_eq!(m1.skipped_roots, 0, "克隆副本的根与原根同戳，去重即消");
    // 节点数 = 根 + A 的 3 个（登录/SQLi/文档）+ B 的 1 个（支付）
    assert_eq!(m1.dag.nodes().len(), 5);
    assert_eq!(m1.canonical.len(), 9, "去重后 union：6 + 3 条独立增量");

    // 共享节点复验落在正确的全局节点上（B 的裁决没有串到别人头上）
    let login_node = m1.dag.nodes().iter().find(|n| n.title == "登录").unwrap();
    assert_eq!(login_node.verdict, Trit::Hardened, "B 对共享节点的复验应生效");
    // 各自的新节点各得其所
    let pay = m1.dag.nodes().iter().find(|n| n.title == "支付").unwrap();
    assert_eq!(pay.verdict, Trit::Hardened);
    let atk_node = m1.dag.nodes().iter().find(|n| n.title == "SQLi").unwrap();
    assert_eq!(atk_node.verdict, Trit::Vulnerable);
    let docs = m1.dag.nodes().iter().find(|n| n.title == "文档").unwrap();
    assert_eq!(docs.verdict, Trit::Hardened);

    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn concurrent_verdicts_last_write_wins_keeps_both_events() {
    let pa = temp("sync-lww-a");
    let pb = temp("sync-lww-b");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    let mut a = Journal::create_as(&pa, "main", 0).expect("create A");
    let login = a.spawn(NodeId(0), NodeKind::Delta, "登录").expect("spawn");
    a.into_dag().expect("flush");

    std::fs::copy(&pa, &pb).expect("克隆日志");
    let mut b = Journal::open_as(&pb, 1).expect("open B");
    b.record(login, Trit::Hardened, None, "B 判安全").expect("record");
    b.into_dag().expect("flush");

    let mut a = Journal::open_as(&pa, 0).expect("reopen A");
    a.record(login, Trit::Vulnerable, None, "A 判漏洞").expect("record");
    a.into_dag().expect("flush");

    let sa = read_log(&pa);
    let sb = read_log(&pb);
    let m = merge_replicas(vec![(0, sa.clone()), (1, sb.clone())]);
    assert!(m.deferred.is_empty());

    let login_node = m.dag.nodes().iter().find(|n| n.title == "登录").unwrap();
    assert_eq!(login_node.history.len(), 2, "两条裁决都进历史，审计不丢");
    // 后到者胜：按 HLC 全序比较两条记录的戳
    let ts_a = sa
        .iter()
        .find(|s| matches!(&s.delta, Delta::Record { note, .. } if note == "A 判漏洞"))
        .map(|s| s.ts)
        .unwrap();
    let ts_b = sb
        .iter()
        .find(|s| matches!(&s.delta, Delta::Record { note, .. } if note == "B 判安全"))
        .map(|s| s.ts)
        .unwrap();
    let expected = if ts_a > ts_b { Trit::Vulnerable } else { Trit::Hardened };
    assert_eq!(
        login_node.verdict,
        expected,
        "后到者胜（ts_a={ts_a}, ts_b={ts_b}）"
    );

    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn three_replicas_converge() {
    let p0 = temp("sync-3-0");
    let p1 = temp("sync-3-1");
    let p2 = temp("sync-3-2");
    for p in [&p0, &p1, &p2] {
        let _ = std::fs::remove_file(p);
    }

    let mut a = Journal::create_as(&p0, "main", 0).expect("create");
    let base = a.spawn(NodeId(0), NodeKind::Delta, "基座").expect("spawn");
    a.into_dag().unwrap();

    std::fs::copy(&p0, &p1).unwrap();
    let mut b = Journal::open_as(&p1, 1).unwrap();
    let f1 = b.spawn(NodeId(0), NodeKind::Delta, "B 功能").unwrap();
    b.record(f1, Trit::Hardened, None, "B 通过").unwrap();
    b.into_dag().unwrap();

    std::fs::copy(&p0, &p2).unwrap();
    let mut c = Journal::open_as(&p2, 2).unwrap();
    let f2 = c.spawn(NodeId(0), NodeKind::Delta, "C 功能").unwrap();
    c.record(f2, Trit::Hardened, None, "C 通过").unwrap();
    c.record(base, Trit::Unknown, None, "C 对基座存疑").unwrap();
    c.into_dag().unwrap();

    let streams: Vec<(u32, Vec<Stamped>)> = [
        (0u32, read_log(&p0)),
        (1, read_log(&p1)),
        (2, read_log(&p2)),
    ]
    .into();

    let m1 = merge_replicas(streams.clone());
    let m2 = merge_replicas(streams.iter().rev().cloned().collect());
    assert_eq!(m1.dag, m2.dag, "三副本合并同样顺序无关");
    assert!(m1.deferred.is_empty(), "{:?}", m1.deferred);
    assert_eq!(m1.dag.nodes().len(), 4, "根 + 基座 + B 功能 + C 功能");

    // C 对共享基座的 0 态裁决生效（全序上它最后写）
    let base = m1.dag.nodes().iter().find(|n| n.title == "基座").unwrap();
    let ts_c = streams[2]
        .1
        .iter()
        .find(|s| matches!(&s.delta, Delta::Record { note, .. } if note == "C 对基座存疑"))
        .map(|s| s.ts)
        .unwrap();
    // 基座此前没有别的裁决 → C 的 0 就是终态
    assert_eq!(base.verdict, if ts_c.physical_ms > 0 { Trit::Unknown } else { Trit::Unknown });

    for p in [&p0, &p1, &p2] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn high_frequency_merge_six_replicas_is_order_independent() {
    // "AI 高频合并"压力场景：6 个写者各自高频产出，合并必须与输入顺序无关、
    // 裁决恰好归位（不串号）、幂等可重放。
    let mut streams: Vec<(u32, Vec<Stamped>)> = Vec::new();
    for w in 0..6u32 {
        let mut s = vec![Stamped {
            ts: Hlc::new(1000 + w as u64, 0, w),
            delta: Delta::Root {
                title: format!("main-{w}"),
            },
        }];
        for i in 0..25u32 {
            s.push(Stamped {
                ts: Hlc::new(2000 + w as u64, i, w),
                delta: Delta::Spawn {
                    parent: NodeId(0),
                    kind: NodeKind::Delta,
                    title: format!("w{w}-n{i:02}"),
                },
            });
        }
        for i in 0..25u32 {
            let verdict = match i % 3 {
                0 => Trit::Hardened,
                1 => Trit::Unknown,
                _ => Trit::Vulnerable,
            };
            s.push(Stamped {
                ts: Hlc::new(3000 + w as u64, i, w),
                delta: Delta::Record {
                    id: NodeId(1 + i),
                    verdict,
                    evidence: None,
                    note: format!("w{w} 裁决 {i}"),
                },
            });
        }
        streams.push((w, s));
    }

    let m0 = merge_replicas(streams.clone());
    assert_eq!(m0.dag.nodes().len(), 1 + 6 * 25, "根 + 6×25 个节点");
    assert!(m0.deferred.is_empty(), "{:?}", m0.deferred);

    // 5 种确定性洗牌输入顺序 → 全部收敛到同一 DAG
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for round in 0..5 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut shuffled = streams.clone();
        for i in (1..shuffled.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let j = (seed as usize) % (i + 1);
            shuffled.swap(i, j);
        }
        let m = merge_replicas(shuffled);
        assert_eq!(m.dag, m0.dag, "第 {round} 种洗牌顺序必须收敛到同一 DAG");
    }

    // 逐节点校验：每个节点恰好一次裁决，且值恰好归位（无串号）
    for n in m0.dag.nodes().iter().skip(1) {
        let t = n.title.trim_start_matches('w');
        let (w, i) = t.split_once("-n").unwrap();
        let _w: u32 = w.parse().unwrap();
        let i: u32 = i.parse().unwrap();
        let expect = match i % 3 {
            0 => Trit::Hardened,
            1 => Trit::Unknown,
            _ => Trit::Vulnerable,
        };
        assert_eq!(n.verdict, expect, "节点 {} 裁决串号", n.title);
        assert_eq!(n.history.len(), 1, "节点 {} 应恰好一次裁决", n.title);
    }
}

#[test]
fn anti_entropy_makes_files_identical_and_repeats() {
    let pa = temp("sync-ae-a");
    let pb = temp("sync-ae-b");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    // 副本 A 建库：登录 + SQLi 打穿
    let mut a = Journal::create_as(&pa, "main", 0).expect("create A");
    let login = a.spawn(NodeId(0), NodeKind::Delta, "登录").expect("spawn");
    let atk = a
        .spawn(login, NodeKind::AttackVector, "SQLi")
        .expect("spawn");
    a.record(atk, Trit::Vulnerable, None, "打穿").expect("record");
    a.into_dag().unwrap();

    // 副本 B = 克隆 + 自己的支付模块
    std::fs::copy(&pa, &pb).unwrap();
    let mut b = Journal::open_as(&pb, 1).unwrap();
    let pay = b.spawn(NodeId(0), NodeKind::Delta, "支付").unwrap();
    b.record(pay, Trit::Hardened, None, "通过").unwrap();
    b.into_dag().unwrap();

    // 第一轮反熵：两个文件逐字节一致
    let mut a = Journal::open_as(&pa, 0).unwrap();
    let mut b = Journal::open_as(&pb, 1).unwrap();
    let (ma, mb) = sync::anti_entropy(&mut a, &mut b).unwrap();
    assert_eq!(ma.dag, mb.dag);
    assert!(ma.deferred.is_empty() && mb.deferred.is_empty());
    assert_eq!(
        std::fs::read_to_string(&pa).unwrap(),
        std::fs::read_to_string(&pb).unwrap(),
        "反熵后双方日志应逐字节一致"
    );

    // 收敛后继续各自追加 → 第二轮反熵仍收敛，双方都拿到对方的新节点
    let t1 = a.spawn(NodeId(0), NodeKind::Delta, "遥测").unwrap();
    a.record(t1, Trit::Hardened, None, "A 通过").unwrap();
    let t2 = b.spawn(NodeId(0), NodeKind::Delta, "审计").unwrap();
    b.record(t2, Trit::Hardened, None, "B 通过").unwrap();
    sync::anti_entropy(&mut a, &mut b).unwrap();
    assert_eq!(
        std::fs::read_to_string(&pa).unwrap(),
        std::fs::read_to_string(&pb).unwrap(),
        "第二轮反熵后仍逐字节一致"
    );

    let da = a.into_dag().unwrap();
    assert_eq!(da.nodes().len(), 6, "根+登录+SQLi+支付+遥测+审计");
    let teal = da.nodes().iter().find(|n| n.title == "遥测").unwrap();
    let audit = da.nodes().iter().find(|n| n.title == "审计").unwrap();
    assert_eq!(teal.verdict, Trit::Hardened);
    assert_eq!(audit.verdict, Trit::Hardened);

    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn tcp_roundtrip_converges() {
    let pa = temp("sync-tcp-a");
    let pb = temp("sync-tcp-b");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    let mut a = Journal::create_as(&pa, "main", 0).expect("create A");
    let login = a.spawn(NodeId(0), NodeKind::Delta, "登录").expect("spawn");
    a.record(login, Trit::Hardened, None, "A 通过").expect("record");
    a.into_dag().unwrap();

    std::fs::copy(&pa, &pb).unwrap();
    let mut b = Journal::open_as(&pb, 1).unwrap();
    let pay = b.spawn(NodeId(0), NodeKind::Delta, "支付").unwrap();
    b.record(pay, Trit::Hardened, None, "B 通过").unwrap();
    b.into_dag().unwrap();

    // B 作为服务端（真实 TCP，回环地址 + 临时端口）
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let pb_for_thread = pb.clone();
    let handle = std::thread::spawn(move || {
        let mut b = Journal::open_as(&pb_for_thread, 1).expect("open B");
        sync::tcp::serve_merge(listener, &mut b).expect("serve");
        b.into_dag().expect("flush B").nodes().len()
    });

    // A 作为客户端：发日志、采纳规范日志
    let mut a = Journal::open_as(&pa, 0).unwrap();
    let adopted = sync::tcp::pull_merge(addr, &mut a).expect("pull_merge");
    assert!(adopted > 0);
    let a_nodes = a.into_dag().expect("flush A").nodes().len();
    let b_nodes = handle.join().expect("serve 线程");
    assert_eq!(a_nodes, b_nodes, "双方节点数一致");
    assert_eq!(
        std::fs::read_to_string(&pa).unwrap(),
        std::fs::read_to_string(&pb).unwrap(),
        "TCP 往返后双方日志逐字节一致"
    );

    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn tcp_authenticated_roundtrip_converges() {
    const PSK: &[u8] = b"psk-2026-trit-immune";
    let pa = temp("sync-tcpauth-a");
    let pb = temp("sync-tcpauth-b");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    let mut a = Journal::create_as(&pa, "main", 0).unwrap();
    let login = a.spawn(NodeId(0), NodeKind::Delta, "登录").unwrap();
    a.record(login, Trit::Hardened, None, "A 通过").unwrap();
    a.into_dag().unwrap();

    std::fs::copy(&pa, &pb).unwrap();
    let mut b = Journal::open_as(&pb, 1).unwrap();
    let pay = b.spawn(NodeId(0), NodeKind::Delta, "支付").unwrap();
    b.record(pay, Trit::Hardened, None, "B 通过").unwrap();
    b.into_dag().unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let pb_for_thread = pb.clone();
    let handle = std::thread::spawn(move || {
        let mut b = Journal::open_as(&pb_for_thread, 1).unwrap();
        sync::tcp::serve_merge_with(listener, &mut b, Some(PSK)).expect("serve");
        b.into_dag().unwrap()
    });
    let mut a = Journal::open_as(&pa, 0).unwrap();
    sync::tcp::pull_merge_with(addr, &mut a, Some(PSK)).expect("pull");
    handle.join().expect("serve 线程");
    assert_eq!(
        std::fs::read_to_string(&pa).unwrap(),
        std::fs::read_to_string(&pb).unwrap(),
        "认证通道收敛后日志逐字节一致"
    );

    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn tcp_with_wrong_psk_is_rejected_both_ways() {
    const K1: &[u8] = b"key-one";
    const K2: &[u8] = b"key-two";
    let pa = temp("sync-tcpbad-a");
    let pb = temp("sync-tcpbad-b");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    let mut a = Journal::create_as(&pa, "main", 0).unwrap();
    let f = a.spawn(NodeId(0), NodeKind::Delta, "功能").unwrap();
    a.record(f, Trit::Hardened, None, "通过").unwrap();
    a.into_dag().unwrap();
    std::fs::copy(&pa, &pb).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let pb_for_thread = pb.clone();
    let handle = std::thread::spawn(move || {
        let mut b = Journal::open_as(&pb_for_thread, 1).unwrap();
        sync::tcp::serve_merge_with(listener, &mut b, Some(K2)).is_err()
    });
    let mut a = Journal::open_as(&pa, 0).unwrap();
    assert!(
        sync::tcp::pull_merge_with(addr, &mut a, Some(K1)).is_err(),
        "客户端应因验证失败/连接中断而出错"
    );
    assert!(handle.join().unwrap(), "服务端应拒绝错钥帧");
    // 双方文件都没被污染：B 未采纳任何东西
    let b_log = std::fs::read_to_string(&pb).unwrap();
    assert_eq!(b_log.lines().count(), 4, "B 的日志保持克隆时的 3 条 Delta + 头");

    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}
