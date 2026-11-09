//! 三值权重的扩散模型实验层（DDPM / DDIM，`publish = false`，不属于稳定 API）。
//!
//! 复用 [`trit_learn`] 的三值 QAT 原语——absmean 量化 + STE 反向 + Adam——
//! 把「三进制」这条主线推进到生成模型：去噪网络的**权重**是 `{-1, 0, +1}`，
//! 每行一个 f32 scale，训练中始终按量化后的权重前向（QAT），因此不存在
//! 「训练完再量化」的精度落差。
//!
//! - [`schedule`]：β / α / ᾱ 预计算（线性、余弦两种）；
//! - [`data`]：可判定的 2D 数据集（细圆环 + 圆盘）与确定性 PRNG；
//! - [`denoiser`]：三值 MLP 去噪器（时间步嵌入 + 两层 tanh + 线性输出）与 FP 对照；
//! - [`train`]：DDPM 训练循环（随机 t、预测 ε、MSE）；
//! - [`sample`]：DDPM / DDIM 采样与分布统计。
//!
//! 设计约束：偏置与时间步嵌入保持 f32（三值化只作用于线性层权重），
//! 这是 BitNet 系 QAT 的常见做法——把尺度类参数也三值化会让训练无法起步。

#![forbid(unsafe_code)]

pub mod data;
pub mod denoiser;
pub mod sample;
pub mod schedule;
pub mod train;

pub use data::{Xorshift, sample_ring};
pub use denoiser::{Denoiser, TernaryMlp};
pub use sample::{RingStats, ddim_sample, ddpm_sample, ring_stats};
pub use schedule::{DiffusionSchedule, Schedule};
pub use train::{TrainOptions, TrainReport, train};
