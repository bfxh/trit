//! 三值编解码实验层（`publish = false`，不属于稳定 API）。
//!
//! 多个方向的「编码器 / 解码器」：
//!
//! - [`plan`]：按数据分布选最省编码（无 `X` → 1 bit/trit；极稀疏 → 索引；其余 1.6 bit/trit）。
//! - [`pack`]：5 trits / byte 二进制打包（1.6 bit/trit，比双导轨 2 bit/trit 省 20%）。
//! - [`balanced`]：平衡三进制整数 `+0-` 文本，i64 全域 41 位。
//! - [`hamming`]：GF(3) 上的 `[4,2,3]` 纠错码，保证纠正 1 个符号错误。
//! - [`quantize`]：f32 权重 ↔ trit 三值量化/反量化（TWN 风格，beta=0.7）。
//! - [`sparse`]：三值稀疏 COO 编码，只存非 Unknown 位置。
//! - [`interleave`]：块交织/解交织，把 burst error 打散后配合 hamming 纠错。
//! - [`checksum`]：GF(3) CRC-3 完整性校验，只检错不纠错。
//! - [`rle`]：三值游程编码，连续相同 trit 压成 `(value, count)` 对，可逆。
//! - [`delta`]：三值差分编码，存相邻 trit 的差值 mod 3，适合缓变序列。
//! - [`gray`]：三值反射格雷码，相邻 index 的码字汉明距离恒为 1。
//! - [`sop`]：三值 Quine-McCluskey 化简，真值表 → 最简积之和（SOP）。
//! - [`count`]：三值 #SAT，数一组 SOP 积项覆盖了多少个具体赋值。
//!
//! 大多数都是**无损**的：解码后必须逐位还原，任何非法输入都要显式报错而不是猜；
//! 唯一有损的是 [`quantize`]——它的目的就是压缩浮点到三值。

#![forbid(unsafe_code)]

pub mod balanced;
pub mod checksum;
pub mod count;
pub mod delta;
pub mod gray;
pub mod hamming;
pub mod interleave;
pub mod pack;
pub mod plan;
pub mod quantize;
pub mod rle;
pub mod sop;
pub mod sparse;

mod error;
pub use error::CodecError;
