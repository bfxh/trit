//! 跨副本 Delta 同步：**stamp 即身份，全序即合并**。
//!
//! 模型（v1：克隆 + 追加）：
//!
//! - 每个副本日志 = 共同前缀（原写者盖戳，克隆而来）+ 本写者盖戳的追加；
//! - 副本时钟在追加前观察过日志（`create_as`/`open_as` 保证），因此共同前缀的戳
//!   在 HLC 全序上先于所有分歧后增量；
//! - 合并 = 去重（stamp 即 Delta 身份）→ 按 HLC 全序 → 逐条把**写者本地 id 改写为
//!   全局 id** → 复用 `ImmuneDag::apply` 同一条代码路径重放；
//! - 第二个及之后的 `ROOT` 跳过（独立建库的副本各有一个根）；
//! - 重放失败的 Delta 进隔离区（不静默丢弃，留人裁）；
//! - 共享节点上的裁决冲突按全序后到者胜（两条历史事件都保留，审计不丢）。
//!
//! 已知边界：某个写者引用"它通过同步见到的、由别的写者新建"的节点时，本地 id
//! 无从改写，会进隔离区——v1 不做跨命名空间引用展开。

use crate::auth::{self, ct_eq, hmac_sha256};
use crate::dag::{ImmuneDag, NodeId};
use crate::delta::{self, Delta, Stamped};
use crate::hlc::Hlc;
use crate::journal::{self, Journal};
use crate::verdict::ExploitEvidence;
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;

/// 重放失败被隔离的 Delta（不静默丢弃，留人裁）。
#[derive(Debug, Clone)]
pub struct DeferredDelta {
    pub ts: Hlc,
    pub delta: Delta,
    pub reason: String,
}

/// 合并结果：收敛后的 DAG + 规范日志（可作为各副本的新日志基线）。
#[derive(Debug, Default)]
pub struct MergeOutcome {
    pub dag: ImmuneDag,
    /// 全序规范日志：已去重、已去重复根、已剔除隔离项，本地 id 已改写为全局 id
    pub canonical: Vec<Stamped>,
    /// 被跳过的重复 `ROOT` 条数（独立建库的副本各带一个根）
    pub skipped_roots: usize,
    pub deferred: Vec<DeferredDelta>,
}

/// 归并多个副本日志，收敛出同一份 DAG 与规范日志。
///
/// 结果与 `replicas` 的输入顺序无关（确定性收敛）。
pub fn merge_replicas(replicas: Vec<(u32, Vec<Stamped>)>) -> MergeOutcome {
    // 1) 去重：stamp 即 Delta 身份（克隆副本共享相同戳），保留首次出现。
    //    BTreeMap 迭代序 = Hlc 全序 = merge_order 的结果。
    let mut uniq: BTreeMap<Hlc, Stamped> = BTreeMap::new();
    for (_, stream) in &replicas {
        for s in stream {
            uniq.entry(s.ts).or_insert_with(|| s.clone());
        }
    }

    // 2) 每个写者的"自有 spawn 本地 id"：在其本副本日志里按序推演——
    //    副本日志里任何 Spawn 都在创建该副本视角下的下一个本地节点。
    let mut own_local: HashMap<u32, Vec<u32>> = HashMap::new();
    for (w, stream) in &replicas {
        let mut local = 1u32; // #0 是根
        let mut list = Vec::new();
        for s in stream {
            if matches!(s.delta, Delta::Spawn { .. }) {
                if s.ts.writer == *w {
                    list.push(local);
                }
                local += 1;
            }
        }
        own_local.insert(*w, list);
    }
    let mut own_global: HashMap<u32, Vec<u32>> = own_local.keys().map(|w| (*w, Vec::new())).collect();

    // 3) 全序重放 + 本地 id → 全局 id 改写
    let mut dag = ImmuneDag::empty();
    let mut outcome = MergeOutcome::default();
    for s in uniq.into_values() {
        let w = s.ts.writer;
        let translated = translate_delta(&own_local, &own_global, w, &s.delta);
        let d = match translated {
            Ok(d) => d,
            Err(reason) => {
                outcome.deferred.push(DeferredDelta {
                    ts: s.ts,
                    delta: s.delta,
                    reason,
                });
                continue;
            }
        };
        match &d {
            Delta::Root { .. } => {
                if dag.nodes().is_empty() {
                    // 空 DAG 上 Root 不会失败
                    let _ = dag.apply(&d);
                    outcome.canonical.push(Stamped { ts: s.ts, delta: d });
                } else {
                    outcome.skipped_roots += 1;
                }
            }
            Delta::Spawn { .. } => {
                let global = dag.nodes().len() as u32;
                if let Some(g) = own_global.get_mut(&w) {
                    g.push(global);
                }
                // Spawn 在合法日志序里不会失败；万一失败按隔离处理
                if dag.apply(&d).is_ok() {
                    outcome.canonical.push(Stamped { ts: s.ts, delta: d });
                } else {
                    outcome.deferred.push(DeferredDelta {
                        ts: s.ts,
                        delta: s.delta,
                        reason: "spawn 重放失败".into(),
                    });
                }
            }
            _ => match dag.apply(&d) {
                Ok(()) => outcome.canonical.push(Stamped { ts: s.ts, delta: d }),
                Err(e) => outcome.deferred.push(DeferredDelta {
                    ts: s.ts,
                    delta: s.delta,
                    reason: e.to_string(),
                }),
            },
        }
    }
    outcome.dag = dag;
    outcome
}

/// 把写者 `w` 本地命名空间里的节点引用改写成全局 id。
/// 共同前缀（早于该写者第一个自有 spawn 的本地 id）恒等映射——前缀按原序最先重放。
fn translate(
    own_local: &HashMap<u32, Vec<u32>>,
    own_global: &HashMap<u32, Vec<u32>>,
    w: u32,
    x: u32,
) -> Result<u32, String> {
    let (locals, globals) = match (own_local.get(&w), own_global.get(&w)) {
        (Some(l), Some(g)) => (l, g),
        _ => return Ok(x), // 未见其日志的写者：按共同前缀处理
    };
    match locals.binary_search(&x) {
        Ok(i) => Ok(globals[i]),
        Err(0) => Ok(x), // 共同前缀
        Err(_) => Err(format!("写者 #{w} 引用了未知本地节点 #{x:02}")),
    }
}

fn translate_delta(
    own_local: &HashMap<u32, Vec<u32>>,
    own_global: &HashMap<u32, Vec<u32>>,
    w: u32,
    delta: &Delta,
) -> Result<Delta, String> {
    Ok(match delta {
        Delta::Root { .. } | Delta::Spawn { .. } => delta.clone(),
        Delta::Record {
            id,
            verdict,
            evidence,
            note,
        } => Delta::Record {
            id: NodeId(translate(own_local, own_global, w, id.0)?),
            verdict: *verdict,
            evidence: match evidence {
                Some(e) => Some(ExploitEvidence {
                    vector: e.vector.clone(),
                    payload: e.payload.clone(),
                    target: NodeId(translate(own_local, own_global, w, e.target.0)?),
                    round: e.round,
                }),
                None => None,
            },
            note: note.clone(),
        },
        Delta::Quarantine { id, note } => Delta::Quarantine {
            id: NodeId(translate(own_local, own_global, w, id.0)?),
            note: note.clone(),
        },
        Delta::HumanReview { id, verdict, note } => Delta::HumanReview {
            id: NodeId(translate(own_local, own_global, w, id.0)?),
            verdict: *verdict,
            note: note.clone(),
        },
        Delta::CrossLink { from, to, pass, note } => Delta::CrossLink {
            from: NodeId(translate(own_local, own_global, w, from.0)?),
            to: NodeId(translate(own_local, own_global, w, to.0)?),
            pass: *pass,
            note: note.clone(),
        },
    })
}

/// 日志传输帧：`trit-immune-log v1` + `WRITER` 行 + 日志行（与 journal v2 行同格式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFrame {
    /// 发送副本自己的写者 id（其日志里自有增量的命名空间）。
    pub writer: u32,
    pub stamped: Vec<Stamped>,
}

pub const LOG_FRAME_HEADER: &str = "trit-immune-log v1";

/// 把本副本日志编码成传输帧。
pub fn encode_frame(writer: u32, stream: &[Stamped]) -> String {
    let mut out = String::new();
    out.push_str(LOG_FRAME_HEADER);
    out.push('\n');
    out.push_str(&format!("WRITER\t{writer}\n"));
    for s in stream {
        out.push_str(&format!("{}\t{}\n", s.ts, delta::encode(&s.delta)));
    }
    out
}

/// 解析传输帧。
pub fn parse_frame(text: &str) -> Result<LogFrame, String> {
    let mut lines = text.lines();
    match lines.next() {
        Some(h) if h.trim_end_matches('\r') == LOG_FRAME_HEADER => {}
        Some(other) => return Err(format!("日志帧头部不合法：{other:?}")),
        None => return Err("空帧".into()),
    }
    let writer_line = lines.next().ok_or("缺少 WRITER 行")?;
    let mut wf = writer_line.split('\t');
    let bad = || "WRITER 行不合法".to_string();
    if wf.next() != Some("WRITER") {
        return Err(bad());
    }
    let writer = wf.next().ok_or_else(bad)?.parse::<u32>().map_err(|_| bad())?;
    if wf.next().is_some() {
        return Err(bad());
    }
    let mut stamped = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let (ts, d) = journal::parse_stamped_line(line)?;
        stamped.push(Stamped { ts, delta: d });
    }
    Ok(LogFrame { writer, stamped })
}

/// 规范化帧文本：按行重建并保证尾随换行（签名与验证走同一路径）。
fn normalize_frame(text: &str) -> String {
    let mut s: String = text.lines().collect::<Vec<_>>().join("\n");
    s.push('\n');
    s
}

/// 给帧加 HMAC-SHA256 签名：在 WRITER 行后插入 `AUTH\t<64 位十六进制>`，
/// 签名覆盖帧体（除 AUTH 行外的全部行，按规范化形式）。
pub fn sign_frame(unsigned: &str, key: &[u8]) -> Result<String, String> {
    if unsigned.lines().any(|l| l.starts_with("AUTH\t")) {
        return Err("帧已含 AUTH 行".into());
    }
    parse_frame(unsigned)?;
    let body = normalize_frame(unsigned);
    let tag = auth::hex(&hmac_sha256(key, body.as_bytes()));
    let mut out = String::new();
    for (i, line) in body.lines().enumerate() {
        out.push_str(line);
        out.push('\n');
        if i == 1 {
            out.push_str(&format!("AUTH\t{tag}\n"));
        }
    }
    Ok(out)
}

/// 验证带签名的帧：HMAC 不匹配 / 缺 AUTH / 结构非法一律拒绝
/// （提供密钥即强制认证——免疫系统最毒的攻击是注入伪造的 `+1` 裁决）。
pub fn verify_frame(signed: &str, key: &[u8]) -> Result<LogFrame, String> {
    let lines: Vec<&str> = signed.lines().collect();
    if lines.len() < 3 {
        return Err("帧不完整（头 + WRITER + AUTH 至少三行）".into());
    }
    let tag = lines[2]
        .strip_prefix("AUTH\t")
        .ok_or("帧缺少 AUTH 行（要求认证）")?;
    let mut body = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i == 2 {
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    let expect = auth::hex(&hmac_sha256(key, body.as_bytes()));
    if !ct_eq(expect.as_bytes(), tag.as_bytes()) {
        return Err("HMAC 校验失败".into());
    }
    parse_frame(&body)
}

/// 从 journal 文件读出本副本日志（跳过头部行）。
pub fn parse_log_file(path: &Path) -> io::Result<Vec<Stamped>> {
    let text = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue; // 头部行 / 空行
        }
        let (ts, d) = journal::parse_stamped_line(line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("第 {} 行：{e}", i + 1)))?;
        out.push(Stamped { ts, delta: d });
    }
    Ok(out)
}

/// 把远端日志并入本副本：合并 → 采纳规范日志。
pub fn merge_into(
    local: &mut Journal,
    remote_writer: u32,
    remote: Vec<Stamped>,
) -> io::Result<MergeOutcome> {
    let mine = parse_log_file(local.path())?;
    let w = local.clock().writer();
    let out = merge_replicas(vec![(w, mine), (remote_writer, remote)]);
    local.adopt(out.canonical.clone())?;
    Ok(out)
}

/// 进程内双向反熵（不经网络）：双方合并彼此日志并各自采纳规范日志。
/// 合并内核确定性 ⇒ 双方收敛到逐字节相同的日志文件。
pub fn anti_entropy(a: &mut Journal, b: &mut Journal) -> io::Result<(MergeOutcome, MergeOutcome)> {
    let la = parse_log_file(a.path())?;
    let lb = parse_log_file(b.path())?;
    let wa = a.clock().writer();
    let wb = b.clock().writer();
    let ma = {
        let out = merge_replicas(vec![(wa, la.clone()), (wb, lb.clone())]);
        a.adopt(out.canonical.clone())?;
        out
    };
    let mb = {
        let out = merge_replicas(vec![(wb, lb), (wa, la)]);
        b.adopt(out.canonical.clone())?;
        out
    };
    Ok((ma, mb))
}

/// TCP 传输（std）。
///
/// 安全边界：只接受字面 `IP:端口`（[`SocketAddr`]），不做域名解析、不请求 URL——
/// 把 SSRF / DNS 重绑定面从协议层排除；对端地址由运维显式配置。
/// 帧格式：8 字节大端长度 + UTF-8 日志帧（上限 64 MiB）。
pub mod tcp {
    use super::*;
    const MAX_FRAME: u64 = 64 * 1024 * 1024;

    pub fn write_frame(stream: &mut TcpStream, text: &str) -> io::Result<()> {
        let bytes = text.as_bytes();
        stream.write_all(&(bytes.len() as u64).to_be_bytes())?;
        stream.write_all(bytes)
    }

    pub fn read_frame(stream: &mut TcpStream) -> io::Result<String> {
        let mut buf = [0u8; 8];
        stream.read_exact(&mut buf)?;
        let len = u64::from_be_bytes(buf);
        if len > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "帧超过 64 MiB 上限"));
        }
        let mut body = vec![0u8; len as usize];
        stream.read_exact(&mut body)?;
        String::from_utf8(body).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "帧非 UTF-8"))
    }

    /// 客户端（无认证）。
    pub fn pull_merge(addr: SocketAddr, local: &mut Journal) -> io::Result<usize> {
        pull_merge_with(addr, local, None)
    }

    /// 客户端（可选 PSK 帧认证：HMAC-SHA256，见 [`crate::auth`]）。
    pub fn pull_merge_with(
        addr: SocketAddr,
        local: &mut Journal,
        auth_key: Option<&[u8]>,
    ) -> io::Result<usize> {
        let mine = parse_log_file(local.path())?;
        let w = local.clock().writer();
        let mut stream = TcpStream::connect(addr)?;
        let frame = encode_frame(w, &mine);
        let frame = match auth_key {
            Some(k) => {
                sign_frame(&frame, k).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            }
            None => frame,
        };
        write_frame(&mut stream, &frame)?;
        let resp = read_frame(&mut stream)?;
        let canonical = match auth_key {
            Some(k) => verify_frame(&resp, k),
            None => parse_frame(&resp),
        }
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let n = canonical.stamped.len();
        local.adopt(canonical.stamped)?;
        Ok(n)
    }

    /// 服务端（无认证）。
    pub fn serve_merge(listener: TcpListener, local: &mut Journal) -> io::Result<MergeOutcome> {
        serve_merge_with(listener, local, None)
    }

    /// 服务端（可选 PSK 帧认证：HMAC-SHA256，见 [`crate::auth`]）。
    pub fn serve_merge_with(
        listener: TcpListener,
        local: &mut Journal,
        auth_key: Option<&[u8]>,
    ) -> io::Result<MergeOutcome> {
        let (mut stream, _) = listener.accept()?;
        let frame = read_frame(&mut stream)?;
        let remote = match auth_key {
            Some(k) => verify_frame(&frame, k),
            None => parse_frame(&frame),
        }
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mine = parse_log_file(local.path())?;
        let w = local.clock().writer();
        let out = merge_replicas(vec![(w, mine), (remote.writer, remote.stamped)]);
        let reply = encode_frame(w, &out.canonical);
        let reply = match auth_key {
            Some(k) => {
                sign_frame(&reply, k).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            }
            None => reply,
        };
        write_frame(&mut stream, &reply)?;
        local.adopt(out.canonical.clone())?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
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
            sd(0, 100, 0, Delta::Root { title: "main".into() }),
            sd(0, 101, 0, Delta::Spawn { parent: NodeId(0), kind: crate::NodeKind::Delta, title: "A 模块".into() }),
            sd(0, 102, 0, Delta::Record { id: NodeId(1), verdict: Trit::Vulnerable, evidence: None, note: "A 打穿自己的模块".into() }),
        ];
        let s1 = vec![
            sd(1, 200, 0, Delta::Root { title: "main".into() }),
            sd(1, 201, 0, Delta::Spawn { parent: NodeId(0), kind: crate::NodeKind::Delta, title: "B 模块".into() }),
            sd(1, 202, 0, Delta::Record { id: NodeId(1), verdict: Trit::Hardened, evidence: None, note: "B 放行自己的模块".into() }),
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
        let s0 = vec![sd(0, 50, 0, Delta::Root { title: "main".into() })];
        let s1 = vec![
            sd(1, 100, 0, Delta::Root { title: "main".into() }),
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
        assert!(m.deferred[0].reason.contains("未知本地节点") || m.deferred[0].reason.contains("不存在"));
        assert_eq!(m.dag.nodes().len(), 1);
        assert!(m
            .canonical
            .iter()
            .all(|s| !matches!(s.delta, Delta::Record { .. })));
    }

    #[test]
    fn frame_roundtrip() {
        let stamps = vec![
            sd(0, 100, 0, Delta::Root { title: "main\t带制表".into() }),
            sd(1, 200, 3, Delta::Spawn {
                parent: NodeId(0),
                kind: crate::NodeKind::Message,
                title: "标题\n含换行".into(),
            }),
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
        let stamps = vec![sd(0, 100, 0, Delta::Root { title: "main".into() })];
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
}
