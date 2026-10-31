//! 舰队：多仓聚合视图（"公共流程图"第一步）与副本配对同步。
//!
//! 定位：每仓独立 journal（各自根、各自写者）。舰队提供**聚合解释面**
//! （收敛表 + 聚合流程图），**不做跨仓物理合并**——物理合并只属于
//! 同一仓的副本（克隆分叉场景，见 `sync::anti_entropy`；
//! 独立仓强行合并会把两棵树嫁接到一棵根上）。

use crate::dag::NodeId;
use crate::journal::Journal;
use crate::propagate::Contagion;
use crate::sync;
use crate::trit::Trit;
use crate::delta::{esc, unesc};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

/// 跨仓污染边：仓 A 的节点 → 仓 B 的节点（依赖 / 共享面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossRepoLink {
    pub from: (String, NodeId),
    pub to: (String, NodeId),
    /// 通过强度：`-1` 确认可穿透 / `0` 疑似 / `+1` 已证拦截。
    pub pass: Trit,
    pub note: String,
}

/// 多仓舰队：名字 → journal，名字唯一、顺序稳定；另持跨仓污染边与可选注册表。
#[derive(Debug, Default)]
pub struct Fleet {
    repos: Vec<(String, Journal)>,
    cross_links: Vec<CrossRepoLink>,
    link_ledger: Option<PathBuf>,
}

/// 聚合收敛表的一行。
#[derive(Debug, Clone, PartialEq)]
pub struct FleetRow {
    pub name: String,
    pub total: usize,
    pub hardened: usize,
    pub ratio: f64,
    pub suspects: usize,
    pub quarantined: usize,
}

impl Fleet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一仓（= 智能体通道注册的最小形态）。重名拒绝。
    pub fn add(&mut self, name: &str, journal: Journal) -> Result<(), String> {
        if self.repos.iter().any(|(n, _)| n == name) {
            return Err(format!("重名仓：{name}"));
        }
        self.repos.push((name.to_string(), journal));
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Journal> {
        self.repos.iter().find(|(n, _)| n == name).map(|(_, j)| j)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Journal> {
        self.repos
            .iter_mut()
            .find(|(n, _)| n == name)
            .map(|(_, j)| j)
    }

    /// 读某仓某节点当前裁决（跨仓协议的只读原语）。
    pub fn repo_verdict(&self, repo: &str, id: NodeId) -> io::Result<Trit> {
        let j = self
            .get(repo)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("无仓 {repo}")))?;
        let dag = j.dag();
        if id.0 as usize >= dag.nodes().len() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("节点 #{:02} 不存在于 {repo}", id.0),
            ));
        }
        Ok(dag.node(id).verdict)
    }

    /// 记录某仓某节点的裁决（跨仓协议的仓内写原语，走该仓自己的 journal）。
    pub fn record_repo(&mut self, repo: &str, id: NodeId, verdict: Trit, note: &str) -> io::Result<()> {
        let j = self
            .get_mut(repo)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("无仓 {repo}")))?;
        j.record(id, verdict, None, note)
            .map_err(io::Error::other)
    }

    pub fn names(&self) -> Vec<&str> {
        self.repos.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// 仓清单（名 + journal；供舰队级影响分析等编排层只读使用）。
    pub fn repos(&self) -> &[(String, Journal)] {
        &self.repos
    }

    /// 聚合收敛表：每仓一行（公共流程图的数据层）。
    pub fn report(&self) -> Vec<FleetRow> {
        self.repos
            .iter()
            .map(|(n, j)| {
                let cv = j.dag().convergence();
                FleetRow {
                    name: n.clone(),
                    total: cv.total,
                    hardened: cv.hardened,
                    ratio: cv.ratio,
                    suspects: j.dag().suspects().len(),
                    quarantined: cv.quarantined,
                }
            })
            .collect()
    }

    /// 聚合流程图：每仓一节（收敛头 + render）+ 跨仓依赖边列表。
    pub fn render_with_edges(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "══ 舰队　{} 仓　跨仓污染边 {} 条 ══",
            self.repos.len(),
            self.cross_links.len()
        );
        for (n, j) in &self.repos {
            let cv = j.dag().convergence();
            let _ = writeln!(
                out,
                "\n── 仓库 {n}　收敛 {:.0}%（{}/{}）　0 态 {}　隔离 {} ──",
                cv.ratio * 100.0,
                cv.hardened,
                cv.total,
                j.dag().suspects().len(),
                cv.quarantined
            );
            out.push_str(&j.dag().render());
        }
        if !self.cross_links.is_empty() {
            out.push_str("\n── 跨仓依赖边 ──\n");
            for l in &self.cross_links {
                let _ = writeln!(
                    out,
                    "{}#{:02} → {}#{:02}　[{} {}]　{}",
                    l.from.0,
                    l.from.1.0,
                    l.to.0,
                    l.to.1.0,
                    l.pass,
                    l.pass.symbol(),
                    l.note
                );
            }
        }
        out
    }

    /// 聚合流程图（不含跨仓边，向后兼容）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (n, j) in &self.repos {
            let cv = j.dag().convergence();
            let _ = writeln!(
                out,
                "══ 仓库 {n}　收敛 {:.0}%（{}/{}）　0 态待办 {}　隔离 {} ══",
                cv.ratio * 100.0,
                cv.hardened,
                cv.total,
                j.dag().suspects().len(),
                cv.quarantined
            );
            out.push_str(&j.dag().render());
            out.push('\n');
        }
        out
    }

    /// 同名克隆仓的副本配对同步（物理合并；独立仓禁用——会嫁接树根）。
    pub fn sync_pair(&mut self, a: &str, b: &str) -> io::Result<(sync::MergeOutcome, sync::MergeOutcome)> {
        let ia = self
            .repos
            .iter()
            .position(|(n, _)| n == a)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("无仓 {a}")))?;
        let ib = self
            .repos
            .iter()
            .position(|(n, _)| n == b)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("无仓 {b}")))?;
        if ia == ib {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "同一仓不能与自身反熵",
            ));
        }
        let (lo, hi) = if ia < ib { (ia, ib) } else { (ib, ia) };
        let (l, r) = self.repos.split_at_mut(hi);
        let ja = &mut l[lo].1;
        let jb = &mut r[0].1;
        sync::anti_entropy(ja, jb)
    }

    /// 打开跨仓边注册表（追加式；"账外无数据"——跨仓边影响裁决，必须入账）。
    pub fn open_link_ledger(&mut self, path: impl Into<PathBuf>) {
        self.link_ledger = Some(path.into());
    }

    /// 声明一条跨仓污染边（两端仓与节点必须已存在）。
    pub fn add_cross_link(
        &mut self,
        from: (&str, NodeId),
        to: (&str, NodeId),
        pass: Trit,
        note: &str,
    ) -> Result<(), String> {
        let (fr, fid) = from;
        let (tr, tid) = to;
        let ja = self
            .get(fr)
            .ok_or_else(|| format!("源仓 {fr} 不存在"))?;
        if fid.0 as usize >= ja.dag().nodes().len() {
            return Err(format!("源节点 #{:02} 不存在于 {fr}", fid.0));
        }
        let jb = self
            .get(tr)
            .ok_or_else(|| format!("目标仓 {tr} 不存在"))?;
        if tid.0 as usize >= jb.dag().nodes().len() {
            return Err(format!("目标节点 #{:02} 不存在于 {tr}", tid.0));
        }
        let link = CrossRepoLink {
            from: (fr.to_string(), fid),
            to: (tr.to_string(), tid),
            pass,
            note: note.to_string(),
        };
        if let Some(ledger) = &self.link_ledger {
            let line = format!(
                "FLEETLINK\t{}\t{}\t{}\t{}\t{}\t{}\n",
                esc(fr),
                fid.0,
                esc(tr),
                tid.0,
                pass,
                esc(note)
            );
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(ledger)
                .map_err(|e| e.to_string())?;
            use std::io::Write as _;
            f.write_all(line.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        self.cross_links.push(link);
        Ok(())
    }

    /// 从注册表载入跨仓边（追加式：只增；未知仓/节点的边照收，
    /// 传播时跳过——加载期不验证，注册表与仓的状态解耦）。
    pub fn load_links(&mut self, path: &Path) -> io::Result<(usize, usize)> {
        let text = std::fs::read_to_string(path)?;
        let (mut loaded, mut skipped) = (0usize, 0usize);
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut f = line.split('\t');
            match (f.next(), f.next(), f.next(), f.next(), f.next(), f.next(), f.next(), f.next()) {
                (
                    Some("FLEETLINK"),
                    Some(fr),
                    Some(fid),
                    Some(tr),
                    Some(tid),
                    Some(pass),
                    Some(note),
                    None,
                ) => {
                    let parsed = (|| -> io::Result<CrossRepoLink> {
                    let fr = unesc(fr).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    let fid = NodeId(
                        fid.parse::<u32>()
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                    );
                    let tr = unesc(tr).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    let tid = NodeId(
                        tid.parse::<u32>()
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                    );
                    let pass = pass
                        .parse::<Trit>()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    let note =
                        unesc(note).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    Ok(CrossRepoLink {
                        from: (fr, fid),
                        to: (tr, tid),
                        pass,
                        note,
                    })
                })()
                .map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("注册表行不合法：{e}"))
                })?;
                    self.cross_links.push(parsed);
                    loaded += 1;
                }
                _ => skipped += 1,
            }
        }
        Ok((loaded, skipped))
    }

    pub fn cross_links(&self) -> &[CrossRepoLink] {
        &self.cross_links
    }

    /// 舰队级污染传播：仓内边 + 跨仓边统一进 (min,max) 半环。
    /// 源规则与单仓一致：非 `+1` 或被隔离者外泄（确证 `-1` 强度、可疑/隔离 `0` 强度）。
    /// 返回 仓名 → 该仓各节点的污染等级。未知仓的跨仓边跳过（加载期解耦）。
    pub fn contamination(&self) -> BTreeMap<String, Vec<Trit>> {
        let mut base: BTreeMap<String, usize> = BTreeMap::new();
        let mut total = 0usize;
        for (n, j) in &self.repos {
            base.insert(n.clone(), total);
            total += j.dag().nodes().len();
        }
        let mut g = Contagion::new(total);
        for (n, j) in &self.repos {
            let off = base[n];
            for l in j.dag().links() {
                g.link(off + l.from.0 as usize, off + l.to.0 as usize, l.pass);
            }
        }
        for l in &self.cross_links {
            let (Some(fo), Some(to)) = (base.get(&l.from.0), base.get(&l.to.0)) else {
                continue;
            };
            let fi = fo + l.from.1.0 as usize;
            let ti = to + l.to.1.0 as usize;
            if fi < total && ti < total {
                g.link(fi, ti, l.pass);
            }
        }
        let mut sources = Vec::new();
        for (n, j) in &self.repos {
            let off = base[n];
            for node in j.dag().nodes() {
                let sev = if node.quarantined {
                    node.verdict.and(Trit::Unknown)
                } else if node.verdict != Trit::Hardened {
                    node.verdict
                } else {
                    continue;
                };
                sources.push((off + node.id.0 as usize, sev));
            }
        }
        let dist = g.spread(&sources);
        let mut out: BTreeMap<String, Vec<Trit>> = BTreeMap::new();
        for (n, j) in &self.repos {
            let off = base[n];
            let len = j.dag().nodes().len();
            out.insert(n.clone(), dist[off..off + len].to_vec());
        }
        out
    }

    /// 跨仓有效裁决 = min(自身裁决, 舰队污染)——`policy::PrSnapshot.verdict` 的数据源。
    pub fn effective_verdicts(&self, repo: &str) -> io::Result<Vec<Trit>> {
        let cont = self
            .contamination()
            .remove(repo)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("无仓 {repo}")))?;
        let j = self
            .get(repo)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("无仓 {repo}")))?;
        Ok(j.dag()
            .nodes()
            .iter()
            .map(|n| n.verdict.and(cont[n.id.0 as usize]))
            .collect())
    }

    /// **跨仓 Delta 反熵**（完整协议）：扫描全部跨仓边，把源端当前裁决经边强度
    /// **落账**到目标端节点——目标仓自己的 journal 里出现 `Delta::Record`，
    /// 独立打开也能看到影响历史，门禁可折叠，重放一致（与"查询时传播"
    /// `effective_verdicts` 互补：查询是现算视图，落账是入账事实）。
    ///
    /// 语义纪律（与全系统同构）：
    /// - 落账值 = `max(源 verdict, 边 pass)`——与 (min,max) 查询语义严格一致，
    ///   落账后查询结果不会与账目矛盾；
    /// - **变坏才落账**（落账值 < 目标自身 verdict 才写）——天然幂等，可反复跑；
    /// - **`+1` 永不跨仓落账**——下游的 +1 必须自己验证挣得，与"扫描器永不 +1"
    ///   同一条纪律；已证拦截边（pass=+1）同样不落坏消息；
    /// - 未知仓/越界节点跳过（与 `load_links` 的加载期解耦语义一致）。
    ///
    /// 两阶段：先全量收集计划（不可变借用），再逐条落账（可变借用）。
    /// note 自动附加溯源（`← 仓:节点`），账目可追溯。
    pub fn sync_cross_repo(&mut self, note: &str) -> io::Result<Vec<CrossRepoDelta>> {
        // 阶段 1：收集计划
        type Plan = Vec<((String, NodeId), (String, NodeId), Trit)>;
        let mut plan: Plan = Vec::new();
        for link in &self.cross_links {
            let Some(src) = self.get(&link.from.0) else {
                continue;
            };
            let src_dag = src.dag();
            if link.from.1 .0 as usize >= src_dag.nodes().len() {
                continue;
            }
            let sv = src_dag.node(link.from.1).verdict;
            let out = sv.max(link.pass);
            if out == Trit::Hardened {
                continue; // +1 永不跨仓
            }
            let Some(dst) = self.get(&link.to.0) else {
                continue;
            };
            let dst_dag = dst.dag();
            if link.to.1 .0 as usize >= dst_dag.nodes().len() {
                continue;
            }
            let dv = dst_dag.node(link.to.1).verdict;
            if out >= dv {
                continue; // 变坏才落账
            }
            plan.push((link.from.clone(), link.to.clone(), out));
        }
        // 阶段 2：落账
        let mut results = Vec::with_capacity(plan.len());
        for (from, to, v) in plan {
            let full_note = format!("{note} ← {}:{}", from.0, from.1 .0);
            if let Some(j) = self.get_mut(&to.0) {
                j.record(to.1, v, None, &full_note)
                    .map_err(io::Error::other)?;
            }
            results.push(CrossRepoDelta {
                from,
                to,
                verdict: v,
            });
        }
        Ok(results)
    }

    /// **多跳攻击链枚举**（fleet 级渗透路径报告，`dag::breach_sources` 的跨仓
    /// 完全体）：列出从任一确认 `-1` 源到 `target` 的污染路径。
    ///
    /// 语义（与传播引擎严格对齐）：
    /// - **源判定 = 节点自身 verdict 为 `-1`**——跨仓反熵（[`Self::sync_cross_repo`]）
    ///   已把污染落账进目标节点自身，所以"最近的确认源"直接从账本读；
    /// - **只有确认穿透边（pass = `-1`）传播打穿**——(min,max) 语义下
    ///   max(源 `-1`, 边) ≤ 0 的边都不传 `-1`；
    /// - 可疑节点（自身 `0`）不 emit 但**继续追根**（上游可能有确认源）；
    /// - 自身 `+1` 的节点**挡路即停**（确认源不会在它上游——污染没穿过它）；
    /// - 锚定攻击分支（`AttackVector` 且 `-1`）是叶子源；
    /// - 深度/条数双上限防爆炸，截断如实报告（[`AttackChains::truncated`]）。
    pub fn attack_chains(
        &self,
        target_repo: &str,
        target: NodeId,
        max_chains: usize,
        max_depth: usize,
    ) -> AttackChains {
        let mut out = AttackChains {
            chains: Vec::new(),
            truncated: false,
        };
        let Some(j) = self.get(target_repo) else {
            return out;
        };
        if target.0 as usize >= j.dag().nodes().len() {
            return out;
        }
        let mut nodes: Vec<(String, NodeId)> = vec![(target_repo.to_string(), target)];
        let mut edges: Vec<ChainVia> = vec![ChainVia::Source];
        let mut visited: std::collections::BTreeSet<(String, u32)> =
            std::collections::BTreeSet::new();
        Self::walk(
            self,
            target_repo,
            target,
            true,
            &mut nodes,
            &mut edges,
            &mut visited,
            &mut out,
            max_chains,
            max_depth,
        );
        out
    }

    /// DFS 反向走图：`nodes/edges` 是从 target 到当前的路径（`edges[0]` 为
    /// 起点标记）。`first` = 当前节点是 target（无条件展开）。
    #[allow(clippy::too_many_arguments)]
    fn walk(
        fleet: &Fleet,
        repo: &str,
        id: NodeId,
        first: bool,
        nodes: &mut Vec<(String, NodeId)>,
        edges: &mut Vec<ChainVia>,
        visited: &mut std::collections::BTreeSet<(String, u32)>,
        out: &mut AttackChains,
        max_chains: usize,
        depth_left: usize,
    ) {
        if !visited.insert((repo.to_string(), id.0)) {
            out.truncated = true; // 环：如实截断
            return;
        }
        let j = fleet.get(repo).expect("walk：仓必须存在");
        let dag = j.dag();
        let verdict = dag.node(id).verdict;

        // 源判定：自身 -1（target 自身 -1 也是源——没有已知上游注入）
        if !first && verdict == Trit::Vulnerable {
            // 条数预算：耗尽则如实截断，不再产生新链
            if out.chains.len() >= max_chains {
                out.truncated = true;
                visited.remove(&(repo.to_string(), id.0));
                return;
            }
            // 攻击方向序列 = [n_k(源), n_{k-1}, …, n_0(target)]；进入 n_i 的边
            // = edges[i+1]（edges[j] 连接 n_{j-1} 与 n_j，即逆走第 j 步用的边）
            let k = nodes.len() - 1;
            let mut steps: Vec<ChainStep> = Vec::with_capacity(nodes.len());
            steps.push(ChainStep {
                node: nodes[k].clone(),
                via: ChainVia::Source,
            });
            for i in (0..k).rev() {
                steps.push(ChainStep {
                    node: nodes[i].clone(),
                    via: edges[i + 1],
                });
            }
            out.chains.push(AttackChain { steps });
            if out.chains.len() >= max_chains {
                out.truncated = true;
            }
            visited.remove(&(repo.to_string(), id.0));
            return;
        }
        // 自身 +1（非 target）挡路：污染没穿过它，上游不属打穿路径
        if !first && verdict == Trit::Hardened {
            visited.remove(&(repo.to_string(), id.0));
            return;
        }
        // 深度耗尽：如实截断
        if depth_left == 0 {
            out.truncated = true;
            visited.remove(&(repo.to_string(), id.0));
            return;
        }

        // 展开上游：① 因果链 parent；② 锚定攻击分支（叶子源）；③ 确认穿透跨仓边
        let n = dag.node(id);
        if let Some(p) = n.parent {
            nodes.push((repo.to_string(), p));
            edges.push(ChainVia::Causal);
            Self::walk(fleet, repo, p, false, nodes, edges, visited, out, max_chains, depth_left - 1);
            nodes.pop();
            edges.pop();
        }
        for a in dag.anchored_attacks(id) {
            if dag.node(a).verdict == Trit::Vulnerable {
                if out.chains.len() >= max_chains {
                    out.truncated = true;
                    break;
                }
                // 攻击分支 a 是叶子源，打进锚点 n_k = 路径末端：
                // 序列 = [a(源), n_k(锚点), …, n_0(target)]；进入锚点 = AnchoredAttack，
                // 其余同 causal 构造（进入 n_i = edges[i+1]）
                let k = nodes.len() - 1;
                let mut steps: Vec<ChainStep> = Vec::with_capacity(nodes.len() + 1);
                steps.push(ChainStep {
                    node: (repo.to_string(), a),
                    via: ChainVia::Source,
                });
                for i in (0..=k).rev() {
                    let via = if i == k { ChainVia::AnchoredAttack } else { edges[i + 1] };
                    steps.push(ChainStep {
                        node: nodes[i].clone(),
                        via,
                    });
                }
                out.chains.push(AttackChain { steps });
                if out.chains.len() >= max_chains {
                    out.truncated = true;
                }
            }
        }
        for l in &fleet.cross_links {
            if l.to.0 == repo && l.to.1 == id && l.pass == Trit::Vulnerable
                && fleet.get(&l.from.0).is_some() {
                    nodes.push(l.from.clone());
                    edges.push(ChainVia::CrossLink);
                    Self::walk(
                        fleet,
                        &l.from.0,
                        l.from.1,
                        false,
                        nodes,
                        edges,
                        visited,
                        out,
                        max_chains,
                        depth_left - 1,
                    );
                    nodes.pop();
                    edges.pop();
                }
        }
        visited.remove(&(repo.to_string(), id.0));
    }
}

/// 攻击链的一步：节点 + **进入该节点的方式**（首步 = 源标记 [`ChainVia::Source`]；
/// target 步骤的 via = 进入 target 的边——攻击方向上最后一跳）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainStep {
    pub node: (String, NodeId),
    pub via: ChainVia,
}

/// 进入节点的方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainVia {
    /// 源（确认 `-1`，链的起点）。
    Source,
    /// 仓内因果链边（parent → child）。
    Causal,
    /// 锚定攻击分支打穿锚点。
    AnchoredAttack,
    /// 跨仓确认穿透边。
    CrossLink,
}

/// 一条完整攻击链：源 → … → target（顺序即攻击方向）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttackChain {
    pub steps: Vec<ChainStep>,
}

/// 枚举结果：链集合 + 截断标志（深度/条数/环任一触发即如实报告）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttackChains {
    pub chains: Vec<AttackChain>,
    pub truncated: bool,
}

/// 一条跨仓落账：源端 (仓, 节点) → 目标端 (仓, 节点)，落账裁决。
#[derive(Debug, Clone, PartialEq)]
pub struct CrossRepoDelta {
    pub from: (String, NodeId),
    pub to: (String, NodeId),
    pub verdict: Trit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeId;
    use std::fs;

    #[test]
    fn independent_repos_aggregate_without_merging() {
        let pa = std::env::temp_dir().join(format!("fleet-a-{}.journal", std::process::id()));
        let pb = std::env::temp_dir().join(format!("fleet-b-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);

        let mut fleet = Fleet::new();
        let mut a = Journal::create_as(&pa, "main", 0).unwrap();
        let f = a.spawn(NodeId(0), crate::NodeKind::Delta, "A 功能").unwrap();
        a.record(f, crate::Trit::Hardened, None, "通过").unwrap();
        a.into_dag().unwrap();
        let mut b = Journal::create_as(&pb, "main", 1).unwrap();
        let _ = b.spawn(NodeId(0), crate::NodeKind::Delta, "B 功能").unwrap();
        b.into_dag().unwrap();

        // 从文件重开后入队
        fleet.add("alpha", Journal::open_as(&pa, 0).unwrap()).unwrap();
        fleet.add("beta", Journal::open_as(&pb, 1).unwrap()).unwrap();
        // 重名拒绝
        let pd = std::env::temp_dir().join(format!("fleet-dup-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&pd);
        let jdup = Journal::create_as(&pd, "main", 0).unwrap();
        assert!(fleet.add("alpha", jdup).is_err(), "重名拒绝");
        let _ = std::fs::remove_file(&pd);

        let rows = fleet.report();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "alpha");
        assert_eq!(rows[0].hardened, 2, "root + A 功能");
        assert_eq!(rows[1].suspects, 1, "B 功能未测即 0");

        let rendered = fleet.render();
        assert!(rendered.contains("仓库 alpha"));
        assert!(rendered.contains("仓库 beta"));
        assert!(rendered.contains("A 功能"));
        assert!(rendered.contains("B 功能"));

        let _ = fs::remove_file(&pa);
        let _ = fs::remove_file(&pb);
    }

    #[test]
    fn clone_replicas_converge_through_fleet_sync() {
        let pa = std::env::temp_dir().join(format!("fleet-c-a-{}.journal", std::process::id()));
        let pb = std::env::temp_dir().join(format!("fleet-c-b-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);

        let mut a = Journal::create_as(&pa, "main", 0).unwrap();
        let login = a.spawn(NodeId(0), crate::NodeKind::Delta, "登录").unwrap();
        a.record(login, crate::Trit::Hardened, None, "通过").unwrap();
        a.into_dag().unwrap();

        std::fs::copy(&pa, &pb).unwrap();
        let mut b = Journal::open_as(&pb, 1).unwrap();
        let pay = b.spawn(NodeId(0), crate::NodeKind::Delta, "支付").unwrap();
        b.record(pay, crate::Trit::Hardened, None, "通过").unwrap();
        b.into_dag().unwrap();

        let mut fleet = Fleet::new();
        fleet.add("a", Journal::open_as(&pa, 0).unwrap()).unwrap();
        fleet.add("b", Journal::open_as(&pb, 1).unwrap()).unwrap();
        fleet.sync_pair("a", "b").expect("反熵");

        assert_eq!(
            std::fs::read_to_string(&pa).unwrap(),
            std::fs::read_to_string(&pb).unwrap(),
            "经舰队同步后双方日志逐字节一致"
        );
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);
    }

    #[test]
    fn cross_repo_taint_flows_and_blocked_edges_do_not() {
        let pa = std::env::temp_dir().join(format!("fleet-x-a-{}.journal", std::process::id()));
        let pb = std::env::temp_dir().join(format!("fleet-x-b-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);

        // 仓 a：登录被证伪；仓 b：支付自身 +1
        let mut a = Journal::create_as(&pa, "main", 0).unwrap();
        let login = a.spawn(NodeId(0), crate::NodeKind::Delta, "登录").unwrap();
        let atk = a.spawn(login, crate::NodeKind::AttackVector, "SQLi").unwrap();
        a.record(atk, crate::Trit::Vulnerable, None, "打穿").unwrap();
        a.record(login, crate::Trit::Vulnerable, None, "面已证伪").unwrap();
        a.into_dag().unwrap();

        let mut b = Journal::create_as(&pb, "main", 1).unwrap();
        let pay = b.spawn(NodeId(0), crate::NodeKind::Delta, "支付").unwrap();
        b.record(pay, crate::Trit::Hardened, None, "自身通过").unwrap();
        b.into_dag().unwrap();

        let mut fleet = Fleet::new();
        fleet.add("auth", Journal::open_as(&pa, 0).unwrap()).unwrap();
        fleet.add("pay", Journal::open_as(&pb, 1).unwrap()).unwrap();

        // 未声明跨仓边：B 不受 A 影响
        assert_eq!(
            fleet.effective_verdicts("pay").unwrap()[1],
            crate::Trit::Hardened
        );

        // 声明"支付复用登录会话解析"（确认可穿透）→ -1 传染
        fleet
            .add_cross_link(
                ("auth", NodeId(1)),
                ("pay", NodeId(1)),
                crate::Trit::Vulnerable,
                "支付复用登录会话解析",
            )
            .unwrap();
        assert_eq!(
            fleet.effective_verdicts("pay").unwrap()[1],
            crate::Trit::Vulnerable,
            "上游 -1 沿跨仓边传染"
        );

        // 换成已证拦截的边 → 不传染
        let mut fleet2 = Fleet::new();
        fleet2.add("auth", Journal::open_as(&pa, 0).unwrap()).unwrap();
        fleet2.add("pay", Journal::open_as(&pb, 1).unwrap()).unwrap();
        fleet2
            .add_cross_link(
                ("auth", NodeId(1)),
                ("pay", NodeId(1)),
                crate::Trit::Hardened,
                "已加 WAF",
            )
            .unwrap();
        assert_eq!(
            fleet2.effective_verdicts("pay").unwrap()[1],
            crate::Trit::Hardened,
            "拦截边不传播"
        );

        let _ = fs::remove_file(&pa);
        let _ = fs::remove_file(&pb);
    }

    #[test]
    fn cross_repo_links_persist_in_ledger() {
        let pa = std::env::temp_dir().join(format!("fleet-p-a-{}.journal", std::process::id()));
        let pb = std::env::temp_dir().join(format!("fleet-p-b-{}.journal", std::process::id()));
        let ledger = std::env::temp_dir().join(format!("fleet-p-{}.links", std::process::id()));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);
        let _ = std::fs::remove_file(&ledger);

        let mut a = Journal::create_as(&pa, "main", 0).unwrap();
        let _ = a.spawn(NodeId(0), crate::NodeKind::Delta, "登录").unwrap();
        a.into_dag().unwrap();
        let mut b = Journal::create_as(&pb, "main", 1).unwrap();
        let pay = b.spawn(NodeId(0), crate::NodeKind::Delta, "支付").unwrap();
        b.record(pay, crate::Trit::Hardened, None, "通过").unwrap();
        b.into_dag().unwrap();

        // 注册表入账（账外无数据）
        let mut fleet = Fleet::new();
        fleet.add("auth", Journal::open_as(&pa, 0).unwrap()).unwrap();
        fleet.add("pay", Journal::open_as(&pb, 1).unwrap()).unwrap();
        fleet.open_link_ledger(&ledger);
        fleet
            .add_cross_link(
                ("auth", NodeId(1)),
                ("pay", NodeId(1)),
                crate::Trit::Unknown,
                "疑似\t复用\n含特殊字符",
            )
            .unwrap();
        assert_eq!(fleet.cross_links().len(), 1);

        // 新舰队从注册表重载 → 污染复现
        let mut fleet2 = Fleet::new();
        fleet2.add("auth", Journal::open_as(&pa, 0).unwrap()).unwrap();
        fleet2.add("pay", Journal::open_as(&pb, 1).unwrap()).unwrap();
        let (loaded, skipped) = fleet2.load_links(&ledger).unwrap();
        assert_eq!((loaded, skipped), (1, 0));
        // 可疑边 → 0 强度传染：支付被拉成 0
        assert_eq!(
            fleet2.effective_verdicts("pay").unwrap()[1],
            crate::Trit::Unknown
        );
        assert_eq!(fleet2.render(), fleet.render(), "重载后聚合视图一致");

        let _ = fs::remove_file(&pa);
        let _ = fs::remove_file(&pb);
        let _ = fs::remove_file(&ledger);
    }
}
