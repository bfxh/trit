//! fleet 层的 HTTP 面（从 `crate::server` 拆出——上游 module budget：
//! 单文件 ≤800 行）：`FleetServer` 的监听与连接生命周期。
//!
//! 端点清单与纯函数路由表本体在 `fleet_server/routes.rs`（不绑 TCP、可直接测）。
//! 写端点（record/links/sync/lock）需 `.with_psk(k)` 配置共享密钥（审计 H5）。

mod routes;
pub(crate) use routes::route_fleet;

use crate::fleet::Fleet;
use crate::server::http;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

/// 共享的多仓舰队服务（fleet 层的 HTTP 面，与单仓 `crate::server::Server` 平行）。
#[derive(Debug, Clone)]
pub struct FleetServer {
    pub fleet: Arc<RwLock<Fleet>>,
    psk: Option<Arc<[u8]>>,
}

impl FleetServer {
    pub fn new(fleet: Fleet) -> Self {
        Self {
            fleet: Arc::new(RwLock::new(fleet)),
            psk: None,
        }
    }

    pub fn from_shared(fleet: Arc<RwLock<Fleet>>) -> Self {
        Self { fleet, psk: None }
    }

    /// 配置写端点 PSK：写请求必须带 `Authorization: Bearer <hex(psk)>`（审计 H5）。
    pub fn with_psk(mut self, key: impl Into<Vec<u8>>) -> Self {
        self.psk = Some(Arc::from(key.into()));
        self
    }

    /// 阻塞服务循环（每连接一线程 + 写端点认证）。返回实际绑定地址。
    pub fn serve(&self, addr: SocketAddr) -> io::Result<SocketAddr> {
        http::serve(addr, self.psk.clone(), Arc::clone(&self.fleet), route_fleet)
    }
}
