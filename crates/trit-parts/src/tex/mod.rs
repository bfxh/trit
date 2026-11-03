//! 程序化材质（N5 的第一块）：可平铺的 2D 纹理 + 无缝度判据。
//!
//! 全部用**周期函数**构造（周期 = 贴图边长），所以左右/上下边界天然连续；
//! `seam_ratio` 是判据：边界相邻差 / 内部相邻差，≈1 表示无缝。
//! `quilt` 是**非参数**合成：直接从源图搬 patch，统计由构造保证（学出来的合成见 `tconv`）。

/// splitmix64：给 patch 搜索用（`seed | 1` 会丢最低位，2026-11-01 踩过）。
mod quilt;
mod style;
mod tiles;

pub(crate) use quilt::box_blur_rgb;
pub use quilt::quilt;
pub use style::{PRESETS, Preset, Style, apply_pointwise, stylize};
pub use tiles::{seam_ratio, tile};

#[cfg(test)]
mod tests;
