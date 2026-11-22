//! pgwire 远程查询：启动前物化一次，服务期间按间隔刷新。

#[path = "pgwire_serve.rs"]
mod serve;
pub use serve::serve_trace;
