//! 集成测试共用件：写端点 PSK + 带认证的写请求 + 起服务器（审计 H1/H5）。
#![allow(dead_code)]

use std::net::SocketAddr;
use trit_immune::*;

/// 写端点 PSK（未配 PSK 时写端点默认禁用）。
pub const PSK: &[u8] = b"test-psk";

/// 带 PSK 的写请求（读仍走 `server::client::get`）。
pub fn post(addr: SocketAddr, path: &str, body: &str) -> std::io::Result<(u16, String)> {
    server::client::post_auth(addr, path, PSK, body)
}

/// 起一个配了 PSK 的单仓服务器，返回地址。
pub fn bind_server(journal: Journal) -> SocketAddr {
    Server::new(journal)
        .with_psk(PSK)
        .serve("127.0.0.1:0".parse().unwrap())
        .expect("起服务器")
}
