//! 本地 HTTP 服务器：把免疫网络暴露成服务（std 线程 + RwLock，零依赖）。
//!
//! 并发模型（"AI 高频操作"的答案在架构上）：
//! - **读多写少**：render/gate/suspects 走读锁并行；deltas（合并采纳）与
//!   scan（校准入账）走写锁串行——单写者纪律在服务边界的体现；
//! - **高频多写者**不在一个 journal 上抢锁，而是各开副本（各自写者 id）、
//!   经 `/v1/deltas` 反熵合并——合并内核与输入顺序无关（有测试钉死）；
//! - 安全边界：服务默认绑 127.0.0.1（监听 ≠ 出网请求）；本服务自身不发起
//!   URL 请求；任何未来出网一律走 [`crate::net::check_url`]。

#[cfg(test)]
use crate::fleet::Fleet;
use crate::journal::Journal;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, RwLock};

const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 16 * 1024 * 1024;

/// 共享的免疫网络服务。写端点需 `.with_psk(k)` 配置共享密钥（否则写端点默认禁用）；
/// 读端点公开。
#[derive(Debug, Clone)]
pub struct Server {
    pub journal: Arc<RwLock<Journal>>,
    psk: Option<Arc<[u8]>>,
}

impl Server {
    pub fn new(journal: Journal) -> Self {
        Self {
            journal: Arc::new(RwLock::new(journal)),
            psk: None,
        }
    }

    pub fn from_shared(journal: Arc<RwLock<Journal>>) -> Self {
        Self { journal, psk: None }
    }

    /// 配置写端点 PSK：写请求必须带 `Authorization: Bearer <hex(psk)>`（审计 H1）。
    pub fn with_psk(mut self, key: impl Into<Vec<u8>>) -> Self {
        self.psk = Some(Arc::from(key.into()));
        self
    }

    /// 阻塞服务循环（每连接一线程，读多写少 + 写端点认证）。返回实际绑定地址。
    pub fn serve(&self, addr: SocketAddr) -> io::Result<SocketAddr> {
        http::serve(addr, self.psk.clone(), Arc::clone(&self.journal), route)
    }
}

mod auth;
pub(crate) mod http;
mod route;
use route::route;

/// 一次 HTTP 请求解析结果：方法 / 路径 / 查询对 / 请求头 / 请求体。
type ParsedRequest = (
    String,
    String,
    Vec<(String, String)>,
    Vec<(String, String)>,
    String,
);

pub(crate) fn read_request(stream: &mut TcpStream) -> io::Result<ParsedRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "请求头过大"));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "连接提前关闭"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut content_length = 0usize;
    let mut headers: Vec<(String, String)> = Vec::new();
    for h in lines {
        if let Some((k, v)) = h.split_once(':') {
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    if content_length > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "请求体过大"));
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    let body = String::from_utf8_lossy(&body).to_string();
    let (path, query_str) = target.split_once('?').unwrap_or((&target, ""));
    let query: Vec<(String, String)> = query_str
        .split('&')
        .filter(|kv| !kv.is_empty())
        .filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    Ok((method, path.to_string(), query, headers, body))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub(crate) fn format_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// fleet 层 HTTP 面已拆至 [`crate::fleet_server`]（上游 module budget：
/// 单文件 ≤800 行）——类型路径经此 re-export 保持不变。
#[cfg(feature = "full")]
pub use crate::fleet_server::FleetServer;

/// 极简 HTTP 客户端（测试与脚本用；返回 (状态码, 响应体)）。
pub mod client {
    use super::*;

    pub fn get(addr: SocketAddr, path: &str) -> io::Result<(u16, String)> {
        request_auth(addr, "GET", path, "", None)
    }

    pub fn post(addr: SocketAddr, path: &str, body: &str) -> io::Result<(u16, String)> {
        request_auth(addr, "POST", path, body, None)
    }

    /// 带写端点 PSK 的 POST：写端点须 `Authorization: Bearer <hex(psk)>`。
    pub fn post_auth(
        addr: SocketAddr,
        path: &str,
        psk: &[u8],
        body: &str,
    ) -> io::Result<(u16, String)> {
        request_auth(addr, "POST", path, body, Some(psk))
    }

    pub fn request(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: &str,
    ) -> io::Result<(u16, String)> {
        request_auth(addr, method, path, body, None)
    }

    fn request_auth(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: &str,
        psk: Option<&[u8]>,
    ) -> io::Result<(u16, String)> {
        let mut s = TcpStream::connect(addr)?;
        let auth = psk
            .map(|k| format!("Authorization: Bearer {}\r\n", super::auth::to_hex(k)))
            .unwrap_or_default();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: server\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes())?;
        let mut resp = Vec::new();
        s.read_to_end(&mut resp)?;
        let text = String::from_utf8_lossy(&resp).to_string();
        let mut lines = text.splitn(2, "\r\n\r\n");
        let head = lines.next().unwrap_or("");
        let body = lines.next().unwrap_or("").to_string();
        let status = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "响应无状态行"))?;
        Ok((status, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet_server::route_fleet;

    fn temp_journal() -> Journal {
        let p =
            std::env::temp_dir().join(format!("trit-immune-srv-t-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Journal::create_as(&p, "t", 0).unwrap()
    }

    /// fleet 端点测试用的两仓舰队（各自 temp journal，上游 -1 / 下游 +1）。
    fn temp_fleet() -> Fleet {
        use crate::{NodeId, NodeKind, Trit};
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let (pa, pb) = (
            dir.join(format!(
                "trit-fleet-srv-a-{}-{n}.journal",
                std::process::id()
            )),
            dir.join(format!(
                "trit-fleet-srv-b-{}-{n}.journal",
                std::process::id()
            )),
        );
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);
        let mut a = Journal::create_as(&pa, "up", 0).unwrap();
        let na = a.spawn(NodeId(0), NodeKind::Delta, "上游节点").unwrap();
        a.record(na, Trit::Vulnerable, None, "确认打穿").unwrap();
        drop(a.into_dag().unwrap());
        let mut b = Journal::create_as(&pb, "down", 1).unwrap();
        let nb = b.spawn(NodeId(0), NodeKind::Delta, "下游节点").unwrap();
        b.record(nb, Trit::Hardened, None, "自身通过").unwrap();
        drop(b.into_dag().unwrap());
        let mut f = Fleet::new();
        f.add("up", Journal::open_as(&pa, 0).unwrap()).unwrap();
        f.add("down", Journal::open_as(&pb, 1).unwrap()).unwrap();
        f
    }

    #[test]
    fn fleet_route_read_endpoints() {
        let f = temp_fleet();
        let shared = std::sync::RwLock::new(f);
        let (s, body) =
            crate::fleet_server::route_fleet(&shared, "GET", "/v1/fleet/health", &[], "");
        assert_eq!((s, body.trim()), (200, "ok"));
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/repos", &[], "");
        assert_eq!(s, 200);
        assert!(body.contains("up") && body.contains("down"), "{body}");
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/board", &[], "");
        assert_eq!(s, 200);
        assert!(body.starts_with("name\ttotal\thardened"), "{body}");
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/render", &[], "");
        assert_eq!(s, 200);
        assert!(!body.is_empty());
        // verdicts：缺参 400；有参返回表
        let (s, _) = route_fleet(&shared, "GET", "/v1/fleet/verdicts", &[], "");
        assert_eq!(s, 400, "缺 repo");
        let q = vec![("repo".to_string(), "down".to_string())];
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/verdicts", &q, "");
        assert_eq!(s, 200);
        assert!(body.contains("node\tverdict"), "{body}");
        let q2 = vec![
            ("repo".to_string(), "down".to_string()),
            ("node".to_string(), "1".to_string()),
        ];
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/verdict", &q2, "");
        assert_eq!((s, body.trim()), (200, "+1"));
        // 405 / 404
        let (s, _) = route_fleet(&shared, "POST", "/v1/fleet/board", &[], "");
        assert_eq!(s, 405);
        let (s, _) = route_fleet(&shared, "GET", "/v1/fleet/nope", &[], "");
        assert_eq!(s, 404);
    }

    #[test]
    fn fleet_route_attacks_endpoint() {
        let f = temp_fleet();
        let shared = std::sync::RwLock::new(f);
        // 声明确认穿透边：up#1(-1) → down#1(+1)
        let (s, _) = route_fleet(
            &shared,
            "POST",
            "/v1/fleet/links",
            &[],
            "up\t1\tdown\t1\t-1\t穿透\n",
        );
        assert_eq!(s, 200);
        let q = vec![
            ("repo".to_string(), "down".to_string()),
            ("node".to_string(), "1".to_string()),
        ];
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/attacks", &q, "");
        assert_eq!(s, 200, "{body}");
        assert!(body.starts_with("chains=1 truncated=false"), "{body}");
        assert!(
            body.contains("\tsource\n") && body.contains("\tcrosslink\n"),
            "{body}"
        );
        // 缺参 400
        let (s, _) = route_fleet(&shared, "GET", "/v1/fleet/attacks", &[], "");
        assert_eq!(s, 400);
        // 自定义上限
        let q2 = vec![
            ("repo".to_string(), "down".to_string()),
            ("node".to_string(), "1".to_string()),
            ("max_chains".to_string(), "1".to_string()),
        ];
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/attacks", &q2, "");
        assert_eq!(s, 200);
        assert!(body.starts_with("chains=1"), "{body}");
    }

    #[test]
    fn fleet_route_write_and_sync_end_to_end() {
        let f = temp_fleet();
        let shared = std::sync::RwLock::new(f);

        // 声明跨仓边：up#1 → down#1，确认穿透
        let (s, body) = route_fleet(
            &shared,
            "POST",
            "/v1/fleet/links",
            &[],
            "up\t1\tdown\t1\t-1\t确认穿透\n",
        );
        assert_eq!((s, body.trim()), (200, "links=1"));
        // 坏行拒绝
        let (s, _) = route_fleet(&shared, "POST", "/v1/fleet/links", &[], "垃圾行\n");
        assert_eq!(s, 400);

        // sync：-1 沿边落账
        let (s, body) = route_fleet(&shared, "POST", "/v1/fleet/sync", &[], "端到端反熵");
        assert_eq!(s, 200, "{body}");
        assert!(body.starts_with("landed=1"), "{body}");

        // 幂等：再 sync 零落账
        let (s, body) = route_fleet(&shared, "POST", "/v1/fleet/sync", &[], "");
        assert_eq!(s, 200);
        assert!(body.starts_with("landed=0"), "{body}");

        // 落账事实可见：down#1 现在 -1
        let q = vec![
            ("repo".to_string(), "down".to_string()),
            ("node".to_string(), "1".to_string()),
        ];
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/verdict", &q, "");
        assert_eq!((s, body.trim()), (200, "-1"));

        // record 原语：人工复核复原
        let (s, body) = route_fleet(
            &shared,
            "POST",
            "/v1/fleet/record",
            &[],
            "down\t1\t+1\t人工复核通过",
        );
        assert_eq!((s, body.trim()), (200, "recorded down#1=+1"));
        let (s, body) = route_fleet(&shared, "GET", "/v1/fleet/verdict", &q, "");
        assert_eq!((s, body.trim()), (200, "+1"));
    }

    #[test]
    fn fleet_server_tcp_smoke() {
        use crate::server::client;
        let f = temp_fleet();
        let psk = b"fleet-psk";
        let srv = FleetServer::new(f).with_psk(psk);
        let addr = srv
            .serve(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap();
        // 读端点无需认证
        let (s, body) = client::get(addr, "/v1/fleet/health").unwrap();
        assert_eq!((s, body.trim()), (200, "ok"));
        // 写端点须带 PSK
        let (s, _) =
            client::post_auth(addr, "/v1/fleet/links", psk, "up\t1\tdown\t1\t-1\t穿透\n").unwrap();
        assert_eq!(s, 200);
        let (s, body) = client::post_auth(addr, "/v1/fleet/sync", psk, "tcp 反熵").unwrap();
        assert_eq!(s, 200);
        assert!(body.starts_with("landed=1"), "{body}");
    }

    #[test]
    fn write_endpoints_require_psk() {
        use crate::server::client;
        let lo = std::net::SocketAddr::from(([127, 0, 0, 1], 0));

        // H5：FleetServer 未配 PSK ⇒ 写端点 403（默认禁用）；读端点仍 200
        let srv = FleetServer::new(temp_fleet());
        let addr = srv.serve(lo).unwrap();
        let (s, _) = client::post(addr, "/v1/fleet/links", "up\t1\tdown\t1\t-1\tx\n").unwrap();
        assert_eq!(s, 403, "未配 PSK 的写端点默认禁用");
        let (s, _) = client::get(addr, "/v1/fleet/health").unwrap();
        assert_eq!(s, 200, "读端点不受影响");

        // H1：Server 配了 PSK ⇒ 错 Bearer 401；对的通过；空诊断 400（不能把节点洗成 +1）
        let p = std::env::temp_dir().join(format!("trit-auth-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let j = Journal::create_as(&p, "t", 0).unwrap();
        let srv = Server::new(j).with_psk(b"k1");
        let addr = srv.serve(lo).unwrap();
        let (s, _) = client::get(addr, "/v1/render").unwrap();
        assert_eq!(s, 200, "读端点公开");
        let (s, _) = client::post_auth(addr, "/v1/diag?node=0", b"wrong", "{}").unwrap();
        assert_eq!(s, 401, "错误 Bearer");
        let (s, body) = client::post_auth(addr, "/v1/diag?node=0", b"k1", "{}").unwrap();
        assert_eq!(s, 400, "空诊断须拒绝而非 +1：{body}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn routing_status_codes() {
        let j = RwLock::new(temp_journal());
        let (status, _) = route(&j, "GET", "/v1/health", &[], "");
        assert_eq!(status, 200);
        let (status, _) = route(&j, "DELETE", "/v1/health", &[], "");
        assert_eq!(status, 405);
        let (status, _) = route(&j, "GET", "/v1/nope", &[], "");
        assert_eq!(status, 404);
        let (status, _) = route(&j, "GET", "/v1/gate", &[], "");
        assert_eq!(status, 400, "缺 tip 参数");
    }
}
