#![doc = include_str!("../README.md")]
#![allow(unsafe_op_in_unsafe_fn)]

// SIMD 模块整体是 `#[target_feature] unsafe fn`，内部每条 intrinsic 再套一层
// `unsafe {}` 只会淹没真正需要审查的边界；安全性由 target_feature 与长度
// 前置条件保证，故在 crate 级关闭该 lint。

pub mod bench;
pub mod frame;
pub mod gemv;
pub mod gf3;
pub mod golay;
pub mod interval;
pub mod pack;
pub mod pool;
pub mod quant;
pub mod shape;
pub mod simd;
pub mod trit;
pub use frame::{FrameCode, FrameCodec, FrameReject, FrameResult, decode_line, encode_line};
pub use gemv::{Kernel, TernaryMatrix, TryteMatrix};
pub use gf3::{ExtendedDecode, ExtendedHamming, HammingCode, HammingDecode};
pub use golay::{Golay11, GolayDecode};
pub use interval::Interval;
pub use pack::{Rail, Trytes};
pub use quant::{QuantReport, Scheme};
pub use trit::Trit;
