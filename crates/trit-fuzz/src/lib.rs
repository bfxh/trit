//! `trit-fuzz`：三值测试语料的确定性生成器。
//!
//! 只造进料（随机 trit 向量、随机表达式），不做求解、不进产品路径。
//! RNG 复用 [`trit_bench::SeedRng`]（同一组 xorshift 移位 13/7/17），
//! 所以"换包"不改变序列口径；同种子必须能逐条重放同一份语料。

pub mod corpus;

pub use trit_bench::SeedRng;
