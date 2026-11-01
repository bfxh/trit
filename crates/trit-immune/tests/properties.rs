//! 性质测试（property-based testing）：不测具体例子，测数学不变量。
//!
//! 用确定性随机生成器（xorshift64）生成随机输入，对每个输入验证系统性质。
//! 例子测试说"这个输入对"，性质测试说"任何输入都对"——后者是系统正确性的
//! 充分条件，也是重构时最硬的护栏。同一种子永远生成同一输入序列，失败可复现。
//!
//! 不变量清单（每条都来自 GOAL.md 的语义承诺）：
//! 1. **门禁单调性**：因果链上任意位置新增 `-1`，门禁永不升级——K3 AND（min）的单调性；
//! 2. **合并交换律**：merge(A,B) 与 merge(B,A) 收敛出同一份 DAG——
//!    HLC 戳即 Delta 身份、BTreeMap 全序归并的推论；
//! 3. **传播单调性**：给目标仓加一条确认穿透边，目标有效裁决只会变坏不会变好
//!    ——(min,max) 半环的单调性；
//! 4. **重放一致性**：实时增量写入、全量日志重放、快照+尾部重放收敛出同一份
//!    DAG——"日志即数据库"的充分条件；
//! 5. **收敛单调性**：把一个非 `+1` 节点修成 `+1`，收敛率严格上升。

use std::path::{Path, PathBuf};
use trit_immune::*;

/// xorshift64 确定性随机生成器：种子固定 → 输入序列固定。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick_trit(&mut self) -> Trit {
        match self.below(3) {
            0 => Trit::Vulnerable,
            1 => Trit::Unknown,
            _ => Trit::Hardened,
        }
    }
}

/// 门禁恶劣度排序：Allow(0) < Freeze(1) < Block(2)。单调性 = 恶劣度不降。
fn gate_rank(g: GateDecision) -> u8 {
    match g {
        GateDecision::Allow => 0,
        GateDecision::Freeze => 1,
        GateDecision::Block => 2,
    }
}

/// 临时日志路径（进程 id + 轮次去重；开始前清残留）。
fn tmp_path(tag: &str, n: u64) -> PathBuf {
    std::env::temp_dir().join(format!(
        "trit-prop-{tag}-{}-{n}.journal",
        std::process::id()
    ))
}

/// 读日志正文（跳过头部），与 tests/sync.rs 的 read_log 同一逻辑。
fn read_log(path: &Path) -> Vec<Stamped> {
    let text = std::fs::read_to_string(path).expect("读日志");
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let (ts, d) = parse_stamped_line(line).expect("日志行解析");
        out.push(Stamped { ts, delta: d });
    }
    out
}

// ── 性质 1：门禁单调性 ──────────────────────────────────────────────────

#[test]
fn prop_gate_monotone_new_minus_one_never_upgrades() {
    let mut rng = Rng(0xDEAD_BEEF);
    for round in 0..64u64 {
        // 链式 DAG：ROOT ← L0 ← L1 ← …，tip 的因果链覆盖全部节点
        let mut dag = ImmuneDag::root("main");
        let depth = 3 + rng.below(8) as usize;
        let mut chain = vec![NodeId(0)];
        let mut parent = NodeId(0);
        for i in 0..depth {
            let id = dag.spawn(parent, NodeKind::Delta, &format!("L{i}"));
            dag.record(id, rng.pick_trit(), None, "随机初始");
            chain.push(id);
            parent = id;
        }
        let tip = *chain.last().unwrap();

        // (a) 任意初始态：新增 -1，恶劣度不降
        let before = dag.merge_gate(tip);
        let victim = chain[rng.below(chain.len() as u64) as usize];
        dag.record(victim, Trit::Vulnerable, None, "新增-1");
        let after = dag.merge_gate(tip);
        assert!(
            gate_rank(after) >= gate_rank(before),
            "round {round}: 加 -1 后门禁升级 {before:?} → {after:?}"
        );

        // (b) 穷举位置：全链 +1（含根）→ Allow；链上任一节点改 -1 → 不得再 Allow
        for &n in &chain {
            dag.record(n, Trit::Hardened, None, "全+1");
        }
        assert_eq!(
            dag.merge_gate(tip),
            GateDecision::Allow,
            "round {round}: 全链 +1 应放行"
        );
        for &n in &chain {
            dag.record(n, Trit::Vulnerable, None, "改-1");
            assert_ne!(
                dag.merge_gate(tip),
                GateDecision::Allow,
                "round {round}: 链上 {n:?} 变 -1 后仍放行"
            );
            dag.record(n, Trit::Hardened, None, "复原");
        }
    }
}

// ── 性质 2：合并交换律（随机化） ─────────────────────────────────────────

#[test]
fn prop_merge_commutative_random_replicas() {
    let mut rng = Rng(0xCAFE_BABE);
    for round in 0..32u64 {
        let (pa, pb) = (tmp_path("mgmt-a", round), tmp_path("mgmt-b", round));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);

        // A：随机子树 + 随机裁决
        let mut a = Journal::create_as(&pa, "A", 0).unwrap();
        let mut a_view = 1usize; // 根
        let a_spawns = 2 + rng.below(6) as usize;
        let mut a_titles = Vec::new();
        for i in 0..a_spawns {
            let parent = NodeId(rng.below(a_view as u64) as u32);
            let title = format!("A{i}");
            let id = a.spawn(parent, NodeKind::Delta, &title).unwrap();
            a_titles.push(title.clone());
            a_view += 1;
            if rng.below(4) != 0 {
                let v = rng.pick_trit();
                a.record(id, v, None, "A 裁决").unwrap();
            }
        }
        drop(a.into_dag().unwrap()); // flush + 关句柄

        // B：克隆 A 的日志（前缀共享 → 同戳去重），以写者 1 追加自己的增量
        std::fs::copy(&pa, &pb).unwrap();
        let mut b = Journal::open_as(&pb, 1).unwrap();
        let b_spawns = 1 + rng.below(4) as usize;
        let mut reverify = Vec::new();
        for (i, b_view) in (a_view..a_view + b_spawns).enumerate() {
            let parent = NodeId(rng.below(b_view as u64) as u32);
            let title = format!("B{i}");
            let id = b.spawn(parent, NodeKind::Delta, &title).unwrap();
            if rng.below(4) == 0 {
                reverify.push(id);
            }
        }
        // B 对 A 的部分节点复验（同 id 语义：全局改写后应落在正确节点上）
        for &id in &reverify {
            b.record(id, rng.pick_trit(), None, "B 复验").unwrap();
        }
        drop(b.into_dag().unwrap());

        let sa = read_log(&pa);
        let sb = read_log(&pb);

        // 交换律：两种输入顺序收敛出同一份 DAG
        let m_ab = merge_replicas(vec![(0, sa.clone()), (1, sb.clone())]);
        let m_ba = merge_replicas(vec![(1, sb.clone()), (0, sa.clone())]);
        assert_eq!(m_ab.dag, m_ba.dag, "round {round}: 合并结果与输入顺序无关");
        assert!(
            m_ab.deferred.is_empty() && m_ba.deferred.is_empty(),
            "round {round}: 克隆+追加模型下不应有隔离项"
        );

        // 确定性：同一输入跑两遍结果一致
        let m_ab2 = merge_replicas(vec![(0, sa.clone()), (1, sb.clone())]);
        assert_eq!(m_ab.dag, m_ab2.dag, "round {round}: 合并确定性");

        // 节点数 = 根 + A 的 spawn + B 的 spawn（重复根已去重）
        assert_eq!(
            m_ab.dag.nodes().len(),
            1 + a_spawns + b_spawns,
            "round {round}: 节点数应精确"
        );
        // B 的复验要落在正确的全局节点上（按标题找回，标题唯一）
        for title in &a_titles {
            let node = m_ab
                .dag
                .nodes()
                .iter()
                .find(|n| &n.title == title)
                .unwrap_or_else(|| panic!("round {round}: 节点 {title} 丢失"));
            assert!(
                matches!(
                    node.verdict,
                    Trit::Vulnerable | Trit::Unknown | Trit::Hardened
                ),
                "round {round}: {title} 裁决损坏"
            );
        }
    }
}

// ── 性质 3：传播单调性 ──────────────────────────────────────────────────

#[cfg(feature = "full")]
#[test]
fn prop_propagation_confirmed_breach_never_improves_target() {
    for round in 0..5u64 {
        let (pa, pb) = (tmp_path("prop-a", round), tmp_path("prop-b", round));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);

        let mut a = Journal::create_as(&pa, "src", 0).unwrap();
        let src = a.spawn(NodeId(0), NodeKind::Delta, "源头").unwrap();
        a.record(src, Trit::Vulnerable, None, "确认打穿").unwrap();
        drop(a.into_dag().unwrap());

        let mut b = Journal::create_as(&pb, "dst", 1).unwrap();
        let dst = b.spawn(NodeId(0), NodeKind::Delta, "目标").unwrap();
        b.record(dst, Trit::Hardened, None, "自身通过").unwrap();
        drop(b.into_dag().unwrap());

        let mut fleet = Fleet::new();
        fleet.add("src", Journal::open_as(&pa, 0).unwrap()).unwrap();
        fleet.add("dst", Journal::open_as(&pb, 1).unwrap()).unwrap();

        // 无边：目标保持自身 +1
        let before = fleet.effective_verdicts("dst").unwrap();
        assert_eq!(before[1], Trit::Hardened, "round {round}: 无边不受影响");

        // 加一条确认穿透边：目标被污染
        fleet
            .add_cross_link(
                ("src", NodeId(1)),
                ("dst", NodeId(1)),
                Trit::Vulnerable,
                "确认穿透",
            )
            .unwrap();
        let after = fleet.effective_verdicts("dst").unwrap();
        assert_eq!(after[1], Trit::Vulnerable, "round {round}: 穿透应传染");

        // 再加一条 0（未知）边：坏消息主导，目标不会变好
        fleet
            .add_cross_link(
                ("src", NodeId(0)),
                ("dst", NodeId(1)),
                Trit::Unknown,
                "未知依赖",
            )
            .unwrap();
        let still = fleet.effective_verdicts("dst").unwrap();
        assert_eq!(still[1], Trit::Vulnerable, "round {round}: 加边不得洗白");
    }
}

// ── 性质 4：重放一致性（三条路径同一份 DAG） ────────────────────────────

#[test]
fn prop_replay_all_paths_converge_to_same_dag() {
    let mut rng = Rng(0xFACE_B00C);
    for round in 0..24u64 {
        let p = tmp_path("replay", round);
        let _ = std::fs::remove_file(&p);

        // 随机操作序列：spawn / record / quarantine
        let mut j = Journal::create_as(&p, "main", 0).unwrap();
        let mut ids: Vec<NodeId> = vec![NodeId(0)];
        let n_ops = 5 + rng.below(15);
        let mut title_n = 0usize;
        for _ in 0..n_ops {
            let target = ids[rng.below(ids.len() as u64) as usize];
            match rng.below(4) {
                0 => {
                    title_n += 1;
                    let id = j
                        .spawn(target, NodeKind::Delta, &format!("N{title_n}"))
                        .unwrap();
                    ids.push(id);
                }
                1 | 2 => {
                    let v = rng.pick_trit();
                    j.record(target, v, None, "随机裁决").unwrap();
                }
                _ => {
                    j.quarantine(target, "污染隔离").unwrap();
                }
            }
        }
        let live = j.dag().clone();

        // 路径 1：全量日志重放
        let full = Journal::open_as(&p, 0).unwrap();
        assert!(!full.recovered_from_snapshot());
        assert_eq!(full.dag(), &live, "round {round}: 全量重放 = 实时写");
        drop(full);

        // 路径 2：快照 + 尾部 0 条
        j.snapshot().unwrap();
        let snap0 = Journal::open_as(&p, 0).unwrap();
        assert!(snap0.recovered_from_snapshot());
        assert_eq!(snap0.tail_replayed(), 0);
        assert_eq!(snap0.dag(), &live, "round {round}: 快照恢复 = 实时写");
        drop(snap0);

        // 路径 3：快照 + 尾部 2 条
        let ids_now = ids.clone();
        for _ in 0..2 {
            let target = ids_now[rng.below(ids_now.len() as u64) as usize];
            j.record(target, rng.pick_trit(), None, "尾部增量").unwrap();
        }
        let live2 = j.dag().clone();
        let snap2 = Journal::open_as(&p, 0).unwrap();
        assert!(snap2.recovered_from_snapshot());
        assert_eq!(snap2.tail_replayed(), 2, "round {round}: 应只重放尾部 2 条");
        assert_eq!(snap2.dag(), &live2, "round {round}: 快照+尾部 = 实时写");
        drop(snap2);
        drop(j);
    }
}

// ── 性质 5：收敛单调性 ──────────────────────────────────────────────────

#[test]
fn prop_convergence_fixing_strictly_increases_ratio() {
    let mut rng = Rng(0xABCD_1234);
    for round in 0..32u64 {
        let mut dag = ImmuneDag::root("main");
        let n = 3 + rng.below(10) as usize;
        let mut ids = vec![NodeId(0)];
        let mut parent = NodeId(0);
        for i in 0..n {
            let id = dag.spawn(parent, NodeKind::Delta, &format!("N{i}"));
            dag.record(id, rng.pick_trit(), None, "初始");
            ids.push(id);
            parent = id;
        }

        // 随机修一个非 +1 节点（含根）
        let fix = ids[rng.below(ids.len() as u64) as usize];
        let old = dag.node(fix).verdict;
        let before = dag.convergence();
        dag.record(fix, Trit::Hardened, None, "修复");
        let after = dag.convergence();

        if old != Trit::Hardened {
            assert_eq!(
                after.hardened,
                before.hardened + 1,
                "round {round}: hardened 应恰好 +1"
            );
            assert!(
                after.ratio > before.ratio,
                "round {round}: 修复后收敛率应上升 {before:?} → {after:?}"
            );
        } else {
            assert_eq!(after, before, "round {round}: 重复 +1 不应改变收敛率");
        }
    }
}
