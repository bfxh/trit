//! 网络面：pcap → 双向五元组流 → 三值判决；攻击侧是三值攻击图。
//!
//! 检测链路是 `pcap::Reader` → `packet::decode` → `flow::FlowTable` →
//! `features::ternarize` → `decision::decide`。每一层都能单独喂合成夹具，
//! 跳过的报文一律计数，不存在「悄悄丢掉」。
//!
//! 攻击侧 [`attack`]：节点与边都带证据等级（`T` 已证实 / `X` 待验证 / `F`
//! 已排除），路径折叠走 K3 AND——**没有一条已证实结论能从未验证的环节得出**。
//! [`probe`] 是证据生产端：allowlist 默认拒绝、只对实验室目标（环回/私网）的
//! 受控 TCP 探测，结果直接是带 `source` 的证据行。
//!
//! 为什么放在 `trit-net` 里而不是独立 crate：`scripts/publish_graph.py` 禁止
//! 实验层之间互相依赖（实验层 → 实验层 = 红），而三值解析 [`crate::analyze`]
//! 只在这里有。

pub mod attack;
pub mod calibrate;
pub mod dataset;
pub mod decision;
pub mod drift;
pub mod features;
pub mod flow;
pub mod packet;
pub mod pcap;
pub mod probe;
