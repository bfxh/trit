//! fleet 层的 HTTP 面（从 `crate::server` 拆出——上游 module budget：
//! 单文件 ≤800 行）：`FleetServer` + `route_fleet` 纯函数路由。
//!
//! 端点（全部 TSV 文本，与单仓 server 的 `route` 同风格）：
//! - `GET  /v1/fleet/health` — 存活
//! - `GET  /v1/fleet/repos` — 仓名列表（每行一个）
//! - `GET  /v1/fleet/board` — 聚合收敛表（name/total/hardened/ratio/suspects/quarantined）
//! - `GET  /v1/fleet/render` — 聚合流程图（含跨仓边）
//! - `GET  /v1/fleet/links` — 跨仓边列表
//! - `GET  /v1/fleet/verdicts?repo=X` — X 仓有效裁决（自身 ∧ 舰队污染）
//! - `GET  /v1/fleet/verdict?repo=X&node=N` — 单节点当前裁决
//! - `GET  /v1/fleet/attacks?repo=&node=` — 多跳攻击链
//! - `POST /v1/fleet/links` — 声明跨仓边；body 每行 `from_repo\tfrom_node\tto_repo\tto_node\tpass\tnote`
//! - `POST /v1/fleet/record` — 仓内写裁决；body `repo\tnode\tverdict\tnote`
//! - `POST /v1/fleet/sync` — 跨仓 Delta 反熵；body = note（可空）

use crate::fleet::Fleet;
use crate::server::{format_response, read_request};
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, RwLock};

/// 共享的多仓舰队服务（fleet 层的 HTTP 面，与单仓 `crate::server::Server` 平行）。
#[derive(Debug, Clone)]
pub struct FleetServer {
    pub fleet: Arc<RwLock<Fleet>>,
}

impl FleetServer {
    pub fn new(fleet: Fleet) -> Self {
        Self {
            fleet: Arc::new(RwLock::new(fleet)),
        }
    }

    pub fn from_shared(fleet: Arc<RwLock<Fleet>>) -> Self {
        Self { fleet }
    }

    /// 阻塞服务循环（每连接一线程）。返回实际绑定地址。
    pub fn serve(&self, addr: SocketAddr) -> io::Result<SocketAddr> {
        let listener = TcpListener::bind(addr)?;
        let bound = listener.local_addr()?;
        let shared = Arc::clone(&self.fleet);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    let _ = handle_fleet_conn(&mut stream, &shared);
                });
            }
        });
        Ok(bound)
    }
}

fn handle_fleet_conn(stream: &mut TcpStream, fleet: &RwLock<Fleet>) -> io::Result<()> {
    let (method, path, query, body) = read_request(stream)?;
    let (status, text) = route_fleet(fleet, &method, &path, &query, &body);
    let resp = format_response(status, &text);
    stream.write_all(resp.as_bytes())
}

/// fleet 纯函数路由（不绑 TCP，可直接测；server.rs 的测试直接调用）。
pub(crate) fn route_fleet(
    fleet: &RwLock<Fleet>,
    method: &str,
    path: &str,
    query: &[(String, String)],
    body: &str,
) -> (u16, String) {
    match (method, path) {
        ("GET", "/v1/fleet/health") => (200, "ok\n".into()),
        ("GET", "/v1/fleet/repos") => {
            let f = fleet.read().expect("锁中毒");
            (
                200,
                f.names().into_iter().collect::<Vec<_>>().join("\n") + "\n",
            )
        }
        ("GET", "/v1/fleet/board") => {
            let f = fleet.read().expect("锁中毒");
            let mut out = String::from("name\ttotal\thardened\tratio\tsuspects\tquarantined\n");
            for r in f.report() {
                out.push_str(&format!(
                    "{}\t{}\t{}\t{:.3}\t{}\t{}\n",
                    r.name, r.total, r.hardened, r.ratio, r.suspects, r.quarantined
                ));
            }
            (200, out)
        }
        ("GET", "/v1/fleet/render") => (200, fleet.read().expect("锁中毒").render_with_edges()),
        ("GET", "/v1/fleet/attacks") => {
            // 多跳攻击链：?repo=X&node=N[&max_chains=&max_depth=]
            let Some((_, repo)) = query.iter().find(|(k, _)| k == "repo") else {
                return (400, "缺 repo 参数\n".into());
            };
            let Some((_, node)) = query.iter().find(|(k, _)| k == "node") else {
                return (400, "缺 node 参数\n".into());
            };
            let Ok(id) = node.parse::<u32>() else {
                return (400, format!("node 非数字：{node}\n"));
            };
            let max_chains = query
                .iter()
                .find(|(k, _)| k == "max_chains")
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(64);
            let max_depth = query
                .iter()
                .find(|(k, _)| k == "max_depth")
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(8);
            let f = fleet.read().expect("锁中毒");
            let res = f.attack_chains(repo, crate::NodeId(id), max_chains, max_depth);
            let mut out = format!("chains={} truncated={}\n", res.chains.len(), res.truncated);
            out.push_str("step\trepo\tnode\tvia\n");
            for (ci, chain) in res.chains.iter().enumerate() {
                for (si, step) in chain.steps.iter().enumerate() {
                    let via = match step.via {
                        crate::fleet::ChainVia::Source => "source",
                        crate::fleet::ChainVia::Causal => "causal",
                        crate::fleet::ChainVia::AnchoredAttack => "attack",
                        crate::fleet::ChainVia::CrossLink => "crosslink",
                    };
                    out.push_str(&format!(
                        "{}:{}\t{}\t{}\t{}\n",
                        ci, si, step.node.0, step.node.1 .0, via
                    ));
                }
            }
            (200, out)
        }
        ("GET", "/v1/fleet/links") => {
            let f = fleet.read().expect("锁中毒");
            let mut out = String::from("from_repo\tfrom_node\tto_repo\tto_node\tpass\tnote\n");
            for l in &f.cross_edges() {
                out.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\n",
                    l.from.0,
                    l.from.1 .0,
                    l.to.0,
                    l.to.1 .0,
                    l.pass.to_i8(),
                    l.note
                ));
            }
            (200, out)
        }
        ("GET", "/v1/fleet/verdicts") => {
            let Some((_, repo)) = query.iter().find(|(k, _)| k == "repo") else {
                return (400, "缺 repo 参数\n".into());
            };
            let f = fleet.read().expect("锁中毒");
            match f.effective_verdicts(repo) {
                Ok(vs) => {
                    let mut out = String::from("node\tverdict\n");
                    for (i, v) in vs.iter().enumerate() {
                        out.push_str(&format!("{}\t{}\n", i, v.to_i8()));
                    }
                    (200, out)
                }
                Err(e) => (404, format!("{e}\n")),
            }
        }
        ("GET", "/v1/fleet/verdict") => {
            let Some((_, repo)) = query.iter().find(|(k, _)| k == "repo") else {
                return (400, "缺 repo 参数\n".into());
            };
            let Some((_, node)) = query.iter().find(|(k, _)| k == "node") else {
                return (400, "缺 node 参数\n".into());
            };
            let Ok(id) = node.parse::<u32>() else {
                return (400, format!("node 非数字：{node}\n"));
            };
            let f = fleet.read().expect("锁中毒");
            match f.repo_verdict(repo, crate::NodeId(id)) {
                Ok(v) => (200, format!("{v}\n")),
                Err(e) => (404, format!("{e}\n")),
            }
        }
        ("POST", "/v1/fleet/links") => {
            let mut f = fleet.write().expect("锁中毒");
            let (mut ok, _bad) = (0usize, 0usize);
            for (i, line) in body.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = line.split('\t').collect();
                let Some((fr, fid, tr, tid, pass)) = (|| {
                    Some((
                        parts.first()?.to_string(),
                        parts.get(1)?.parse::<u32>().ok()?,
                        parts.get(2)?.to_string(),
                        parts.get(3)?.parse::<u32>().ok()?,
                        parts.get(4)?.parse::<crate::Trit>().ok()?,
                    ))
                })() else {
                    return (400, format!("第 {} 行不合法（应为 6 列 TSV）\n", i + 1));
                };
                let note = parts.get(5).copied().unwrap_or("");
                match f.add_cross_link(
                    (fr.as_str(), crate::NodeId(fid)),
                    (tr.as_str(), crate::NodeId(tid)),
                    pass,
                    note,
                ) {
                    Ok(()) => ok += 1,
                    Err(e) => return (400, format!("第 {} 行拒绝：{e}\n", i + 1)),
                }
            }
            (200, format!("links={ok}\n"))
        }
        ("POST", "/v1/fleet/record") => {
            let parts: Vec<&str> = body.trim().split('\t').collect();
            let Some((repo, id, verdict)) = (|| {
                Some((
                    parts.first()?.to_string(),
                    parts.get(1)?.parse::<u32>().ok()?,
                    parts.get(2)?.parse::<crate::Trit>().ok()?,
                ))
            })() else {
                return (400, "body 应为 repo\\tnode\\tverdict\\tnote\n".into());
            };
            let note = parts.get(3).copied().unwrap_or("");
            let mut f = fleet.write().expect("锁中毒");
            match f.record_repo(&repo, crate::NodeId(id), verdict, note) {
                Ok(()) => (200, format!("recorded {repo}#{id}={verdict}\n")),
                Err(e) => (400, format!("{e}\n")),
            }
        }
        ("POST", "/v1/fleet/sync") => {
            let note = if body.trim().is_empty() {
                "fleet sync"
            } else {
                body.trim()
            };
            let mut f = fleet.write().expect("锁中毒");
            match f.sync_cross_repo(note) {
                Ok(landed) => {
                    let mut out = format!("landed={}\n", landed.len());
                    out.push_str("from_repo\tfrom_node\tto_repo\tto_node\tverdict\n");
                    for d in &landed {
                        out.push_str(&format!(
                            "{}\t{}\t{}\t{}\t{}\n",
                            d.from.0,
                            d.from.1 .0,
                            d.to.0,
                            d.to.1 .0,
                            d.verdict.to_i8()
                        ));
                    }
                    (200, out)
                }
                Err(e) => (500, format!("同步失败：{e}\n")),
            }
        }
        ("GET", "/v1/fleet/coords") => {
            // 坐标台账：?repo= 可只看一仓（PROV = 裁决出处，USE = 实际用到）
            let f = fleet.read().expect("锁中毒");
            let want = query
                .iter()
                .find(|(k, _)| k == "repo")
                .map(|(_, v)| v.clone());
            let mut out = String::from("role\trepo\tnode\tcoord\n");
            for p in f.provides() {
                if want.as_deref().is_some_and(|r| r != p.repo) {
                    continue;
                }
                out.push_str(&format!(
                    "PROV\t{}\t{}\t{}\n",
                    p.repo,
                    p.node.0,
                    p.coord.key()
                ));
            }
            for u in f.uses() {
                if want.as_deref().is_some_and(|r| r != u.repo) {
                    continue;
                }
                out.push_str(&format!(
                    "USE\t{}\t{}\t{}\n",
                    u.repo,
                    u.node.0,
                    u.coord.key()
                ));
            }
            (200, out)
        }
        ("GET", "/v1/fleet/gaps") => {
            // 坐标缺口：不匹配不静默——每一条都是需要人/智能体补的 0 态候选
            let f = fleet.read().expect("锁中毒");
            let mut out = String::from("repo\tnode\tcoord\treason\tnote\n");
            for g in f.coord_gaps() {
                out.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\n",
                    g.consumer,
                    g.node.0,
                    g.used.key(),
                    reason_str(g.reason),
                    crate::delta::esc(&g.note())
                ));
            }
            (200, out)
        }
        ("POST", "/v1/fleet/coord-declare") => {
            // body = 每行 `repo\tnode\tcoord`（coord 用 CoordKey::key() 的规范化形态）
            let mut f = fleet.write().expect("锁中毒");
            let mut ok = 0usize;
            for (i, line) in body.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = line.split('\t').collect();
                let (Some(repo), Some(node), Some(k)) = (parts.first(), parts.get(1), parts.get(2))
                else {
                    return (
                        400,
                        format!("第 {} 行不合法（应为 repo\\tnode\\tcoord）\n", i + 1),
                    );
                };
                let Ok(id) = node.parse::<u32>() else {
                    return (400, format!("第 {} 行 node 非数字：{node}\n", i + 1));
                };
                let Some(coord) = crate::coords::CoordKey::from_key(k) else {
                    return (400, format!("第 {} 行坐标键不合法：{k:?}\n", i + 1));
                };
                match f.declare_provider(repo, crate::NodeId(id), coord) {
                    Ok(()) => ok += 1,
                    Err(e) => return (400, format!("第 {} 行拒绝：{e}\n", i + 1)),
                }
            }
            (200, format!("providers={ok}\n"))
        }
        ("POST", "/v1/fleet/lock") => {
            // ?repo=&build= ；body = 原始 Cargo.lock 文本 → 坐标入账（幂等）
            let Some((_, repo)) = query.iter().find(|(k, _)| k == "repo") else {
                return (400, "缺 repo 参数\n".into());
            };
            let build = query
                .iter()
                .find(|(k, _)| k == "build")
                .map(|(_, v)| v.clone());
            let mut f = fleet.write().expect("锁中毒");
            match f.attach_lock(repo, body, build.as_deref()) {
                Ok(r) => {
                    let mut out = format!("created={}\treused={}\n", r.created.len(), r.reused);
                    out.push_str("node\tcoord\n");
                    for (n, c) in &r.created {
                        out.push_str(&format!("{}\t{}\n", n.0, c.key()));
                    }
                    (200, out)
                }
                Err(e) => (400, format!("{e}\n")),
            }
        }
        ("POST", "/v1/fleet/coord-sync") => {
            let note = if body.trim().is_empty() {
                "coord sync"
            } else {
                body.trim()
            };
            let mut f = fleet.write().expect("锁中毒");
            match f.sync_coordinates(note) {
                Ok(landed) => {
                    let mut out = format!("landed={}\n", landed.len());
                    out.push_str("from_repo\tfrom_node\tto_repo\tto_node\tverdict\n");
                    for d in &landed {
                        out.push_str(&format!(
                            "{}\t{}\t{}\t{}\t{}\n",
                            d.from.0,
                            d.from.1 .0,
                            d.to.0,
                            d.to.1 .0,
                            d.verdict.to_i8()
                        ));
                    }
                    (200, out)
                }
                Err(e) => (500, format!("坐标反熵失败：{e}\n")),
            }
        }
        (m, p) if p.starts_with("/v1/fleet/") => {
            // 已知端点但方法不对 → 405；未知路径 → 404
            const KNOWN: [&str; 15] = [
                "/v1/fleet/health",
                "/v1/fleet/repos",
                "/v1/fleet/board",
                "/v1/fleet/render",
                "/v1/fleet/attacks",
                "/v1/fleet/links",
                "/v1/fleet/coords",
                "/v1/fleet/gaps",
                "/v1/fleet/verdicts",
                "/v1/fleet/verdict",
                "/v1/fleet/record",
                "/v1/fleet/sync",
                "/v1/fleet/coord-declare",
                "/v1/fleet/lock",
                "/v1/fleet/coord-sync",
            ];
            if KNOWN.contains(&p) {
                (405, format!("方法 {m} 不适用于 {p}\n"))
            } else {
                (404, "未知端点（fleet 端点在 /v1/fleet/ 下）\n".into())
            }
        }
        _ => (404, "未知端点（fleet 端点在 /v1/fleet/ 下）\n".into()),
    }
}

/// 缺口类型的线名（HTTP 输出用，与 [`crate::coords::GapReason`] 一一对应）。
fn reason_str(r: crate::coords::GapReason) -> &'static str {
    use crate::coords::GapReason as G;
    match r {
        G::NoProvider => "no-provider",
        G::SourceMismatch => "source-mismatch",
        G::DigestMismatch => "digest-mismatch",
        G::BuildMismatch => "build-mismatch",
        G::MissingNode => "missing-node",
    }
}
