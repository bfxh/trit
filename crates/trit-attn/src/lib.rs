//! 三值注意力 / 快权重记忆实验层（`publish = false`，不属于稳定 API）。
//!
//! - [`scores`]：注意力与「宽度为序列长度的 FFN」双视角，两者数值等价；
//! - [`memory`]：把「键 → 值」现场写进整数矩阵（快权重），用查询解码——三值编码器/解码器；
//! - [`block`]：可训练的三值注意力块（注意力 + 残差 + FFN 的训练集成，QAT + 全链 STE 反向）；
//! - [`train`]：关联回忆训练任务——把 [`memory`] 的手工快权重换成学出来的 [`block`]。
//!
//! 这一层的价值不在压缩，而在**看清楚注意力到底是什么**：一个由输入现场装配、
//! 宽度随上下文增长的两层网络——并且它能被三值 QAT 训练出来。

#![forbid(unsafe_code)]

pub mod block;
pub mod memory;
pub mod scores;
pub mod train;

pub use block::{AttentionBlock, BlockOptimizers};
pub use memory::{RecallReport, TernaryMemory, recall_experiment};
pub use scores::{attention, ffn_view, quantize, quantize_matrix};
pub use train::{
    RecallDataset, RecallSample, RecallTrainOptions, RecallTrainReport, evaluate, recall_dataset,
    train_recall,
};
