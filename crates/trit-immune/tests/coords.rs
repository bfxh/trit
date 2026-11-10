#![cfg(feature = "full")]
//! 依赖坐标（共享库 × 多项目 × 多智能体）测试。
//!
//! 每条锁一个语义纪律，且都要能判红：
//! 1. 扇出具域：库的 `-1` 只打到**实际用到该坐标**的项目，不同版本不受牵连；
//! 2. 不匹配不静默：来源/摘要/build 对不上 → 零落账 + 显式缺口上报；
//! 3. 幂等：重复入账不增节点，重复反熵第二次起零落账；
//! 4. `+1` 永不跨仓落账（库绿 ≠ 应用绿）；
//! 5. 不覆盖：已更坏时跳过（单调，不洗白）；
//! 6. 多智能体副本并发入账 → 合并顺序无关，且收敛后不再重复建节点；
//! 7. 判红接入：坐标被打穿 → 本仓有效裁决 `-1` → 策略否决（含 gate 仍 Allow 的路径）。

use std::path::{Path, PathBuf};
use trit_immune::*;

fn tmp(tag: &str, n: u64) -> PathBuf {
    std::env::temp_dir().join(format!("trit-coord-{tag}-{}-{n}", std::process::id()))
}

/// 造 Cargo.lock 文本：(name, version, source, checksum)。
fn lock_text(pins: &[(&str, &str, Option<&str>, Option<&str>)]) -> String {
    let mut s = String::from("# Cargo.lock v3\n");
    for (name, version, source, checksum) in pins {
        s.push_str(&format!(
            "[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n"
        ));
        if let Some(src) = source {
            s.push_str(&format!("source = \"{src}\"\n"));
        }
        if let Some(sum) = checksum {
            s.push_str(&format!("checksum = \"{sum}\"\n"));
        }
    }
    s
}

const REG: Option<&str> = Some("registry+https://github.com/rust-lang/crates.io-index");

fn key(name: &str, version: &str, source: Option<&str>, digest: Option<&str>) -> CoordKey {
    CoordKey {
        name: name.into(),
        version: version.into(),
        source: source.map(str::to_string),
        digest: digest.map(str::to_string),
        build: None,
    }
}

fn key_build(
    name: &str,
    version: &str,
    source: Option<&str>,
    digest: Option<&str>,
    build: Option<&str>,
) -> CoordKey {
    let mut k = key(name, version, source, digest);
    k.build = build.map(str::to_string);
    k
}

/// 最小仓：根 + 一个业务节点（写死裁决），落盘后重开（走真实 journal 路径）。
fn repo(path: &Path, writer: u32, title: &str, verdict: Trit) -> Journal {
    let _ = std::fs::remove_file(path);
    let mut j = Journal::create_as(path, title, writer).unwrap();
    let n = j.spawn(NodeId(0), NodeKind::Delta, title).unwrap();
    j.record(n, verdict, None, "初始").unwrap();
    drop(j.into_dag().unwrap());
    Journal::open_as(path, writer).unwrap()
}

/// 建一个"库 + 若干消费者"的舰队，返回舰队与各消费者坐标节点。
/// `ledger` 给了就先打开坐标注册表（声明随之落盘）。
struct Scene {
    fleet: Fleet,
    paths: Vec<PathBuf>,
    lib_node: NodeId,
    apps: Vec<(String, NodeId)>,
}

fn scene(tag: &str, app_locks: &[(&str, String)]) -> Scene {
    scene_with_ledger(tag, app_locks, None)
}

fn scene_with_ledger(tag: &str, app_locks: &[(&str, String)], ledger: Option<PathBuf>) -> Scene {
    let mut fleet = Fleet::new();
    if let Some(l) = &ledger {
        let _ = std::fs::remove_file(l);
        fleet.open_coord_ledger(l.clone());
    }
    let lib_path = tmp(&format!("{tag}-lib"), 0);
    fleet
        .add("lib", repo(&lib_path, 0, "lib", Trit::Vulnerable))
        .unwrap();
    let mut paths = vec![lib_path];
    let lib_node = NodeId(1);
    let mut apps = Vec::new();
    for (i, (name, lock)) in app_locks.iter().enumerate() {
        let p = tmp(&format!("{tag}-app-{i}"), i as u64);
        fleet
            .add(name, repo(&p, 10 + i as u32, name, Trit::Hardened))
            .unwrap();
        let report = fleet.attach_lock(name, lock, None).unwrap();
        let node = report
            .created
            .iter()
            .find(|(_, c)| c.name == "lib")
            .map(|(n, _)| *n)
            .expect("锁里的 lib 坐标必须入账");
        apps.push((name.to_string(), node));
        paths.push(p);
    }
    Scene {
        fleet,
        paths,
        lib_node,
        apps,
    }
}

impl Scene {
    /// 落盘全部仓的 journal（`sync` 才真正写到盘，flush 只到 OS）。
    fn flush_all(&mut self) {
        let names: Vec<String> = self.fleet.names().iter().map(|s| s.to_string()).collect();
        for n in names {
            self.fleet.get_mut(&n).unwrap().sync().unwrap();
        }
    }

    /// 用同一批 journal 文件重开舰队（写者号与创建时一致），验证持久化与注册表读回。
    fn reopen(&self) -> Fleet {
        let mut fleet = Fleet::new();
        fleet
            .add("lib", Journal::open_as(&self.paths[0], 0).unwrap())
            .unwrap();
        for (i, (name, _)) in self.apps.iter().enumerate() {
            fleet
                .add(
                    name,
                    Journal::open_as(&self.paths[i + 1], 10 + i as u32).unwrap(),
                )
                .unwrap();
        }
        fleet
    }

    fn cleanup(self) {
        for p in &self.paths {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[test]
fn fanout_is_scoped_to_the_exact_coordinate() {
    let lock_old = lock_text(&[("lib", "1.2.0", REG, Some("aaa"))]);
    let lock_new = lock_text(&[("lib", "2.0.0", REG, Some("bbb"))]);
    let mut s = scene("fanout", &[("app1", lock_old), ("app2", lock_new)]);
    // 库的两个发布节点：1.2.0 被打穿，2.0.0 尚未出结论（0）
    let rel2 = s
        .fleet
        .get_mut("lib")
        .unwrap()
        .spawn(NodeId(0), NodeKind::Delta, "release-2.0.0")
        .unwrap();
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    s.fleet
        .declare_provider("lib", rel2, key("lib", "2.0.0", REG, Some("bbb")))
        .unwrap();

    // 每个消费者只与"自己实际锁到的那个坐标"连边
    let edges = s.fleet.coord_edges();
    assert_eq!(edges.len(), 2, "两条坐标边，各对各的版本：{edges:?}");
    assert_eq!(
        edges.iter().filter(|e| e.from.1 == s.lib_node).count(),
        1,
        "被打穿的 1.2.0 只应连到 app1——按 name@version 家族连边会把 2.0.0 也拖进来"
    );

    let landed = s.fleet.sync_coordinates("坐标反熵").unwrap();
    assert_eq!(
        landed.len(),
        1,
        "只有实际用到该坐标的项目才落账：{landed:?}"
    );
    assert_eq!(landed[0].from.0, "lib");
    assert_eq!(landed[0].to.0, "app1");
    assert_eq!(landed[0].verdict, Trit::Vulnerable);

    assert_eq!(
        s.fleet.repo_verdict("app1", s.apps[0].1).unwrap(),
        Trit::Vulnerable,
        "app1（用 1.2.0）应被打穿"
    );
    assert_eq!(
        s.fleet.repo_verdict("app2", s.apps[1].1).unwrap(),
        Trit::Unknown,
        "app2（用 2.0.0）不得被 1.2.0 的破口牵连——扇出具域"
    );
    // 两个坐标都有提供者：不该再有缺口
    assert!(
        s.fleet.coord_gaps().is_empty(),
        "扇出具域后不应误报缺口：{:?}",
        s.fleet.coord_gaps()
    );
    s.cleanup();
}

#[test]
fn digest_mismatch_does_not_land_and_is_reported() {
    // 同 name@version 同来源，但消费者锁里的 checksum 与提供者不同
    let lock = lock_text(&[("lib", "1.2.0", REG, Some("EVIL"))]);
    let mut s = scene("digest", &[("app", lock)]);
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();

    assert!(
        s.fleet.coord_edges().is_empty(),
        "摘要不同不得连边（连了就是把未证的同源性当真）"
    );
    assert!(s.fleet.sync_coordinates("坐标反熵").unwrap().is_empty());
    assert_eq!(
        s.fleet.repo_verdict("app", s.apps[0].1).unwrap(),
        Trit::Unknown,
        "零落账"
    );
    let gaps = s.fleet.coord_gaps();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].reason, GapReason::DigestMismatch, "不匹配必须上报");
    assert!(gaps[0].note().contains("摘要"), "{}", gaps[0].note());
}

#[test]
fn coordinate_without_provider_is_reported() {
    // 消费者锁里有 lib 9.9.9，但没人声明它的裁决出处
    let lock = lock_text(&[("lib", "9.9.9", REG, Some("zzz"))]);
    let mut s = scene("noprov", &[("app", lock)]);
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    assert!(s.fleet.coord_edges().is_empty(), "没有提供者不得凭空连边");
    assert!(s.fleet.sync_coordinates("坐标反熵").unwrap().is_empty());
    let gaps = s.fleet.coord_gaps();
    assert_eq!(gaps.len(), 1, "未覆盖的坐标必须进 0 态候选：{:?}", gaps);
    assert_eq!(gaps[0].reason, GapReason::NoProvider);
    assert!(gaps[0].note().contains("无提供者"), "{}", gaps[0].note());
    s.cleanup();
}

#[test]
fn source_mismatch_is_a_confusion_signal() {
    let lock = lock_text(&[(
        "lib",
        "1.2.0",
        Some("registry+https://internal.example"),
        Some("aaa"),
    )]);
    let mut s = scene("confusion", &[("app", lock)]);
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    assert!(s.fleet.coord_edges().is_empty());
    let gaps = s.fleet.coord_gaps();
    assert_eq!(gaps.len(), 1);
    assert_eq!(
        gaps[0].reason,
        GapReason::SourceMismatch,
        "同名同版本换了来源 = dependency confusion，必须单列"
    );
}

#[test]
fn build_suffix_splits_the_same_checksum() {
    // 提供者按 feature 集 A 出证；消费者 B 用同一 checksum 但不同 feature 集
    let lock_a = lock_text(&[("lib", "1.2.0", REG, Some("aaa"))]);
    let mut fleet = Fleet::new();
    let lib_path = tmp("build-lib", 0);
    fleet
        .add("lib", repo(&lib_path, 0, "lib", Trit::Vulnerable))
        .unwrap();
    let app_a = tmp("build-a", 1);
    let app_b = tmp("build-b", 2);
    fleet
        .add("appA", repo(&app_a, 10, "appA", Trit::Hardened))
        .unwrap();
    fleet
        .add("appB", repo(&app_b, 11, "appB", Trit::Hardened))
        .unwrap();
    let ra = fleet.attach_lock("appA", &lock_a, Some("feat-x")).unwrap();
    let rb = fleet.attach_lock("appB", &lock_a, Some("feat-y")).unwrap();
    fleet
        .declare_provider(
            "lib",
            NodeId(1),
            key_build("lib", "1.2.0", REG, Some("aaa"), Some("feat-x")),
        )
        .unwrap();

    let edges = fleet.coord_edges();
    assert_eq!(edges.len(), 1, "只有 build 也对齐的坐标才算同一个产物");
    assert_eq!(edges[0].to.0, "appA");
    let landed = fleet.sync_coordinates("坐标反熵").unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(
        fleet.repo_verdict("appA", ra.created[0].0).unwrap(),
        Trit::Vulnerable
    );
    assert_eq!(
        fleet.repo_verdict("appB", rb.created[0].0).unwrap(),
        Trit::Unknown,
        "feature 集不同 → 未被证明受同一破口影响"
    );
    let gaps = fleet.coord_gaps();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].reason, GapReason::BuildMismatch);
    for p in [&lib_path, &app_a, &app_b] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn attach_and_sync_are_idempotent() {
    let lock = lock_text(&[
        ("lib", "1.2.0", REG, Some("aaa")),
        ("other", "0.1.0", REG, Some("ccc")),
    ]);
    let mut s = scene("idem", &[("app", lock.clone())]);
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    let nodes_after_first = s.fleet.get("app").unwrap().dag().nodes().len();
    let edges_after_first = s.fleet.coord_edges().len();

    let again = s.fleet.attach_lock("app", &lock, None).unwrap();
    assert!(
        again.created.is_empty(),
        "第二遍不得新建节点：{:?}",
        again.created
    );
    assert_eq!(again.reused, 2, "两个坐标都应复用");
    assert_eq!(
        s.fleet.get("app").unwrap().dag().nodes().len(),
        nodes_after_first,
        "重复入账节点数不变"
    );
    assert_eq!(s.fleet.coord_edges().len(), edges_after_first);

    let first = s.fleet.sync_coordinates("坐标反熵").unwrap();
    assert_eq!(first.len(), 1);
    assert!(
        s.fleet.sync_coordinates("坐标反熵").unwrap().is_empty(),
        "变坏才落账：第二遍应零落账"
    );
}

#[test]
fn plus_one_never_lands_from_a_shared_lib() {
    let lock = lock_text(&[("lib", "1.2.0", REG, Some("aaa"))]);
    let mut fleet = Fleet::new();
    let lib_path = tmp("pos-lib", 0);
    fleet
        .add("lib", repo(&lib_path, 0, "lib", Trit::Hardened))
        .unwrap();
    let app = tmp("pos-app", 1);
    fleet
        .add("app", repo(&app, 10, "app", Trit::Unknown))
        .unwrap();
    let r = fleet.attach_lock("app", &lock, None).unwrap();
    fleet
        .declare_provider("lib", NodeId(1), key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();

    assert_eq!(fleet.coord_edges().len(), 1, "坐标匹配，边应当存在");
    assert!(
        fleet.sync_coordinates("坐标反熵").unwrap().is_empty(),
        "+1 永不跨仓落账：下游的 +1 必须自己挣"
    );
    assert_eq!(
        fleet.repo_verdict("app", r.created[0].0).unwrap(),
        Trit::Unknown
    );
    let _ = std::fs::remove_file(&lib_path);
    let _ = std::fs::remove_file(&app);
}

#[test]
fn landing_is_monotonic_and_never_whitewashes() {
    let lock = lock_text(&[("lib", "1.2.0", REG, Some("aaa"))]);
    let mut s = scene("mono", &[("app", lock)]);
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    s.fleet.sync_coordinates("坐标反熵").unwrap();
    let coord = s.apps[0].1;
    assert_eq!(
        s.fleet.repo_verdict("app", coord).unwrap(),
        Trit::Vulnerable
    );
    // 落账必须可溯源：账目里要点名是哪个坐标从哪个仓打过来的
    let node = s.fleet.get("app").unwrap().dag().node(coord);
    assert!(
        node.history.iter().any(|e| e.note.contains("coord lib:1")),
        "落账 note 必须带坐标溯源，实际：{:?}",
        node.history.iter().map(|e| &e.note).collect::<Vec<_>>()
    );

    // 库侧后来出了 +1（补丁复验）：不得把消费者洗白
    s.fleet
        .record_repo("lib", s.lib_node, Trit::Hardened, "补丁复验")
        .unwrap();
    assert!(
        s.fleet.sync_coordinates("坐标反熵").unwrap().is_empty(),
        "+1 不落账，消费者已入账的 -1 不受影响"
    );
    assert_eq!(
        s.fleet.repo_verdict("app", coord).unwrap(),
        Trit::Vulnerable,
        "已落的账不被上游变好覆盖（下游要自己重验）"
    );
}

#[test]
fn coordinate_breach_reaches_the_gate_and_vetoes_merge() {
    let lock = lock_text(&[("lib", "1.2.0", REG, Some("aaa"))]);
    let mut s = scene("gate", &[("app", lock)]);
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    s.fleet.sync_coordinates("坐标反熵").unwrap();

    // 消费边（坐标 → 本仓根）让污染走到门禁的输入上
    let eff = s.fleet.effective_verdicts("app").unwrap();
    assert_eq!(
        eff[0],
        Trit::Vulnerable,
        "用了被打穿的坐标，本仓根的有效裁决必须 -1"
    );
    let dag = s.fleet.get("app").unwrap().dag();
    let gate = dag.merge_gate(NodeId(0));
    // 消费边（坐标 → 本仓根）让**门禁层**直接看见坐标污染
    assert_eq!(
        gate,
        GateDecision::Block,
        "用了被打穿的坐标，本仓根的门禁必须直接 Block"
    );
    let snapshot = PrSnapshot {
        gate,
        layers: vec![Trit::Hardened, Trit::Hardened],
        baselines: vec![],
        verdict: eff[0],
    };
    let policy = MergePolicy {
        channel: Channel::Main,
        auto_merge: true,
        required_baselines: vec![],
    };
    match decide(&policy, &snapshot) {
        Decision::Block(reason) => assert!(
            reason.contains("因果链"),
            "此路的 gate 已 Block，应由门禁层否决：{reason}"
        ),
        other => panic!("应 Block，实际 {other:?}（gate={gate:?}）"),
    }
    // 第二把锁：调用方若拿"别的节点的 gate"（因果链干净）来问，
    // 传播后的 -1 仍必须一票否决——见 policy 层同名用例
    let synthetic = PrSnapshot {
        gate: GateDecision::Allow,
        layers: vec![Trit::Hardened],
        baselines: vec![],
        verdict: eff[0],
    };
    match decide(&policy, &synthetic) {
        Decision::Block(reason) => assert!(
            reason.contains("有效裁决"),
            "gate 看不见污染时必须由有效裁决层兜住：{reason}"
        ),
        other => panic!("gate=Allow + 有效裁决 -1 应 Block，实际 {other:?}"),
    }
}

#[test]
fn concurrent_agents_converge_order_independently() {
    // 两个智能体各自克隆 app 仓、各自入账同一份锁、各自被同一坐标打穿
    let base = tmp("ma-base", 0);
    let ra = tmp("ma-a", 1);
    let rb = tmp("ma-b", 2);
    let lib = tmp("ma-lib", 3);
    let j0 = repo(&base, 0, "app", Trit::Hardened);
    drop(j0.into_dag().unwrap());
    std::fs::copy(&base, &ra).unwrap();
    std::fs::copy(&base, &rb).unwrap();

    let lock = lock_text(&[("lib", "1.2.0", REG, Some("aaa"))]);
    let mut fa = Fleet::new();
    fa.add("lib", repo(&lib, 0, "lib", Trit::Vulnerable))
        .unwrap();
    fa.add("app", Journal::open_as(&ra, 1).unwrap()).unwrap();
    fa.declare_provider("lib", NodeId(1), key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    fa.attach_lock("app", &lock, None).unwrap();
    fa.sync_coordinates("A 的反熵").unwrap();
    let mut fb = Fleet::new();
    fb.add("lib2", Journal::open_as(&lib, 0).unwrap()).unwrap();
    fb.add("app", Journal::open_as(&rb, 2).unwrap()).unwrap();
    fb.attach_lock("app", &lock, None).unwrap();
    // B 侧自己挣得同一个结论（同一坐标、同一 -1）
    fb.declare_provider("lib2", NodeId(1), key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    fb.sync_coordinates("B 的反熵").unwrap();
    for f in [&mut fa, &mut fb] {
        f.get_mut("app").unwrap().sync().unwrap();
    }

    let sa = read_log(&ra);
    let sb = read_log(&rb);
    let m1 = merge_replicas(vec![(1, sa.clone()), (2, sb.clone())]);
    let m2 = merge_replicas(vec![(2, sb), (1, sa)]);
    assert_eq!(m1.dag, m2.dag, "合并与顺序无关（δ-CRDT 收敛）");
    assert!(m1.deferred.is_empty(), "{:?}", m1.deferred);

    let coords: Vec<_> = m1
        .dag
        .nodes()
        .iter()
        .filter(|n| n.title.starts_with("COORD "))
        .collect();
    assert_eq!(coords.len(), 2, "两副本各建一次，合并后如实两个节点");
    assert!(
        coords.iter().all(|n| n.verdict == Trit::Vulnerable),
        "两个副本都判了同一个 -1，没有放大也没有丢账"
    );

    // 收敛后重新入账同一份锁：按标题采纳既有节点，不再新增（幂等的跨副本形态）
    let mut again = Fleet::new();
    again.add("app", Journal::open_as(&ra, 1).unwrap()).unwrap();
    let r2 = again.attach_lock("app", &lock, None).unwrap();
    assert!(
        r2.created.is_empty(),
        "已有同标题坐标节点时应采纳，不再建第二个：{:?}",
        r2.created
    );
    assert_eq!(r2.reused, 1);

    for p in [&base, &ra, &rb, &lib] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn coord_ledger_round_trips_and_dedupes() {
    let ledger = tmp("ledger.txt", 0);
    let lock = lock_text(&[("lib", "1.2.0", REG, Some("aaa"))]);
    let mut s = scene_with_ledger("rt", &[("app", lock)], Some(ledger.clone()));
    s.fleet
        .declare_provider("lib", s.lib_node, key("lib", "1.2.0", REG, Some("aaa")))
        .unwrap();
    assert_eq!(s.fleet.coord_edges().len(), 1, "现场派生应有一条坐标边");
    let uses = s.fleet.uses().len();

    // 用同一批 journal 重开（声明为空），只从注册表读回
    s.flush_all();
    let mut f2 = s.reopen();
    let (np, nu, bad) = f2.load_coords(&ledger).unwrap();
    assert_eq!(bad, 0, "自写的行必须自读回");
    assert_eq!((np, nu), (1, uses), "提供者/消费声明读回条数");
    assert_eq!(
        f2.coord_edges().len(),
        1,
        "读回的声明应能重新派生出同一条坐标边"
    );
    let (np2, nu2, _) = f2.load_coords(&ledger).unwrap();
    assert_eq!((np2, nu2), (0, 0), "追加式注册表重复加载不得叠加");
    assert_eq!(f2.uses().len(), uses);
    let _ = std::fs::remove_file(&ledger);
    drop(f2);
    s.cleanup();
}

fn read_log(path: &Path) -> Vec<Stamped> {
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
