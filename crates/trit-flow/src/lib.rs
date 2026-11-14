//! 统一生成模型步进层：扩散、漂移（流匹配）与任意自定义场共用一套采样骨架与批量执行器。
//!
//! **它解决的问题**：换一个生成模型（DDIM 扩散 / rectified-flow 漂移场 / 别的什么场），
//! 采样循环、批量组织、延迟读数都要重写一遍——本 crate 把这些抽成三层：
//!
//! 1. [`Field`]：**一步**求速度 `v(x,t|cond)`，批量、每样本自己的 `t` 与条件；
//!    [`X0ToVelocity`] 让「输出 `x̂₀`」的扩散去噪器直接变成速度场（`v = (√ᾱ·x − x̂₀)/√(1−ᾱ)`）；
//! 2. [`Scheme`]：Euler / Midpoint / Heun——DDIM（η=0）就是 Euler，流匹配也是 Euler，
//!    所以「扩散还是漂移」只影响场的实现，不影响积分器；
//! 3. [`Executor`]：常驻线程池 + 按步组批（不同步数的请求同批推进）+ p50/p90/p99 与吞吐读数。
//!
//! **边界（诚实缺口）**：
//! - 不做 HTTP/gRPC 面，不做请求鉴权与超时取消——那是服务层的事，本层只给内核与读数；
//! - 不做 GPU 队列复用：`Field` 是 CPU 上可批量调用的口径，GPU 后端应把一次 `eval` 映射成一次 kernel 启动；
//! - 训练不在本层（`Field` 只前向）；权重从哪来由调用方决定。

#![forbid(unsafe_code)]

pub mod exec;
pub mod field;
pub mod pool;
pub mod solver;
pub mod stats;

pub use exec::{Cfg, Executor, Outcome, Report, Request};
pub use field::{
    Constant, Field, FnField, LinearDrift, MlpField, ShapeError, X0Predictor, X0ToVelocity,
    abar_at, check_batch, velocity_to_x0, x0_to_velocity,
};
pub use pool::Pool;
pub use solver::{Scheme, Scratch, Solver, advance};
pub use stats::{Latency, percentile, summarize};
