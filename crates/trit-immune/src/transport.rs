//! 可插拔传输层：同步管线的 I/O 与内核解耦。
//!
//! 设计原则：**内核不选传输**——调用方选。内核只定义 `Transport` trait
//! 和两个内置实现：
//! - `TcpTransport`：std TCP（可被反向代理包裹做 TLS 终结）；
//! - `MemoryTransport`：进程内直通（测试 / 单机多副本，确定性）。
//!
//! TLS 部署路径：把 TcpTransport 的端口暴露在反向代理（nginx/caddy）后面，
//! 代理终结 TLS，内核只看到明文 TCP + HMAC 认证——**加密是部署关注，不是内核关注**。
//! 这与 DATABASE_URL、REDIS_URL 等基础设施地址的注入模式同构。
//!
//! 未来扩展：Unix domain socket、QUIC、gRPC——都实现同一个 trait，内核不改。

use crate::journal::Journal;
use crate::sync;
use std::io;
use std::net::SocketAddr;

/// 传输层 trait：一对（发送方日志, 接收方合并结果）的 I/O 抽象。
///
/// 实现者负责帧编解码和可靠性；内核只消费 `Vec<Stamped>`。
pub trait Transport: Send {
    /// 把本方日志发给对端，收对端日志帧（文本）。
    fn exchange(&mut self, our_frame: &str) -> io::Result<String>;
}

/// 进程内直通传输：两端共享一个缓冲区，零延迟、确定性（测试用）。
#[derive(Debug, Default)]
pub struct MemoryTransport {
    /// 对端最近一次发来的帧。
    inbox: Option<String>,
}

impl MemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// 模拟对端推送一帧（测试用）。
    pub fn deliver(&mut self, frame: &str) {
        self.inbox = Some(frame.to_string());
    }
}

impl Transport for MemoryTransport {
    fn exchange(&mut self, _our_frame: &str) -> io::Result<String> {
        self.inbox
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "对端未推送"))
    }
}

/// TCP 传输：一问一答（8 字节大端长度 + UTF-8 帧体）。
pub struct TcpTransport {
    pub peer: SocketAddr,
}

impl TcpTransport {
    pub fn new(peer: SocketAddr) -> Self {
        Self { peer }
    }
}

impl Transport for TcpTransport {
    fn exchange(&mut self, our_frame: &str) -> io::Result<String> {
        let mut stream = std::net::TcpStream::connect(self.peer)?;
        crate::sync::tcp::write_frame(&mut stream, our_frame)?;
        crate::sync::tcp::read_frame(&mut stream)
    }
}

/// 把本方日志发给远端并采纳合并结果（走任意传输）。
pub fn pull_merge_via(
    transport: &mut dyn Transport,
    local: &mut Journal,
    auth_key: Option<&[u8]>,
) -> io::Result<usize> {
    let mine = sync::parse_log_file(local.path())?;
    let w = local.clock().writer();
    let frame = sync::encode_frame(w, &mine);
    let frame = match auth_key {
        Some(k) => {
            sync::sign_frame(&frame, k).map_err(io::Error::other)?
        }
        None => frame,
    };
    let resp = transport.exchange(&frame)?;
    let canonical = match auth_key {
        Some(k) => sync::verify_frame(&resp, k),
        None => sync::parse_frame(&resp),
    }
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let n = canonical.stamped.len();
    local.adopt(canonical.stamped)?;
    Ok(n)
}
