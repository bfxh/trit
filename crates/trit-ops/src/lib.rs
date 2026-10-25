//! 三值推理算子库。
//!
//! 三值的价值不在「2 bit 存得下」，而在**乘法可以完全消失**：
//! 权重只有 `{-1, 0, +1}`，`x * t` 只可能是 `-x`、`0`、`+x`。
//! 本库围绕两条真实可实现的主线组织算子：
//!
//! 1. **双导轨位平面**（每 trit 2 bit）：点积变成 4 组 `AND + popcount`；
//!    与 f32 激活相乘时变成按位掩码累加，不需要任何浮点乘法。
//! 2. **tryte 查表**（每字节 5 个 trit，`3^5 = 243`）：把 5 个 trit 的
//!    加权和预计算成一张 243 项的表，之后每个 tryte 只是一次查表。
//!
//! 所有内核都与标量参考实现交叉验证；性能必须实测，不在此处声明。
//!
//! # 与 `trit-core` 的分工
//!
//! `trit-core::value::Ternary` 是**逻辑**取值 `{False, Unknown, True}`，
//! 服务于 SAT / BDD / 电路。本库是**平衡算术** `{-1, 0, +1}`，服务于
//! 矩阵乘。两者的映射是 `False -> -1`、`Unknown -> 0`、`True -> +1`，
//! 由 [`Trit::from_ternary`] / [`Trit::to_ternary`] 提供，不复用同一个类型。
#![allow(unsafe_op_in_unsafe_fn)]

// SIMD 模块整体是 `#[target_feature] unsafe fn`，内部每条 intrinsic 再套一层
// `unsafe {}` 只会淹没真正需要审查的边界；安全性由 target_feature 与长度
// 前置条件保证，故在 crate 级关闭该 lint。

pub mod bench;
pub mod gemv;
pub mod pack;
pub mod pool;
pub mod quant;
pub mod simd;
pub mod trit;

pub use gemv::{Kernel, TernaryMatrix, TryteMatrix};
pub use pack::{Rail, Trytes};
pub use quant::{QuantReport, Scheme};
pub use trit::Trit;
