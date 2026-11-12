//! HTTP 监听与连接处理（从 `server`/`fleet_server` 抽出的共用层，控制行数预算）：
//! 每连接一线程，**先做写端点 PSK 认证**，再交给纯函数路由表。

use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, RwLock};

use super::auth::authorize_write;
use super::conn;
use super::{format_response, read_request};

/// 起监听循环（每连接一线程）。`route` 是纯函数路由表（单仓的 `route` 或 fleet 的
/// `route_fleet`）。返回实际绑定地址。
pub(crate) fn serve<S, F>(
    addr: SocketAddr,
    psk: Option<Arc<[u8]>>,
    state: Arc<RwLock<S>>,
    route: F,
) -> io::Result<SocketAddr>
where
    S: Send + Sync + 'static,
    F: Fn(&RwLock<S>, &str, &str, &[(String, String)], &str) -> (u16, String)
        + Clone
        + Send
        + Sync
        + 'static,
{
    let listener = TcpListener::bind(addr)?;
    let bound = listener.local_addr()?;
    conn::serve(listener, move |mut stream: TcpStream| {
        let _ = handle(&mut stream, &state, psk.as_deref(), &route);
    });
    Ok(bound)
}

fn handle<S, F>(
    stream: &mut TcpStream,
    state: &RwLock<S>,
    psk: Option<&[u8]>,
    route: &F,
) -> io::Result<()>
where
    F: Fn(&RwLock<S>, &str, &str, &[(String, String)], &str) -> (u16, String),
{
    let (method, path, query, headers, body) = read_request(stream)?;
    let (status, text) = match authorize_write(psk, &method, &path, &headers) {
        Err((s, t)) => (s, t),
        Ok(()) => route(state, &method, &path, &query, &body),
    };
    stream.write_all(format_response(status, &text).as_bytes())
}
