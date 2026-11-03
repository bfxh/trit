//! 网络面：pcap → 双向五元组流 → 三值判决；攻击侧是三值攻击图。
//!
//! 检测链路是 `pcap::Reader` → `packet::decode` → `flow::FlowTable` →
//! `features::ternarize` → `decision::decide`。每一层都能单独喂合成夹具，
//! 跳过的报文一律计数，不存在「悄悄丢掉」。
//!
//! 攻击侧见 [`attack`]，证据生产端见 [`probe`]；各自的证据纪律写在自己模块的头注释里。

pub mod attack;
pub mod calibrate;
pub mod dataset;
pub mod decision;
pub mod features;
pub mod flow;
pub mod packet;
pub mod pcap;
pub mod probe;
