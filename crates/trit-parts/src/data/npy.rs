//! `.npy` 的 i32 读写 —— 实现已统一到 `crate::npy`。
//!
//! 这里只保留历史入口名（`data::read_npy_i32` / `data::write_npy_i32`），
//! 逻辑不再有两份：原先 `data` 与 `geo` 各写一份头解析，`header_len` 的边界
//! 在两边同时缺（L65）。

pub use crate::npy::{read_i32 as read_npy_i32, write_i32 as write_npy_i32};
