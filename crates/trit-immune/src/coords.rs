//! 依赖坐标：共享库在"多智能体 + 多项目"下的身份与消费边（ROADMAP #49）。
//!
//! 问题是**扇出**：一个被许多项目依赖的库判 `-1`，若按"仓 → 仓"声明的粗边传播，
//! 会把只用 `lib 2.0` 的项目也打成红（假红一大片），真受影响的那几个反而被平均掉。
//!
//! 本模块把依赖关系下沉到**坐标**粒度：
//!
//! - [`CoordKey`] = `name@version` + `source` + `digest`（+ 可选 `build` 后缀）。
//!   `source` 分开了 dependency confusion（同名同版本换了来源就不是同一个产物），
//!   `digest` 取 `Cargo.lock` 的 checksum；`build` 是给 feature 统一/构建参数留的口子——
//!   **`Cargo.lock` 不记录 feature 集**，所以调用方要把清单指纹（`supply::fingerprint`）
//!   当 build 传进来，否则同 checksum 不同 feature 的两个产物会被判成同坐标；
//! - 消费方用 [`Fleet::attach_lock`] 把自己锁文件里的坐标**入账**成节点
//!   （`Message` 节点、0 态、幂等复用），提供方用 [`Fleet::declare_provider`]
//!   声明"哪个节点代表这个坐标的裁决"；
//! - [`derive_edges`] 只在坐标**逐字段相等**时连边（`pass = -1`，确认可穿透），
//!   连出来的边就是 [`crate::fleet::CrossRepoLink`]——于是 #33 的落账纪律
//!   （**变坏才落账、`+1` 永不跨仓落账**）、(min,max) 查询语义、攻击链枚举
//!   全部原样复用，不另起一套语义；
//! - **不匹配不静默**：同 `name@version` 但 source/digest/build 对不上、或根本没有
//!   提供者的，一律进 [`CoordGap`] 清单（0 态候选）——把边悄悄丢掉就是 fail-open。
//!
//! 账本纪律照旧：坐标声明是追加式注册表（`PROV`/`USE` 行），账外无数据；
//! 加载期不验证（与 `Fleet::load_links` 同一口径），失效声明在 [`CoordGap`] 里如实上报。

use crate::dag::NodeId;
use crate::delta::{esc, unesc};
use crate::fleet::{CrossRepoDelta, CrossRepoLink, Fleet};
use crate::supply::Pin;
use crate::trit::Trit;
use std::io;

/// 一个依赖坐标（身份 = 全部字段，缺一律判"不同坐标"）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoordKey {
    pub name: String,
    pub version: String,
    /// `registry+...` / `git+...#rev`；path/workspace 成员为 None。
    pub source: Option<String>,
    /// `Cargo.lock` 的 checksum；无 checksum 来源为 None（**未证产物身份**）。
    pub digest: Option<String>,
    /// 调用方补充的构建维度指纹（feature 集等 lock 文件不记录的东西）。
    pub build: Option<String>,
}

impl CoordKey {
    /// 从锁文件条目建坐标；`build` 由调用方按项目实际 feature/构建参数给。
    pub fn from_pin(pin: &Pin, build: Option<&str>) -> Self {
        Self {
            name: pin.name.clone(),
            version: pin.version.clone(),
            source: pin.source.clone(),
            digest: pin.checksum.clone(),
            build: build.map(|s| s.to_string()),
        }
    }

    /// 规范化键（账本与去重的主键）：缺字段用 `-` 占位，永不歧义。
    pub fn key(&self) -> String {
        format!(
            "{}@{}|{}|{}|{}",
            self.name,
            self.version,
            self.source.as_deref().unwrap_or("-"),
            self.digest.as_deref().unwrap_or("-"),
            self.build.as_deref().unwrap_or("-"),
        )
    }

    /// [`CoordKey::key`] 的逆（HTTP/CLI 输入用）。宽容：形状不符即 None，不 panic。
    ///
    /// 名称里允许出现 `|`（真实 crate 名不会），所以按**最后三个** `|` 切分，
    /// 版本段也不允许含 `|`；`-` 还原为 None。
    pub fn from_key(s: &str) -> Option<CoordKey> {
        let (name, rest) = s.split_once('@')?;
        if name.is_empty() {
            return None;
        }
        let parts: Vec<&str> = rest.split('|').collect();
        if parts.len() != 4 {
            return None;
        }
        let opt = |v: &str| {
            if v == "-" {
                None
            } else {
                Some(v.to_string())
            }
        };
        Some(CoordKey {
            name: name.to_string(),
            version: parts[0].to_string(),
            source: opt(parts[1]),
            digest: opt(parts[2]),
            build: opt(parts[3]),
        })
    }

    /// 家族键（`name@version`）：缺口报告用它说明"同一个库的坐标对不上"。
    pub fn family(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// 入账节点标题（人类可读，幂等查找走 [`CoordKey::key`]）。
    pub fn title(&self) -> String {
        format!("COORD {}", self.key())
    }
}

/// 一条坐标声明：某仓某节点代表这个坐标（提供 = 裁决出处，消费 = 本项目实际用到）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordDecl {
    pub repo: String,
    pub node: NodeId,
    pub coord: CoordKey,
}

/// 坐标缺口的类型——每一种都**不落账、只上报**，绝不静默连边或静默丢边。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    /// 没有任何仓声明提供这个 `name@version`。
    NoProvider,
    /// 同 `name@version`，来源不同（crates.io ↔ 内网/git）——confusion 形态。
    SourceMismatch,
    /// 同 `name@version`、同来源，checksum 不同——投毒或不同产物。
    DigestMismatch,
    /// 同坐标但 build 维度不同（feature 集/构建参数）——未证同一个产物。
    BuildMismatch,
    /// 账本声明指向不存在的仓/节点（陈旧或跨机导入的注册表行）。
    MissingNode,
}

/// 一处坐标缺口：消费者用了什么、提供者有什么、差在哪一维。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordGap {
    pub consumer: String,
    pub node: NodeId,
    pub used: CoordKey,
    /// 同家族但不同坐标的提供者（`NoProvider`/`MissingNode` 时为 None）。
    pub near: Option<CoordKey>,
    pub reason: GapReason,
}

impl CoordGap {
    /// 0 态候选的说明文案（进 suspects / 报告用）。
    pub fn note(&self) -> String {
        match self.reason {
            GapReason::NoProvider => format!("无提供者：{}", self.used.family()),
            GapReason::SourceMismatch => format!(
                "来源不一致：用 {}，提供者 {}",
                dim(&self.used.source),
                dim(&self.near.as_ref().and_then(|k| k.source.clone()))
            ),
            GapReason::DigestMismatch => format!(
                "摘要不一致：用 {}，提供者 {}",
                dim(&self.used.digest),
                dim(&self.near.as_ref().and_then(|k| k.digest.clone()))
            ),
            GapReason::BuildMismatch => format!(
                "build 维度不一致：用 {}，提供者 {}",
                dim(&self.used.build),
                dim(&self.near.as_ref().and_then(|k| k.build.clone()))
            ),
            GapReason::MissingNode => format!("声明指向不存在的节点：{}", self.used.key()),
        }
    }
}

fn dim(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("-")
}

/// 声明一条提供者（写入 `provides`，坐标已存在则只增一条声明）。
///
/// 仓必须已在舰队里；节点越界照收（加载期不验证），由 [`gaps`] 如实上报。
pub fn declare_provider(
    fleet: &mut Fleet,
    repo: &str,
    node: NodeId,
    coord: CoordKey,
) -> io::Result<()> {
    if fleet.get(repo).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("无仓 {repo}"),
        ));
    }
    if let Some(path) = fleet.coord_ledger_path() {
        let line = encode_line(Role::Provider, repo, node, &coord);
        append_line(&path, &line).map_err(io::Error::other)?;
    }
    fleet.push_provider(CoordDecl {
        repo: repo.to_string(),
        node,
        coord,
    });
    Ok(())
}

/// 把一份 `Cargo.lock` 的坐标入账到某仓的 DAG：每个坐标一个 `Message` 节点
/// （0 态——外部事实入账，作者声称≠已验证），**幂等**：同键复用既有节点。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttachReport {
    /// 新建的坐标节点
    pub created: Vec<(NodeId, CoordKey)>,
    /// 复用既有节点（第二遍跑应当全部落在这里）
    pub reused: usize,
    /// 全部 pin 对应的 (节点, 坐标)——新建与复用都在里面，按锁文件顺序
    pub all: Vec<(NodeId, CoordKey)>,
}

/// **共用入账路径**：把 pins 写进一个仓的 journal（按标题幂等）。
/// [`attach_lock`]（舰队形态）与 `trit lock`（单仓 CLI）都走这里——
/// 两处若各写一套，节点建法与消费边语义迟早分叉。
pub fn attach_pins(
    j: &mut crate::journal::Journal,
    pins: &[Pin],
    build: Option<&str>,
) -> io::Result<AttachReport> {
    let mut report = AttachReport::default();
    for pin in pins {
        let coord = CoordKey::from_pin(pin, build);
        let title = coord.title();
        let existing = j
            .dag()
            .nodes()
            .iter()
            .find(|n| n.title == title)
            .map(|n| n.id);
        if let Some(node) = existing {
            report.reused += 1;
            report.all.push((node, coord));
            continue;
        }
        let node = j
            .spawn(NodeId(0), crate::dag::NodeKind::Message, &title)
            .map_err(io::Error::other)?;
        // 消费即依赖：坐标节点 → 本仓根（pass=-1）。没有这条边，库被打穿
        // 只孤立地记在坐标节点上，本仓的门禁/有效裁决看不见它。
        j.cross_link(node, NodeId(0), Trit::Vulnerable, &format!("消费 {title}"))
            .map_err(io::Error::other)?;
        report.created.push((node, coord.clone()));
        report.all.push((node, coord));
    }
    Ok(report)
}

/// 把一份 `Cargo.lock` 的坐标入账到某仓（舰队形态）：建节点走 [`attach_pins`]
/// 同一条路径，再把 (仓, 坐标) → 节点 的声明登记进舰队与坐标注册表。
pub fn attach_lock(
    fleet: &mut Fleet,
    repo: &str,
    lock_text: &str,
    build: Option<&str>,
) -> io::Result<AttachReport> {
    if fleet.get(repo).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("无仓 {repo}"),
        ));
    }
    let pins = crate::supply::parse_lock(lock_text);
    let report = {
        let j = fleet.get_mut(repo).expect("上面已验存在");
        attach_pins(j, &pins, build)?
    };
    if let Some(path) = fleet.coord_ledger_path() {
        for (node, coord) in &report.created {
            let line = encode_line(Role::Use, repo, *node, coord);
            append_line(&path, &line).map_err(io::Error::other)?;
        }
    }
    for (node, coord) in &report.all {
        fleet.adopt_use(repo, *node, coord);
    }
    Ok(report)
}

/// 派生消费边：**只有坐标逐字段相等才连边**，`pass = -1`（确认可穿透）。
///
/// 同仓的提供/消费不连边（自己影响自己是因果链的事）；越界节点跳过，
/// 由 [`gaps`] 报 `MissingNode`。输出按 (from,to) 稳定排序，两遍跑逐位相同。
pub fn derive_edges(provides: &[CoordDecl], uses: &[CoordDecl]) -> Vec<CrossRepoLink> {
    let mut out: Vec<CrossRepoLink> = Vec::new();
    for u in uses {
        for p in provides {
            if p.repo == u.repo || p.coord.key() != u.coord.key() {
                continue;
            }
            out.push(CrossRepoLink {
                from: (p.repo.clone(), p.node),
                to: (u.repo.clone(), u.node),
                pass: Trit::Vulnerable,
                note: format!("coord {}", u.coord.key()),
            });
        }
    }
    out.sort_by(|a, b| {
        (&a.from, a.to.1 .0)
            .cmp(&(&b.from, b.to.1 .0))
            .then_with(|| a.to.0.cmp(&b.to.0))
    });
    out.dedup_by(|a, b| a.from == b.from && a.to == b.to);
    out
}

/// 缺口清单：对每条消费声明，说明"为什么这条边没连上"。
pub fn gaps(fleet: &Fleet, provides: &[CoordDecl], uses: &[CoordDecl]) -> Vec<CoordGap> {
    let mut out = Vec::new();
    for u in uses {
        let Some(dag) = fleet.get(&u.repo).map(|j| j.dag()) else {
            out.push(CoordGap {
                consumer: u.repo.clone(),
                node: u.node,
                used: u.coord.clone(),
                near: None,
                reason: GapReason::MissingNode,
            });
            continue;
        };
        if u.node.0 as usize >= dag.nodes().len() {
            out.push(CoordGap {
                consumer: u.repo.clone(),
                node: u.node,
                used: u.coord.clone(),
                near: None,
                reason: GapReason::MissingNode,
            });
            continue;
        }
        let exact = provides
            .iter()
            .any(|p| p.repo != u.repo && p.coord.key() == u.coord.key() && node_ok(fleet, p));
        if exact {
            continue;
        }
        let near = provides
            .iter()
            .find(|p| p.repo != u.repo && p.coord.family() == u.coord.family());
        let reason = match near {
            None => GapReason::NoProvider,
            Some(p) => {
                if p.coord.source != u.coord.source {
                    GapReason::SourceMismatch
                } else if p.coord.digest != u.coord.digest {
                    GapReason::DigestMismatch
                } else if p.coord.build != u.coord.build {
                    GapReason::BuildMismatch
                } else {
                    // 坐标相同但提供者节点不可用——归到 MissingNode，仍不静默
                    GapReason::MissingNode
                }
            }
        };
        out.push(CoordGap {
            consumer: u.repo.clone(),
            node: u.node,
            used: u.coord.clone(),
            near: near.map(|p| p.coord.clone()),
            reason,
        });
    }
    out.sort_by(|a, b| {
        (&a.consumer, a.node.0, &a.used.key()).cmp(&(&b.consumer, b.node.0, &b.used.key()))
    });
    out
}

fn node_ok(fleet: &Fleet, d: &CoordDecl) -> bool {
    fleet
        .get(&d.repo)
        .map(|j| (d.node.0 as usize) < j.dag().nodes().len())
        .unwrap_or(false)
}

/// 声明的角色（账本第一列之后的动词）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Provider,
    Use,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Provider => "PROV",
            Role::Use => "USE",
        }
    }
}

/// 编码一条坐标声明（追加式注册表行，9 列）：
/// `COORD\t<PROV|USE>\t<仓>\t<节点>\t<name>\t<version>\t<source>\t<digest>\t<build>`。
/// 每列独立走 `delta::esc`（crate 内部），故行内永不出现制表符/换行，缺字段以 `-` 占位。
pub fn encode_line(role: Role, repo: &str, node: NodeId, coord: &CoordKey) -> String {
    format!(
        "COORD\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        role.as_str(),
        esc(repo),
        node.0,
        esc(&coord.name),
        esc(&coord.version),
        esc(coord.source.as_deref().unwrap_or("-")),
        esc(coord.digest.as_deref().unwrap_or("-")),
        esc(coord.build.as_deref().unwrap_or("-")),
    )
}

/// 解析注册表行（[`encode_line`] 的逆，宽容：列数不符/转义坏 → None，不 panic）。
pub fn parse_line(line: &str) -> Option<(Role, CoordDecl)> {
    let mut f = line.split('\t');
    let (
        Some("COORD"),
        Some(role),
        Some(repo),
        Some(node),
        Some(name),
        Some(version),
        Some(source),
        Some(digest),
        Some(build),
        None,
    ) = (
        f.next(),
        f.next(),
        f.next(),
        f.next(),
        f.next(),
        f.next(),
        f.next(),
        f.next(),
        f.next(),
        f.next(),
    )
    else {
        return None;
    };
    let role = match role {
        "PROV" => Role::Provider,
        "USE" => Role::Use,
        _ => return None,
    };
    let node = node.parse::<u32>().ok()?;
    let repo = unesc(repo).ok()?;
    let name = unesc(name).ok()?;
    let version = unesc(version).ok()?;
    let source = unesc(source).ok()?;
    let digest = unesc(digest).ok()?;
    let build = unesc(build).ok()?;
    let opt = |s: String| if s == "-" { None } else { Some(s) };
    Some((
        role,
        CoordDecl {
            repo,
            node: NodeId(node),
            coord: CoordKey {
                name,
                version,
                source: opt(source),
                digest: opt(digest),
                build: opt(build),
            },
        },
    ))
}

/// 追加一行到坐标注册表（账外无数据：声明必须落盘）。
fn append_line(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// 从注册表读取全部坐标声明（坏行如实计数返回，不静默丢）。
pub fn load_lines(path: &std::path::Path) -> io::Result<(Vec<CoordDecl>, Vec<CoordDecl>, usize)> {
    let text = std::fs::read_to_string(path)?;
    let (mut provides, mut uses, mut bad) = (Vec::new(), Vec::new(), 0usize);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match parse_line(line) {
            Some((Role::Provider, d)) => provides.push(d),
            Some((Role::Use, d)) => uses.push(d),
            None => bad += 1,
        }
    }
    Ok((provides, uses, bad))
}

/// **坐标级跨仓落账**：沿 [`derive_edges`] 的精确匹配边，把提供方当前裁决落到
/// 消费者仓自己的坐标节点上（`Delta::Record` 进消费者的 journal）。
///
/// 纪律与 [`Fleet::sync_cross_repo`] 逐字一致——这里是同一套语义的第二个入口，
/// 不是第二套规则：
/// - **变坏才落账**（天然幂等，可反复跑）；
/// - **`+1` 永不跨仓落账**（下游的 +1 必须自己挣：库绿 ≠ 应用绿）；
/// - 越界/未知仓的声明跳过，由 [`gaps`] 如实上报。
///
/// 坐标边 `pass` 恒为 `-1`（确证穿透），故落账值 = 提供方裁决本身。
pub fn sync(fleet: &mut Fleet, note: &str) -> io::Result<Vec<CrossRepoDelta>> {
    let edges = derive_edges(fleet.provides(), fleet.uses());
    let mut plan: Vec<(String, NodeId, String, NodeId, Trit)> = Vec::new();
    for e in &edges {
        let Some(src) = fleet.get(&e.from.0) else {
            continue;
        };
        if e.from.1 .0 as usize >= src.dag().nodes().len() {
            continue;
        }
        let sv = src.dag().node(e.from.1).verdict;
        let out = sv.max(e.pass);
        // 注：`+1 永不跨仓落账` 由下面的"变坏才落账"蕴含（Hardened 是序的上界），
        // 不再单列分支——单列即死代码，见 tests/coords.rs 的同名用例。
        let Some(dst) = fleet.get(&e.to.0) else {
            continue;
        };
        if e.to.1 .0 as usize >= dst.dag().nodes().len() {
            continue;
        }
        let dv = dst.dag().node(e.to.1).verdict;
        if out >= dv {
            continue; // 变坏才落账
        }
        plan.push((e.from.0.clone(), e.from.1, e.to.0.clone(), e.to.1, out));
    }
    let mut results = Vec::with_capacity(plan.len());
    for (fr, fid, tr, tid, v) in plan {
        let full_note = format!("{note} ← coord {}:{}", fr, fid.0);
        if let Some(j) = fleet.get_mut(&tr) {
            j.record(tid, v, None, &full_note)
                .map_err(io::Error::other)?;
        }
        results.push(CrossRepoDelta {
            from: (fr, fid),
            to: (tr, tid),
            verdict: v,
        });
    }
    Ok(results)
}

/// 舰队坐标视图（TSV，供 HTTP/CLI 直读）：提供者、消费者、缺口三段。
pub fn render(fleet: &Fleet) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for p in fleet.provides() {
        let _ = writeln!(s, "PROV\t{}\t{}\t{}", p.repo, p.node.0, p.coord.key());
    }
    for u in fleet.uses() {
        let _ = writeln!(s, "USE\t{}\t{}\t{}", u.repo, u.node.0, u.coord.key());
    }
    for g in fleet.coord_gaps() {
        let _ = writeln!(
            s,
            "GAP\t{}\t{}\t{}\t{:?}",
            g.consumer,
            g.node.0,
            g.used.key(),
            g.reason
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(name: &str, ver: &str, src: Option<&str>, sum: Option<&str>) -> CoordKey {
        CoordKey {
            name: name.into(),
            version: ver.into(),
            source: src.map(|s| s.into()),
            digest: sum.map(|s| s.into()),
            build: None,
        }
    }

    fn decl(repo: &str, node: u32, c: CoordKey) -> CoordDecl {
        CoordDecl {
            repo: repo.into(),
            node: NodeId(node),
            coord: c,
        }
    }

    #[test]
    fn line_round_trip_covers_all_field_shapes() {
        for c in [
            coord("a", "1.0.0", Some("registry+x"), Some("aa")),
            coord("a", "1.0.0", None, None),
            coord(
                "a-b",
                "1.0.0",
                Some("git+https://x?rev=1#deadbeef"),
                Some("ff"),
            ),
        ] {
            for role in [Role::Provider, Role::Use] {
                let line = encode_line(role, "my/repo", NodeId(7), &c);
                let (r, d) = parse_line(&line).expect("round trip");
                assert_eq!(r, role);
                assert_eq!(d.repo, "my/repo");
                assert_eq!(d.node, NodeId(7));
                assert_eq!(d.coord, c, "缺字段必须以 - 占位还原");
            }
        }
    }

    #[test]
    fn key_round_trip_via_from_key() {
        for c in [
            coord("a", "1.0.0", Some("r"), Some("aa")),
            coord("a", "1.0.0", None, None),
            coord("a", "1.0.0", Some("git+x#dead"), None),
        ] {
            assert_eq!(
                CoordKey::from_key(&c.key()).as_ref(),
                Some(&c),
                "键往返必须无损"
            );
        }
        // 形状不符一律 None（不 panic、不猜）
        for bad in [
            "",
            "a",
            "a@1",
            "a@1|2",
            "a@1|2|3",
            "@1|2|3|4",
            "a@1|2|3|4|5",
        ] {
            assert!(
                CoordKey::from_key(bad).is_none(),
                "坏键必须判 None：{bad:?}"
            );
        }
    }

    #[test]
    fn parse_line_rejects_malformed_without_panic() {
        for bad in [
            "",
            "COORD",
            "FLEETLINK\tPay\t0\tWeb\t1\t-1\tx",
            "COORD\tPROV\tPay\tX\tname\tver\t-\t-\t-",
            "COORD\tWAT\tPay\t1\tname\tver\t-\t-\t-",
            "COORD\tPROV\tPay\t1\tn\tv\t-\t-",
        ] {
            assert!(parse_line(bad).is_none(), "坏行必须判 None：{bad:?}");
        }
    }

    #[test]
    fn derive_edges_only_for_exact_key_match_and_never_self() {
        let provides = vec![decl("lib", 3, coord("a", "1.0.0", Some("r"), Some("aa")))];
        let uses = vec![
            decl("lib", 9, coord("a", "1.0.0", Some("r"), Some("aa"))), // 同仓：不连
            decl("app", 2, coord("a", "1.0.0", Some("r"), Some("aa"))), // 精确同坐标：连
            decl("app", 4, coord("a", "2.0.0", Some("r"), Some("bb"))), // 版本不同：不连
            decl("web", 5, coord("a", "1.0.0", Some("r"), Some("cc"))), // 摘要不同：不连
        ];
        let edges = derive_edges(&provides, &uses);
        assert_eq!(edges.len(), 1, "只有逐字段相等的坐标才连边");
        assert_eq!(
            (edges[0].from.0.as_str(), edges[0].to.0.as_str()),
            ("lib".to_string().as_str(), "app".to_string().as_str())
        );
        assert_eq!(edges[0].to.1, NodeId(2));
        assert_eq!(edges[0].pass, Trit::Vulnerable);
        // 确定性：两遍跑逐位相同
        assert_eq!(edges, derive_edges(&provides, &uses));
    }
}
