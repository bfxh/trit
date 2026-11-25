//! `trit-verified`：主库三值/四值语义的 Verus 验证锚点（K3 / Ł3 / B3 / Belnap B4）。
//! 验证链、运行方式与缺口见 README；由 `scripts/verify.py` 驱动，不进入运行时依赖。

pub mod b3;
pub mod b4;
pub mod k3;
pub mod l3;
pub mod table;
