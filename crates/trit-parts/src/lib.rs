//! 部件平面头：k 位三进制 + valid 的 Rust 三进制 GPU 实现。
//!
//! **`publish = false`，不属于稳定 API。** GPU 部分在 `gpu` feature 后面，
//! 默认构建不拉 `wgpu`。
//!
//! 本 crate 是「模型归 trit 仓」的落点：权重加载、BN 折叠、WGSL 前向都在这里，
//! 不再依赖 `.yanagent/` 里的实验目录。当前只完成**前向**；反向内核与训练循环
//! 见 `README.md` 的缺口表。

#![forbid(unsafe_code)]

pub mod api;
pub mod atlas;
pub mod augment;
pub mod backward;
pub mod data;
pub mod depth;
pub mod depthnet;
pub mod diffuse;
pub mod fuse;
pub mod geo;
pub mod inst;
pub mod job;
pub mod light;
// 观感/风格测量的正本在独立仓 bfxh/trit-look（git 依赖，pin rev）。
pub use trit_look as look;
pub mod loss;
pub mod matnet;
pub mod mesh;
pub mod model;
pub mod occ;
pub mod ood;
pub mod parts;
pub mod ridge;
pub mod splat;
pub mod ste;
pub mod tconv;
pub mod tex;
pub mod tpm;
pub mod uv;
pub mod weights;

#[cfg(feature = "gpu")]
pub mod backend_wgpu;
#[cfg(feature = "gpu")]
pub mod expand;
#[cfg(feature = "gpu")]
pub mod forward;
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "gpu")]
pub mod gpu_bwd;
#[cfg(feature = "gpu")]
pub mod net;
#[cfg(feature = "gpu")]
pub mod state;
