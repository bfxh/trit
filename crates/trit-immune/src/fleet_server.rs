//! fleet 层的 HTTP 面（从 `crate::server` 拆出——上游 module budget：
//! 单文件 ≤800 行）：`FleetServer` 的监听与连接生命周期。
//!
//! 端点清单与纯函数路由表本体在 `fleet_server/routes.rs`（不绑 TCP、可直接测）。

mod routes;
pub(crate) use routes::route_fleet;

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
