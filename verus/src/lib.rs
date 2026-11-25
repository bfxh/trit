//! `trit-verified`：主库三值语义的 Verus 验证锚点。
//!
//! - `table`：K3 / 经典三值算子的编码真值表（普通 Rust，单一来源）。
//! - `k3`：Verus `spec fn` + 代数律证明 + exec 提炼；由 `cargo-verus verify` 检查。
//! - `tests/bridge.rs`：用普通 Rust 把 `table` 与 `trit-core::value::Ternary`
//!   的实现逐项对拍。
//!
//! 验证链：`trit-core ≡ table`（穷举对拍，3×3 完备）
//!         + `table ≡ spec`（Verus 证明）
//!         ⇒ `trit-core` 的 K3 算子满足被证明的 spec。
//!
//! 本 crate 是**验证锚点**，不是运行时依赖：主库的 `cargo build/test` 不构建它，
//! 由 `scripts/verify.py` 单独驱动。

pub mod k3;
pub mod table;
