//! 网络面：pcap → 双向五元组流 → 三值判决。
//!
//! 链路是 `pcap::Reader` → `packet::decode` → `flow::FlowTable` →
//! `features::ternarize` → `decision::decide`。每一层都能单独喂合成夹具，
//! 跳过的报文一律计数，不存在「悄悄丢掉」。
//!
//! 为什么放在 `trit-net` 里而不是独立 crate：`scripts/publish_graph.py` 禁止
//! 实验层之间互相依赖（实验层 → 实验层 = 红），而三值解析 [`crate::analyze`]
//! 只在这里有。

pub mod calibrate;
pub mod dataset;
pub mod decision;
pub mod features;
pub mod flow;
pub mod packet;
pub mod pcap;
