#![cfg(feature = "full")]
//! 落地状态机：六态快乐路径、超时默认策略（小自动/大降级）、验签纪律、账本重放。

use trit_immune::adoption::{Adoption, AdoptionState, Blast, ChangePackage, Impact};
use trit_immune::{Fleet, Journal, NodeId, NodeKind, Trit};

/// 仓 auth（登录被证伪）与仓 pay（支付 +1），跨仓边 auth#1 → pay#1。
fn build_fleet(tag: &str) -> Fleet {
    let mut fleet = Fleet::new();
    let pa = std::env::temp_dir().join(format!("adopt-a-{tag}-{}.journal", std::process::id()));
    let pb = std::env::temp_dir().join(format!("adopt-b-{tag}-{}.journal", std::process::id()));
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);

    let mut a = Journal::create_as(&pa, "main", 0).unwrap();
    let login = a.spawn(NodeId(0), NodeKind::Delta, "登录").unwrap();
    let atk = a.spawn(login, NodeKind::AttackVector, "SQLi").unwrap();
    a.record(atk, Trit::Vulnerable, None, "打穿").unwrap();
    a.record(login, Trit::Vulnerable, None, "面已证伪").unwrap();
    a.into_dag().unwrap();

    let mut b = Journal::create_as(&pb, "main", 1).unwrap();
    let pay = b.spawn(NodeId(0), NodeKind::Delta, "支付").unwrap();
    b.record(pay, Trit::Hardened, None, "通过").unwrap();
    b.into_dag().unwrap();

    fleet.add("auth", Journal::open_as(&pa, 0).unwrap()).unwrap();
    fleet.add("pay", Journal::open_as(&pb, 1).unwrap()).unwrap();
    fleet
        .add_cross_link(
            ("auth", NodeId(1)),
            ("pay", NodeId(1)),
            Trit::Vulnerable,
            "支付复用登录会话解析",
        )
        .unwrap();
    fleet
}

fn pkg(id: &str, blast: Blast, sla: u32) -> ChangePackage {
    ChangePackage {
        id: id.into(),
        title: "登录会话加固".into(),
        areas: vec![trit_immune::design::DesignArea::SecurityBoundary],
        linked_adr: Some("ADR-9".into()),
        migration: vec!["替换会话解析调用".into()],
        codemod: None,
        rollback: "回退到旧解析器".into(),
        decommission_after: 2,
        acceptance: vec!["contract-test".into()],
        owner: "auth-owner".into(),
        blast,
        receiver_sla: sla,
    }
}

#[test]
fn happy_path_walks_all_six_states() {
    let fleet = build_fleet("happy");
    let mut ad = Adoption::new();

    // 已发布 → 已通知（精准推送：只有 pay 仓受影响，且影响节点指认到位）
    let n = ad
        .publish(
            &fleet,
            pkg("CHG-1", Blast::Large, 3),
            &[("auth".into(), NodeId(1))],
        )
        .unwrap();
    assert_eq!(n, 1, "只有 pay 仓依赖 auth 登录");
    let r = &ad.receivers()[0];
    assert_eq!(r.state, AdoptionState::Notified);
    assert_eq!(r.impact, Some(Impact::NeedsAdaptation), "自动影响分析");
    assert_eq!(r.affected, vec![NodeId(1)], "受影响节点 = 支付");

    // 已评估影响
    ad.confirm_impact("CHG-1", "pay", Impact::NeedsAdaptation)
        .unwrap();
    assert_eq!(ad.receivers()[0].state, AdoptionState::ImpactAssessed);

    // 已适配
    ad.mark_adapted("CHG-1", "pay").unwrap();

    // 验签纪律：信号不齐不通过
    ad.verify("CHG-1", "pay", "wrong-signal").unwrap();
    assert_eq!(ad.receivers()[0].state, AdoptionState::Adapted);
    // 齐了 → 已验签
    ad.verify("CHG-1", "pay", "contract-test").unwrap();
    assert_eq!(ad.receivers()[0].state, AdoptionState::Verified);

    // 已归队
    ad.align("CHG-1", "pay").unwrap();
    assert_eq!(ad.receivers()[0].state, AdoptionState::Aligned);
}

#[test]
fn large_blast_timeout_degrades_and_blocks() {
    let fleet = build_fleet("large");
    let mut ad = Adoption::new();
    ad.publish(
        &fleet,
        pkg("CHG-L", Blast::Large, 1),
        &[("auth".into(), NodeId(1))],
    )
    .unwrap();

    ad.tick();
    let r = &ad.receivers()[0];
    assert!(r.degraded, "大变更超时 → 降级标记");
    assert_eq!(r.state, AdoptionState::Notified, "状态不动：不算落地");

    // 降级不影响纪律：仍未验签，不能归队
    assert!(ad.align("CHG-L", "pay").is_err());

    // 但人工通道还在：补完流程照样收敛
    ad.confirm_impact("CHG-L", "pay", Impact::NeedsAdaptation)
        .unwrap();
    ad.mark_adapted("CHG-L", "pay").unwrap();
    ad.verify("CHG-L", "pay", "contract-test").unwrap();
    ad.align("CHG-L", "pay").unwrap();
    assert_eq!(ad.receivers()[0].state, AdoptionState::Aligned);
}

#[test]
fn small_blast_timeout_auto_advances() {
    let fleet = build_fleet("small");
    let mut ad = Adoption::new();
    ad.publish(
        &fleet,
        pkg("CHG-S", Blast::Small, 1),
        &[("auth".into(), NodeId(1))],
    )
    .unwrap();

    // tick 1：自动确认影响
    let t = ad.tick();
    assert_eq!(
        t,
        vec![("CHG-S".into(), "pay".into(), AdoptionState::ImpactAssessed)]
    );
    // tick 2：小变更默认自动适配
    let t = ad.tick();
    assert_eq!(
        t,
        vec![("CHG-S".into(), "pay".into(), AdoptionState::Adapted)]
    );
    // tick 3：验签催办超时 → 降级标记（信号没齐就不能算落地）
    ad.tick();
    assert!(ad.receivers()[0].degraded);
    assert_eq!(ad.receivers()[0].state, AdoptionState::Adapted);
}

#[test]
fn decommission_counts_down_after_align() {
    let fleet = build_fleet("decomm");
    let mut ad = Adoption::new();
    ad.publish(
        &fleet,
        pkg("CHG-D", Blast::Large, 1),
        &[("auth".into(), NodeId(1))],
    )
    .unwrap();
    ad.confirm_impact("CHG-D", "pay", Impact::NeedsAdaptation)
        .unwrap();
    ad.mark_adapted("CHG-D", "pay").unwrap();
    ad.verify("CHG-D", "pay", "contract-test").unwrap();
    ad.align("CHG-D", "pay").unwrap();

    assert_eq!(ad.decommission(), vec![("CHG-D".into(), "pay".into(), 2)]);
    ad.tick();
    ad.tick();
    assert_eq!(ad.decommission(), vec![("CHG-D".into(), "pay".into(), 0)], "旧路径下线到期");
}

#[test]
fn ledger_replay_restores_state() {
    let fleet = build_fleet("replay");
    let ledger = std::env::temp_dir().join(format!(
        "trit-immune-adopt-{}.events",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&ledger);

    let mut ad = Adoption::new();
    ad.open_ledger(&ledger);
    ad.publish(
        &fleet,
        pkg("CHG-R", Blast::Large, 5),
        &[("auth".into(), NodeId(1))],
    )
    .unwrap();
    ad.confirm_impact("CHG-R", "pay", Impact::NeedsAdaptation)
        .unwrap();
    ad.mark_adapted("CHG-R", "pay").unwrap();
    ad.verify("CHG-R", "pay", "contract-test").unwrap();
    ad.align("CHG-R", "pay").unwrap();

    // 重放即复原
    let ad2 = Adoption::replay(&ledger).unwrap();
    assert_eq!(ad2.packages().len(), 1);
    assert_eq!(ad2.packages()[0].id, "CHG-R");
    assert_eq!(ad2.receivers().len(), 1);
    let r = &ad2.receivers()[0];
    assert_eq!(r.state, AdoptionState::Aligned);
    assert_eq!(r.verified, vec!["contract-test"]);
    assert_eq!(r.impact, Some(Impact::NeedsAdaptation));

    let _ = std::fs::remove_file(&ledger);
}
