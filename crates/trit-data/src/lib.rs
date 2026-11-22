//! 智能体轨迹 / 语言数据的**可复算质量门**（M0）。
//!
//! 硬门是确定性规则（schema、可执行性、验证与证据、预算、循环、去重），
//! 软门是显式不确定信号（过程恢复、观察、思考、长度、语言质量）。
//! 决策三支：`accept` / `reject` / `defer`；reject 与 defer 都保留原因码，
//! 服务负样本池与重写闭环。设计口径见 `docs/plans/AGENT-DATA-PLAN.md`。

#![forbid(unsafe_code)]

pub mod gate;
pub mod manifest;
pub mod schema;

pub use gate::{Config, Decision, Verdict, evaluate};
pub use manifest::dataset_manifest;
pub use schema::{Cost, Outcome, Step, Trajectory};
