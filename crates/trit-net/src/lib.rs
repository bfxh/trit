//! 三值权重网络的实验层。
//!
//! **不属于稳定 API**（`publish = false`）。它把两件事放在一起：
//!
//! - **权重三值** `{-1,0,+1}`：`i8` 存储，2 bit 的量化目标；
//! - **逻辑三值** `{F,X,T}`：输入可以是 `X`，输出区分「确定」与「弃权」。
//!
//! 关键读数（本机原型，见 PR 说明）：朴素区间传播会把 96–100% 本可确定的样本判成弃权，
//! 精确枚举才能保住精度——所以这个实验层的价值在 [`analyze`]，不在权重压缩本身。

#![forbid(unsafe_code)]

pub mod format;
pub mod netsec;
pub mod train;

mod infer;
mod model;

pub use infer::{Analysis, DEFAULT_BUDGET, Outcome, Sign, Witness, analyze, forward};
pub use model::{Layer, Model, ModelError};
pub use train::{TrainError, TrainOptions, TrainReport, majority_dataset, train};
