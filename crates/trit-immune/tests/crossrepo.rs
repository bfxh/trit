#![cfg(feature = "full")]
//! 跨仓 Delta 完整协议测试：`Fleet::sync_cross_repo`。
//!
//! 语义纪律（每条一个用例）：
//! 1. 坏消息落账并持久化（目标仓独立重开仍可见，门禁收紧）；
//! 2. `+1` 永不跨仓落账（下游的 +1 必须自己挣）；已证拦截边不落坏消息；
//! 3. 幂等：反复跑反熵，第二次起零落账；
//! 4. 不覆盖：目标已更坏时跳过（单调，不洗白）；
//! 5. 落账与查询无矛盾：落账后 `effective_verdicts` 与账目一致；
//! 6. 未知仓/越界节点如实跳过不炸。

use trit_immune::fleet::CrossRepoDelta;
use trit_immune::*;

fn tmp(tag: &str, n: u64) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "trit-cross-{tag}-{}-{n}.journal",
        std::process::id()
    ))
}

/// 建一个最小仓：根 + 一个业务节点（指定裁决），落盘关闭。
fn make_repo(path: &std::path::Path, writer: u32, title: &str, verdict: Trit) -> Journal {
    let _ = std::fs::remove_file(path);
    let mut j = Journal::create_as(path, title, writer).unwrap();
    let n = j.spawn(NodeId(0), NodeKind::Delta, title).unwrap();
    j.record(n, verdict, None, "初始").unwrap();
    drop(j.into_dag().unwrap());
    Journal::open_as(path, writer).unwrap()
}

#[test]
fn negative_lands_persists_and_tightens_gate() {
    let (pa, pb) = (tmp("a", 1), tmp("b", 1));
    let mut fleet = Fleet::new();
    fleet
        .add("up", make_repo(&pa, 0, "上游库", Trit::Vulnerable))
        .unwrap();
    fleet
        .add("down", make_repo(&pb, 1, "下游应用", Trit::Hardened))
        .unwrap();
    fleet
        .add_cross_link(
            ("up", NodeId(1)),
            ("down", NodeId(1)),
            Trit::Vulnerable,
            "确认穿透",
        )
        .unwrap();

    // 反熵：-1 沿边落账
    let landed: Vec<CrossRepoDelta> = fleet.sync_cross_repo("跨仓反熵").unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].verdict, Trit::Vulnerable);
    assert_eq!(landed[0].from, ("up".to_string(), NodeId(1)));
    assert_eq!(landed[0].to, ("down".to_string(), NodeId(1)));

    // 落账是目标仓自己的 Delta：独立重开仍可见（持久化），带溯源 note
    drop(fleet);
    let j2 = Journal::open_as(&pb, 1).unwrap();
    let node = j2.dag().node(NodeId(1));
    assert_eq!(node.verdict, Trit::Vulnerable, "跨仓 -1 已入账");
    assert!(
        node.history.iter().any(|e| e.note.contains("up:1")),
        "溯源 note 应指向源仓"
    );
    drop(j2);
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn plus_one_never_lands_across_repos() {
    let (pa, pb) = (tmp("a", 2), tmp("b", 2));
    let mut fleet = Fleet::new();
    // 源通过 + 穿透边：max(+1, -1) = +1 → 不落
    fleet
        .add("up", make_repo(&pa, 0, "上游", Trit::Hardened))
        .unwrap();
    fleet
        .add("down", make_repo(&pb, 1, "下游", Trit::Unknown))
        .unwrap();
    fleet
        .add_cross_link(
            ("up", NodeId(1)),
            ("down", NodeId(1)),
            Trit::Vulnerable,
            "穿透边",
        )
        .unwrap();
    let landed = fleet.sync_cross_repo("反熵").unwrap();
    assert!(landed.is_empty(), "源 +1 不得给下游 +1，实际 {landed:?}");
    assert_eq!(
        fleet.repo_verdict("down", NodeId(1)).unwrap(),
        Trit::Unknown
    );

    // 拦截边：已证拦截 → 坏消息也过不来
    let (pc, pd) = (tmp("c", 2), tmp("d", 2));
    fleet
        .add("up2", make_repo(&pc, 2, "上游2", Trit::Vulnerable))
        .unwrap();
    fleet
        .add("down2", make_repo(&pd, 3, "下游2", Trit::Unknown))
        .unwrap();
    fleet
        .add_cross_link(
            ("up2", NodeId(1)),
            ("down2", NodeId(1)),
            Trit::Hardened,
            "已证拦截",
        )
        .unwrap();
    let landed2 = fleet.sync_cross_repo("反熵").unwrap();
    assert!(landed2.is_empty(), "拦截边不落坏消息，实际 {landed2:?}");
    drop(fleet);
    for p in [&pa, &pb, &pc, &pd] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn sync_is_idempotent_and_never_whitewashes() {
    let (pa, pb) = (tmp("a", 3), tmp("b", 3));
    let mut fleet = Fleet::new();
    fleet
        .add("up", make_repo(&pa, 0, "上游", Trit::Vulnerable))
        .unwrap();
    fleet
        .add("down", make_repo(&pb, 1, "下游", Trit::Hardened))
        .unwrap();
    fleet
        .add_cross_link(
            ("up", NodeId(1)),
            ("down", NodeId(1)),
            Trit::Vulnerable,
            "穿透",
        )
        .unwrap();

    assert_eq!(fleet.sync_cross_repo("第一次").unwrap().len(), 1);
    assert!(fleet.sync_cross_repo("第二次").unwrap().is_empty(), "幂等");
    assert!(fleet.sync_cross_repo("第三次").unwrap().is_empty(), "幂等");

    // 不覆盖：目标已是 -1（更坏/相等），上游可疑 0 不得洗白也不得重复落
    fleet
        .record_repo("up", NodeId(1), Trit::Unknown, "上游复查降级")
        .unwrap();
    let landed = fleet.sync_cross_repo("降级传播").unwrap();
    assert!(landed.is_empty(), "max(0,-1)=0 ≥ 目标-1，不落不洗白");
    drop(fleet);
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn ledger_agrees_with_query_semantics() {
    let (pa, pb) = (tmp("a", 4), tmp("b", 4));
    let mut fleet = Fleet::new();
    fleet
        .add("up", make_repo(&pa, 0, "上游", Trit::Vulnerable))
        .unwrap();
    fleet
        .add("down", make_repo(&pb, 1, "下游", Trit::Hardened))
        .unwrap();
    fleet
        .add_cross_link(
            ("up", NodeId(1)),
            ("down", NodeId(1)),
            Trit::Vulnerable,
            "穿透",
        )
        .unwrap();

    // 落账前：查询时传播已显示污染（视图）
    assert_eq!(
        fleet.effective_verdicts("down").unwrap()[1],
        Trit::Vulnerable
    );
    // 落账后：账目与查询一致（事实 = 视图，无矛盾）
    fleet.sync_cross_repo("反熵").unwrap();
    assert_eq!(
        fleet.effective_verdicts("down").unwrap()[1],
        Trit::Vulnerable
    );
    drop(fleet);
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn unknown_repos_and_oob_nodes_are_skipped() {
    let (pa, pb) = (tmp("a", 5), tmp("b", 5));
    let mut fleet = Fleet::new();
    fleet
        .add("up", make_repo(&pa, 0, "上游", Trit::Vulnerable))
        .unwrap();
    fleet
        .add("down", make_repo(&pb, 1, "下游", Trit::Hardened))
        .unwrap();

    // 非法边从注册表加载进来（加载期不验证——与仓状态解耦的真实形态）：
    // 幽灵源仓 / 源节点越界 / 目标节点越界
    let ledger = std::env::temp_dir().join(format!("trit-cross-ledger-{}.tsv", std::process::id()));
    std::fs::write(
        &ledger,
        "FLEETLINK\tghost\t1\tdown\t1\t-1\t幽灵源\n\
         FLEETLINK\tup\t99\tdown\t1\t-1\t源节点越界\n\
         FLEETLINK\tup\t1\tdown\t99\t-1\t目标越界\n",
    )
    .unwrap();
    let (loaded, skipped) = fleet.load_links(&ledger).unwrap();
    assert_eq!((loaded, skipped), (3, 0), "三条边都加载（不验证）");

    // 反熵期如实跳过：不炸、不落账
    let landed = fleet.sync_cross_repo("反熵").unwrap();
    assert!(landed.is_empty(), "非法边全部跳过，实际 {landed:?}");
    drop(fleet);
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
    let _ = std::fs::remove_file(&ledger);
}

// ── 多跳攻击链枚举（fleet::attack_chains） ───────────────────────────────

use trit_immune::fleet::{ChainStep, ChainVia};

/// 建一个链式仓：ROOT ← N1 ← N2 ← …，每节点裁决按 verdicts 对齐。
fn chain_repo(path: &std::path::Path, writer: u32, verdicts: &[Trit]) -> Journal {
    let _ = std::fs::remove_file(path);
    let mut j = Journal::create_as(path, "chain", writer).unwrap();
    let mut parent = NodeId(0);
    for (i, v) in verdicts.iter().enumerate() {
        let id = j.spawn(parent, NodeKind::Delta, &format!("N{i}")).unwrap();
        j.record(id, *v, None, "链上节点").unwrap();
        parent = id;
    }
    drop(j.into_dag().unwrap());
    Journal::open_as(path, writer).unwrap()
}

#[test]
fn attack_chain_within_single_repo() {
    let p = tmp("chain1", 1);
    // A(-1) ← B(0) ← C(0)，target = 尾节点 C
    let mut fleet = Fleet::new();
    fleet
        .add(
            "r",
            chain_repo(&p, 0, &[Trit::Vulnerable, Trit::Unknown, Trit::Unknown]),
        )
        .unwrap();

    let tail = NodeId(3);
    let res = fleet.attack_chains("r", tail, 64, 8);
    assert!(!res.truncated);
    assert_eq!(res.chains.len(), 1, "恰一条链：{res:?}");
    let steps = &res.chains[0].steps;
    // 源 → … → target，方向即攻击方向
    assert_eq!(steps[0].node, ("r".to_string(), NodeId(1)), "源 = 确认 -1");
    assert_eq!(steps[0].via, ChainVia::Source);
    // 中段与 target 的进入边全 Causal（进入语义：每步标注从上一跳流入的边）
    assert!(steps[1..]
        .iter()
        .all(|s: &ChainStep| s.via == ChainVia::Causal));
    assert_eq!(steps.last().unwrap().node, ("r".to_string(), tail));

    // 全 +1 链：无打穿路径
    let p2 = tmp("chain1b", 1);
    let mut fleet2 = Fleet::new();
    fleet2
        .add("r", chain_repo(&p2, 1, &[Trit::Hardened; 3]))
        .unwrap();
    let res2 = fleet2.attack_chains("r", NodeId(3), 64, 8);
    assert!(res2.chains.is_empty(), "全 +1 无链");
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(&p2);
}

#[test]
fn attack_chain_across_repos_and_blocked_edges() {
    let (pa, pb, pc) = (tmp("ac-a", 2), tmp("ac-b", 2), tmp("ac-c", 2));
    let mut fleet = Fleet::new();
    // 源仓：S(-1)；目标仓：T(0) ← U(+1)——T 可疑被穿透边拉黑，target = U
    fleet
        .add("src", chain_repo(&pa, 0, &[Trit::Vulnerable]))
        .unwrap();
    fleet
        .add("dst", chain_repo(&pb, 1, &[Trit::Unknown, Trit::Hardened]))
        .unwrap();
    fleet
        .add_cross_link(
            ("src", NodeId(1)),
            ("dst", NodeId(1)),
            Trit::Vulnerable,
            "确认穿透",
        )
        .unwrap();
    let res = fleet.attack_chains("dst", NodeId(2), 64, 8);
    assert_eq!(res.chains.len(), 1, "跨仓链：{res:?}");
    let steps = &res.chains[0].steps;
    assert_eq!(steps[0].node, ("src".to_string(), NodeId(1)), "源在上游仓");
    assert_eq!(steps[0].via, ChainVia::Source);
    assert_eq!(steps[1].via, ChainVia::CrossLink, "跨仓穿透边");
    assert_eq!(steps[1].node, ("dst".to_string(), NodeId(1)));
    assert_eq!(
        steps.last().unwrap().node,
        ("dst".to_string(), NodeId(2)),
        "终点 = target"
    );

    // 拦截边（pass=+1）：不构成打穿路径
    let (pc1, pc2) = (tmp("ac-x", 2), tmp("ac-y", 2));
    let mut fleet2 = Fleet::new();
    fleet2
        .add("src", chain_repo(&pc1, 2, &[Trit::Vulnerable]))
        .unwrap();
    fleet2
        .add("dst", chain_repo(&pc2, 3, &[Trit::Hardened]))
        .unwrap();
    fleet2
        .add_cross_link(
            ("src", NodeId(1)),
            ("dst", NodeId(1)),
            Trit::Hardened,
            "已证拦截",
        )
        .unwrap();
    let res2 = fleet2.attack_chains("dst", NodeId(1), 64, 8);
    assert!(res2.chains.is_empty(), "拦截边无链");

    // 可疑 0 源经可疑边：不 emit（0 不算确认打穿）
    let (pd, pe) = (tmp("ac-m", 2), tmp("ac-n", 2));
    let mut fleet3 = Fleet::new();
    fleet3
        .add("src", chain_repo(&pd, 4, &[Trit::Unknown]))
        .unwrap();
    fleet3
        .add("dst", chain_repo(&pe, 5, &[Trit::Unknown]))
        .unwrap();
    fleet3
        .add_cross_link(
            ("src", NodeId(1)),
            ("dst", NodeId(1)),
            Trit::Unknown,
            "可疑依赖",
        )
        .unwrap();
    let res3 = fleet3.attack_chains("dst", NodeId(1), 64, 8);
    assert!(res3.chains.is_empty(), "可疑不算打穿");
    for p in [&pa, &pb, &pc, &pc1, &pc2, &pd, &pe] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn attack_chain_depth_truncation_reports_honestly() {
    let (pa, pb) = (tmp("tr-a", 6), tmp("tr-b", 6));
    let mut fleet = Fleet::new();
    // 深链：r 仓 5 节点，头部 -1
    fleet
        .add(
            "r",
            chain_repo(
                &pa,
                0,
                &[
                    Trit::Vulnerable,
                    Trit::Unknown,
                    Trit::Unknown,
                    Trit::Unknown,
                    Trit::Unknown,
                ],
            ),
        )
        .unwrap();
    // 深度 2：到不了源 → 空 + truncated 如实报告
    let res = fleet.attack_chains("r", NodeId(5), 64, 2);
    assert!(res.truncated, "深度不足必须如实截断");

    // 深度足够 → 完整链
    let res2 = fleet.attack_chains("r", NodeId(5), 64, 8);
    assert!(!res2.truncated);
    assert_eq!(res2.chains.len(), 1);
    assert_eq!(res2.chains[0].steps.len(), 5, "源 + 3 中间 + target");

    // 条数上限：两仓各一条链，max_chains=1 → 1 条 + truncated
    fleet
        .add(
            "r2",
            chain_repo(&pb, 1, &[Trit::Vulnerable, Trit::Unknown, Trit::Unknown]),
        )
        .unwrap();
    fleet
        .add_cross_link(
            ("r2", NodeId(1)),
            ("r", NodeId(5)),
            Trit::Vulnerable,
            "第二条链",
        )
        .unwrap();
    let res3 = fleet.attack_chains("r", NodeId(5), 1, 8);
    assert_eq!(res3.chains.len(), 1);
    assert!(res3.truncated, "条数截断如实报告");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

#[test]
fn attack_chain_cycle_terminates() {
    let (pa, pb) = (tmp("cy-a", 7), tmp("cy-b", 7));
    let mut fleet = Fleet::new();
    fleet
        .add("a", chain_repo(&pa, 0, &[Trit::Vulnerable]))
        .unwrap();
    fleet
        .add("b", chain_repo(&pb, 1, &[Trit::Unknown]))
        .unwrap();
    // 成环：a#1 → b#1 → a#1（加载期不验证的注册表形态）
    let ledger = std::env::temp_dir().join(format!("trit-cy-ledger-{}.tsv", std::process::id()));
    std::fs::write(
        &ledger,
        "FLEETLINK\ta\t1\tb\t1\t-1\t环边1\nFLEETLINK\tb\t1\ta\t1\t-1\t环边2\n",
    )
    .unwrap();
    fleet.load_links(&ledger).unwrap();
    // 不挂起、不重复爆栈；截断如实
    let res = fleet.attack_chains("a", NodeId(1), 64, 16);
    assert!(res.truncated, "环必须截断并如实报告");
    drop(fleet);
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
    let _ = std::fs::remove_file(&ledger);
}
